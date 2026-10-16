//! The account worker: one per account, on a thread of its own with a single-threaded runtime
//! and its own connection to the store. It runs the [`SyncMachine`] and carries out its
//! actions: connect, replay the offline queue, sync, wait in IDLE on the inbox (renewed within
//! [`IDLE_RENEW`], a full sync every [`POLL_INTERVAL`]), back off after failures, wait for a new
//! password after a refused login. A second connection, opened when needed, serves what the
//! user waits for: bodies, attachments, server search.
//!
//! The shell creates workers ([`Worker::spawn`]), tells them what happened ([`Command`]) and
//! hands their [`Notice`]s to the UI as events. Everything here runs against the
//! [`Connector`] trait, so the tests drive a worker with the in-memory fake server.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Serialize;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::account::{Account, Auth, Credential, Transport};
use crate::error::{Error, Result};
use crate::imap::client::ImapSession;
use crate::imap::{ANSWERED, Capabilities, FLAGGED, IdleOutcome, ImapClient, Role, SEEN, Search};
use crate::network::Problem;
use crate::oauth::TokenSource;
use crate::outbox::{self, Op};
use crate::store::{Store, now};
use crate::sync::{
    Action, Event, IDLE_RENEW, POLL_INTERVAL, State, SyncMachine, fetch_attachment, fetch_body, server_search,
    sync_folders, sync_mailbox,
};
use crate::{autoconfig, smtp, tr};

/// How a worker reaches the account's servers.
pub trait Connector: Send + 'static {
    type Imap: ImapClient + 'static;
    /// A new, signed-in IMAP connection.
    fn imap(&self) -> impl Future<Output = Result<Self::Imap>>;
    /// Sends a finished message.
    fn send(&self, from: &str, to: &[String], raw: &[u8]) -> impl Future<Output = Result<()>>;
    /// Whether the server files sent mail in Sent itself (Gmail, Microsoft).
    fn files_sent(&self) -> bool;
    /// The user signed in again.
    fn set_password(&self, password: String);
}

/// How a [`NetConnector`] signs in.
enum Secret {
    Password(Mutex<String>),
    /// Microsoft: access tokens for XOAUTH2 from the refresh token.
    OAuth(TokenSource),
}

/// The real servers of an IMAP/SMTP account: with a password, or with OAuth (XOAUTH2).
pub struct NetConnector {
    transport: Transport,
    secret: Secret,
}

fn unsupported() -> Error {
    Error::Invalid(
        tr!(
            "Diese Kontoart wird in dieser Version noch nicht unterstützt.",
            "This kind of account is not supported in this version yet."
        )
        .into(),
    )
}

/// Whether the server refused the sign-in itself (as opposed to the connection failing).
fn refused(e: &Error) -> bool {
    matches!(e, Error::Connection(Problem::Auth { .. }))
}

impl NetConnector {
    pub fn new(account: &Account, password: String) -> Result<Self> {
        match &account.transport {
            Transport::Imap { auth: Auth::Password, .. } => Ok(NetConnector {
                transport: account.transport.clone(),
                secret: Secret::Password(Mutex::new(password)),
            }),
            _ => Err(unsupported()),
        }
    }

    /// An account that signs in with OAuth; `tokens` holds its refresh token.
    pub fn with_oauth(account: &Account, tokens: TokenSource) -> Result<Self> {
        match &account.transport {
            Transport::Imap { auth: Auth::OAuth2(_), .. } => {
                Ok(NetConnector { transport: account.transport.clone(), secret: Secret::OAuth(tokens) })
            }
            _ => Err(unsupported()),
        }
    }

    /// The sign-in for the next connection; `fresh`: a new access token (the last one was refused).
    async fn credential(&self, fresh: bool) -> Result<Credential> {
        match &self.secret {
            Secret::Password(p) => Ok(Credential::Password(p.lock().unwrap_or_else(|e| e.into_inner()).clone())),
            Secret::OAuth(t) => Ok(Credential::XOAuth2(t.access_token(fresh).await?)),
        }
    }

    fn oauth(&self) -> bool {
        matches!(self.secret, Secret::OAuth(_))
    }
}

impl Connector for NetConnector {
    type Imap = ImapSession;

    async fn imap(&self) -> Result<ImapSession> {
        let Transport::Imap { imap, username, .. } = &self.transport else { unreachable!("checked in new") };
        match ImapSession::connect_with(imap, username, &self.credential(false).await?).await {
            // An access token the server no longer takes (revoked, the clock): once more with
            // a fresh one before the user is asked to sign in.
            Err(e) if self.oauth() && refused(&e) => {
                ImapSession::connect_with(imap, username, &self.credential(true).await?).await
            }
            r => r,
        }
    }

    async fn send(&self, from: &str, to: &[String], raw: &[u8]) -> Result<()> {
        let Transport::Imap { smtp, username, .. } = &self.transport else { unreachable!("checked in new") };
        match smtp::send_with(smtp, username, &self.credential(false).await?, from, to, raw).await {
            Err(e) if self.oauth() && refused(&e) => {
                smtp::send_with(smtp, username, &self.credential(true).await?, from, to, raw).await
            }
            r => r,
        }
    }

    fn files_sent(&self) -> bool {
        self.oauth() || matches!(&self.transport, Transport::Imap { smtp, .. } if autoconfig::server_files_sent(smtp))
    }

    fn set_password(&self, password: String) {
        match &self.secret {
            Secret::Password(p) => *p.lock().unwrap_or_else(|e| e.into_inner()) = password,
            Secret::OAuth(t) => t.signed_in(password, None),
        }
    }
}

/// What the shell tells a worker.
#[derive(Debug)]
pub enum Command {
    /// Sync everything now (the user asked, the app came to the front).
    SyncNow,
    /// The offline queue has something new.
    Kick,
    NetworkUp,
    NetworkDown,
    /// The computer woke up or changed networks: drop the connection and connect now.
    Reconnect,
    /// A new password after „Anmeldung erforderlich“.
    SignedIn(String),
    FetchBody {
        message_id: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    FetchAttachment {
        attachment_id: i64,
        reply: oneshot::Sender<Result<Vec<u8>>>,
    },
    ServerSearch {
        mailbox_ids: Vec<i64>,
        query: String,
        reply: oneshot::Sender<Result<Vec<i64>>>,
    },
    Stop,
}

/// Where an account is, for the status line of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Status {
    Connecting,
    Syncing,
    /// Connected and up to date.
    Ready,
    Offline,
    /// A failure; the next attempt follows by itself.
    Retrying {
        in_secs: u64,
    },
    /// The login was refused: the password is needed again.
    SignIn,
    Stopped,
}

/// What a worker tells the shell (and the shell the UI, as events).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Notice {
    Status {
        account_id: i64,
        status: Status,
        problem: Option<Problem>,
        error: Option<String>,
    },
    /// Messages or folders changed: these folders need a fresh list.
    Changed {
        account_id: i64,
        mailboxes: Vec<i64>,
    },
    /// New unread mail arrived in the inbox.
    NewMail {
        account_id: i64,
        messages: Vec<i64>,
    },
    /// The offline queue changed (a step done, failed or refused).
    Outbox {
        account_id: i64,
    },
    /// A message went out.
    Sent {
        account_id: i64,
        outbox_id: i64,
    },
}

pub type Sink = Arc<dyn Fn(Notice) + Send + Sync>;

/// A running worker; dropping it stops the worker. Its [`WorkerHandle`] (also through
/// `Deref`) talks to it.
pub struct Worker {
    pub account_id: i64,
    handle: WorkerHandle,
}

/// Talks to a worker; cheap to clone, so a command can wait for an answer without holding a
/// lock on the list of workers.
#[derive(Debug, Clone)]
pub struct WorkerHandle {
    tx: mpsc::UnboundedSender<Command>,
}

impl std::ops::Deref for Worker {
    type Target = WorkerHandle;
    fn deref(&self) -> &WorkerHandle {
        &self.handle
    }
}

impl Worker {
    pub fn handle(&self) -> WorkerHandle {
        self.handle.clone()
    }
}

impl Worker {
    /// Starts the worker of an account on its own thread, with its own store connection to
    /// `db` (`None`: an in-memory store, for tests that only look at the server).
    pub fn spawn<C: Connector>(account_id: i64, db: PathBuf, connector: C, sink: Sink) -> Result<Worker> {
        let (tx, rx) = mpsc::unbounded_channel();
        let store = Store::open(&db)?;
        std::thread::Builder::new()
            .name(format!("account-{account_id}"))
            // The sync futures are deep; debug builds need more than the default 2 MB.
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(_) => return,
                };
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, run(account_id, store, connector, rx, sink));
            })
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Worker { account_id, handle: WorkerHandle { tx } })
    }
}

impl WorkerHandle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    fn gone() -> Error {
        Error::Network(tr!("Das Konto ist nicht verbunden.", "The account is not connected.").into())
    }

    pub async fn fetch_body(&self, message_id: i64) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::FetchBody { message_id, reply });
        rx.await.map_err(|_| Self::gone())?
    }

    pub async fn fetch_attachment(&self, attachment_id: i64) -> Result<Vec<u8>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::FetchAttachment { attachment_id, reply });
        rx.await.map_err(|_| Self::gone())?
    }

    pub async fn server_search(&self, mailbox_ids: Vec<i64>, query: String) -> Result<Vec<i64>> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::ServerSearch { mailbox_ids, query, reply });
        rx.await.map_err(|_| Self::gone())?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.handle.send(Command::Stop);
    }
}

/// Whether an error is the connection's (worth a reconnect) rather than the server refusing.
pub fn is_transient(e: &Error) -> bool {
    match e {
        Error::Connection(p) => p.is_transient(),
        Error::Network(_) => true,
        _ => false,
    }
}

struct Ctx<C: Connector> {
    account_id: i64,
    store: Rc<Store>,
    connector: Rc<C>,
    sink: Sink,
}

impl<C: Connector> Ctx<C> {
    fn notice(&self, n: Notice) {
        (self.sink)(n)
    }

    fn status(&self, status: Status, error: Option<&Error>) {
        crate::diag::log(
            &format!("account-{}", self.account_id),
            &match error {
                Some(e) => format!("{status:?}: {e}"),
                None => format!("{status:?}"),
            },
        );
        let problem = match error {
            Some(Error::Connection(p)) => Some(p.clone()),
            _ => None,
        };
        self.notice(Notice::Status {
            account_id: self.account_id,
            status,
            problem,
            error: error.map(|e| e.to_string()),
        });
    }

    fn mailbox_named(&self, name: &str) -> Option<i64> {
        self.store.mailbox_by_name(self.account_id, name).ok().flatten().map(|m| m.id)
    }

    fn inbox(&self) -> Option<crate::store::Mailbox> {
        self.store.mailbox_by_role(self.account_id, Role::Inbox).ok().flatten()
    }
}

fn b64(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| Error::Invalid(e.to_string()))
}

/// Carries out one step of the queue; returns the folders it touched.
async fn apply<C: Connector>(ctx: &Ctx<C>, conn: &mut C::Imap, id: i64, op: &Op) -> Result<Vec<String>> {
    match op {
        Op::Send { raw_b64, from, recipients, sent_b64, sent_mailbox, draft, answered } => {
            ctx.connector.send(from, recipients, &b64(raw_b64)?).await?;
            ctx.notice(Notice::Sent { account_id: ctx.account_id, outbox_id: id });
            // Sent: what is left is filing it; that step never sends again.
            let rest = Op::AppendSent {
                mailbox: if ctx.connector.files_sent() { None } else { sent_mailbox.clone() },
                raw_b64: sent_b64.clone().or_else(|| Some(raw_b64.clone())),
                draft: draft.clone(),
                answered: answered.clone(),
            };
            outbox::replace(&ctx.store, id, &rest)?;
            let mut touched = Box::pin(apply(ctx, conn, id, &rest)).await?;
            // The server filed it (Gmail, Microsoft, JMAP): Sent is synced right away.
            if ctx.connector.files_sent()
                && let Some(sent) = sent_mailbox
            {
                touched.push(sent.clone());
            }
            Ok(touched)
        }
        Op::AppendSent { mailbox, raw_b64, draft, answered } => {
            let mut touched = vec![];
            if let (Some(mb), Some(raw)) = (mailbox, raw_b64) {
                conn.append(mb, &[SEEN], &b64(raw)?).await?;
                touched.push(mb.clone());
            }
            if let Some(mid) = draft
                && let Some(drafts) = ctx.store.mailbox_by_role(ctx.account_id, Role::Drafts)?
            {
                conn.select(&drafts.name, None).await?;
                let uids = conn.search(&Search::MessageId(mid.clone())).await?;
                conn.expunge(&uids).await?;
                touched.push(drafts.name);
            }
            if let Some((mb, uid)) = answered {
                conn.select(mb, None).await?;
                conn.store(&[*uid], &[ANSWERED], true).await?;
                touched.push(mb.clone());
            }
            Ok(touched)
        }
        Op::Flags { mailbox, uids, seen, flagged } => {
            conn.select(mailbox, None).await?;
            if let Some(s) = seen {
                conn.store(uids, &[SEEN], *s).await?;
            }
            if let Some(f) = flagged {
                conn.store(uids, &[FLAGGED], *f).await?;
            }
            Ok(vec![mailbox.clone()])
        }
        Op::Move { from, to, uids, ids } => {
            conn.select(from, None).await?;
            conn.move_to(uids, to).await?;
            ctx.store.remove_messages(ids)?;
            Ok(vec![from.clone(), to.clone()])
        }
        Op::Delete { mailbox, uids, ids } => {
            conn.select(mailbox, None).await?;
            conn.expunge(uids).await?;
            ctx.store.remove_messages(ids)?;
            Ok(vec![mailbox.clone()])
        }
        Op::SaveDraft { mailbox, raw_b64, message_id } => {
            conn.select(mailbox, None).await?;
            let old = conn.search(&Search::MessageId(message_id.clone())).await?;
            conn.append(mailbox, &[crate::imap::DRAFT, SEEN], &b64(raw_b64)?).await?;
            conn.select(mailbox, None).await?;
            conn.expunge(&old).await?;
            Ok(vec![mailbox.clone()])
        }
        Op::DeleteDraft { mailbox, message_id } => {
            conn.select(mailbox, None).await?;
            let uids = conn.search(&Search::MessageId(message_id.clone())).await?;
            conn.expunge(&uids).await?;
            Ok(vec![mailbox.clone()])
        }
        Op::CreateFolder { name } => {
            conn.create(name).await?;
            Ok(vec![])
        }
        Op::RenameFolder { from, to } => {
            conn.rename(from, to).await?;
            Ok(vec![])
        }
        Op::DeleteFolder { name } => {
            conn.delete(name).await?;
            Ok(vec![])
        }
    }
}

/// Replays the due steps of the queue in order. A connection failure stops the replay (the
/// caller reconnects); a refusal parks the step and goes on.
async fn replay<C: Connector>(ctx: &Ctx<C>, conn: &mut C::Imap) -> Result<Vec<String>> {
    let mut touched = vec![];
    while let Some(q) = outbox::next_due(&ctx.store, ctx.account_id, now())? {
        outbox::start(&ctx.store, q.id)?;
        let result = apply(ctx, conn, q.id, &q.op).await;
        // The notice follows the queue's new state: a UI that reads the queue when it hears
        // of it must not see the step that just finished.
        let outcome = match result {
            Ok(t) => {
                outbox::done(&ctx.store, q.id)?;
                touched.extend(t);
                None
            }
            Err(e) if is_transient(&e) => {
                crate::diag::log(
                    &format!("account-{}", ctx.account_id),
                    &format!("{} failed, retried later: {e}", q.op.kind()),
                );
                outbox::failed(&ctx.store, q.id, &e.to_string(), now())?;
                Some(e)
            }
            Err(e) => {
                crate::diag::log(&format!("account-{}", ctx.account_id), &format!("{} refused: {e}", q.op.kind()));
                outbox::park(&ctx.store, q.id, &e.to_string())?;
                None
            }
        };
        ctx.notice(Notice::Outbox { account_id: ctx.account_id });
        if let Some(e) = outcome {
            // A send that could not reach the SMTP server: the IMAP connection is fine.
            if matches!(q.op, Op::Send { .. }) {
                break;
            }
            return Err(e);
        }
    }
    Ok(touched)
}

/// One round: the queue, then the folders (all of them, or the inbox and what the queue
/// touched).
async fn sync_round<C: Connector>(ctx: &Ctx<C>, conn: &mut C::Imap, caps: Capabilities, full: bool) -> Result<()> {
    // The settings may have changed since the last round.
    let settings = crate::settings::load(&ctx.store).unwrap_or_default();
    ctx.store.set_subject_threads(settings.subject_threads);
    let touched = replay(ctx, conn).await?;
    let mut ids: Vec<i64> = vec![];
    let mut virtual_folders: Vec<i64> = vec![];
    if full {
        sync_folders(conn, &ctx.store, ctx.account_id).await?;
        ctx.notice(Notice::Changed { account_id: ctx.account_id, mailboxes: vec![] });
        // Folders that show other folders' mail again (Gmail's All Mail) come last: their
        // messages find the bodies of the copies synced before them.
        let mut list: Vec<_> = ctx.store.mailboxes(ctx.account_id)?.into_iter().filter(|m| m.selectable).collect();
        list.sort_by_key(|m| m.role.is_some_and(Role::is_virtual));
        virtual_folders = list.iter().filter(|m| m.role.is_some_and(Role::is_virtual)).map(|m| m.id).collect();
        ids = list.into_iter().map(|m| m.id).collect();
    } else {
        ids.extend(ctx.inbox().map(|m| m.id));
        ids.extend(touched.iter().filter_map(|n| ctx.mailbox_named(n)));
        ids.dedup();
    }
    let inbox = ctx.inbox().map(|m| m.id);
    for &id in &ids {
        let r = sync_mailbox(conn, &ctx.store, caps, id).await?;
        if r.changed() || touched.iter().any(|n| ctx.mailbox_named(n) == Some(id)) {
            ctx.notice(Notice::Changed { account_id: ctx.account_id, mailboxes: vec![id] });
        }
        if Some(id) == inbox && r.added > 0 {
            // Mail of senders the user turned away (the Screener) leaves the inbox at once.
            let moved = crate::triage::move_blocked(&ctx.store, Some(ctx.account_id))?;
            if !moved.hidden.is_empty() {
                ctx.notice(Notice::Changed { account_id: ctx.account_id, mailboxes: vec![id] });
            }
        }
        if Some(id) == inbox && !r.new_unread.is_empty() {
            ctx.notice(Notice::NewMail { account_id: ctx.account_id, messages: r.new_unread });
        }
    }
    // The first lines for the list, the text for search, reading offline: the inbox first,
    // then the other folders of a full round.
    let mut ahead: Vec<i64> = inbox.into_iter().collect();
    ahead.extend(ids.iter().copied().filter(|id| Some(*id) != inbox && !virtual_folders.contains(id)));
    for id in ahead {
        if crate::sync::prefetch_bodies(conn, &ctx.store, id, settings.prefetch).await? > 0 {
            ctx.notice(Notice::Changed { account_id: ctx.account_id, mailboxes: vec![id] });
        }
    }
    Ok(())
}

/// What the queued commands mean for the machine.
fn take_events<C: Connector>(
    ctx: &Ctx<C>,
    queue: &RefCell<VecDeque<Command>>,
    state: State,
    force_full: &Cell<bool>,
) -> Vec<Event> {
    let mut out = vec![];
    while let Some(cmd) = queue.borrow_mut().pop_front() {
        match cmd {
            Command::SyncNow => {
                force_full.set(true);
                out.push(Event::Retry);
            }
            // A new step of the queue: replayed right away, unless the connection is down.
            Command::Kick if !matches!(state, State::Backoff { .. } | State::NeedsSignIn | State::Offline) => {
                out.push(Event::Retry)
            }
            Command::Kick => {}
            Command::NetworkUp => out.push(Event::NetworkUp),
            Command::Reconnect => {
                force_full.set(true);
                out.push(Event::Reconnect)
            }
            Command::NetworkDown => out.push(Event::NetworkDown),
            Command::SignedIn(pw) => {
                ctx.connector.set_password(pw);
                out.push(Event::Retry);
            }
            Command::Stop => out.push(Event::Stop),
            // Answered by the on-demand connection.
            Command::FetchBody { .. } | Command::FetchAttachment { .. } | Command::ServerSearch { .. } => {}
        }
    }
    out
}

async fn run<C: Connector>(
    account_id: i64,
    store: Store,
    connector: C,
    mut rx: mpsc::UnboundedReceiver<Command>,
    sink: Sink,
) {
    let _ = outbox::recover(&store);
    let ctx = Rc::new(Ctx { account_id, store: Rc::new(store), connector: Rc::new(connector), sink });
    let wake = Rc::new(Notify::new());
    let queue: Rc<RefCell<VecDeque<Command>>> = Rc::default();
    let stop = Rc::new(Cell::new(false));
    let (itx, irx) = mpsc::unbounded_channel::<Command>();
    {
        let (wake, queue, stop) = (wake.clone(), queue.clone(), stop.clone());
        tokio::task::spawn_local(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Command::FetchBody { .. } | Command::FetchAttachment { .. } | Command::ServerSearch { .. } => {
                        let _ = itx.send(cmd);
                    }
                    Command::Stop => break,
                    other => {
                        queue.borrow_mut().push_back(other);
                        wake.notify_one();
                    }
                }
            }
            stop.set(true);
            queue.borrow_mut().push_back(Command::Stop);
            wake.notify_one();
        });
    }
    tokio::task::spawn_local(on_demand(ctx.clone(), irx));
    main_loop(&ctx, &wake, &queue, &stop).await;
}

/// One check without a worker, for the phones' background check (WorkManager on Android,
/// background app refresh on iOS), where no connection may stay open: connect, replay the
/// offline queue, sync the inbox (every folder when the account has none yet) and fetch its
/// first lines, log out. Returns the new unread messages of the inbox. The caller bounds the
/// time it may take.
pub async fn check_once<C: Connector>(account_id: i64, store: Store, connector: C) -> Result<Vec<i64>> {
    let _ = outbox::recover(&store);
    let found: Arc<Mutex<Vec<i64>>> = Arc::default();
    let into = found.clone();
    let sink: Sink = Arc::new(move |n| {
        if let Notice::NewMail { messages, .. } = n {
            into.lock().unwrap_or_else(|e| e.into_inner()).extend(messages);
        }
    });
    let ctx = Ctx { account_id, store: Rc::new(store), connector: Rc::new(connector), sink };
    let mut conn = ctx.connector.imap().await?;
    let caps = conn.capabilities().await.unwrap_or_default();
    let full = ctx.inbox().is_none();
    let r = sync_round(&ctx, &mut conn, caps, full).await;
    let _ = conn.logout().await;
    r?;
    let mut out = found.lock().unwrap_or_else(|e| e.into_inner()).clone();
    out.dedup();
    Ok(out)
}

async fn main_loop<C: Connector>(ctx: &Ctx<C>, wake: &Notify, queue: &RefCell<VecDeque<Command>>, stop: &Cell<bool>) {
    let mut m = SyncMachine::default();
    let mut actions: VecDeque<Action> = m.on(Event::Start).into();
    let mut conn: Option<C::Imap> = None;
    let mut caps = Capabilities::default();
    let mut last_full: Option<Instant> = None;
    let force_full = Cell::new(true);
    let mut last_error: Option<Error> = None;
    loop {
        if stop.get() {
            if let Some(mut c) = conn.take() {
                let _ = c.logout().await;
            }
            ctx.status(Status::Stopped, None);
            return;
        }
        let Some(action) = actions.pop_front() else {
            // Nothing to do until the user or the system says something (signed out, offline).
            wake.notified().await;
            for ev in take_events(ctx, queue, m.state, &force_full) {
                actions.extend(m.on(ev));
            }
            continue;
        };
        match action {
            Action::Connect => {
                ctx.status(Status::Connecting, None);
                match ctx.connector.imap().await {
                    Ok(mut c) => {
                        caps = c.capabilities().await.unwrap_or_default();
                        conn = Some(c);
                        last_error = None;
                        actions.extend(m.on(Event::Connected { caps }));
                    }
                    Err(e) => {
                        let auth = matches!(&e, Error::Connection(p) if p.needs_sign_in());
                        last_error = Some(e);
                        actions.extend(m.on(Event::ConnectFailed { auth }));
                    }
                }
            }
            Action::Sync => {
                let Some(c) = conn.as_mut() else {
                    actions.extend(m.on(Event::SyncFailed));
                    continue;
                };
                let full = force_full.get() || last_full.is_none_or(|t| t.elapsed() >= POLL_INTERVAL);
                if full {
                    ctx.status(Status::Syncing, None);
                }
                match sync_round(ctx, c, caps, full).await {
                    Ok(()) => {
                        if full {
                            last_full = Some(Instant::now());
                            force_full.set(false);
                        }
                        ctx.status(Status::Ready, None);
                        actions.extend(m.on(Event::SyncDone));
                    }
                    Err(e) => {
                        last_error = Some(e);
                        actions.extend(m.on(Event::SyncFailed));
                    }
                }
            }
            Action::Idle => {
                let Some(c) = conn.as_mut() else {
                    actions.extend(m.on(Event::SyncFailed));
                    continue;
                };
                // Wake in time for the next full sync and the next due step of the queue.
                let mut timeout = POLL_INTERVAL.saturating_sub(last_full.map_or(Duration::ZERO, |t| t.elapsed()));
                if let Ok(Some(due)) = outbox::next_due_at(&ctx.store, ctx.account_id) {
                    timeout = timeout.min(Duration::from_secs((due - now()).max(0) as u64 + 1));
                }
                let timeout = timeout.clamp(Duration::from_secs(1), IDLE_RENEW);
                let inbox = ctx.inbox().map(|m| m.name).unwrap_or_else(|| "INBOX".into());
                let r = match c.select(&inbox, None).await {
                    Ok(_) => c.idle(timeout, wake).await,
                    Err(e) => Err(e),
                };
                match r {
                    Ok(outcome) => {
                        let events = take_events(ctx, queue, m.state, &force_full);
                        let stopping =
                            events.iter().any(|e| matches!(e, Event::Stop | Event::NetworkDown | Event::Reconnect));
                        if !stopping {
                            actions.extend(m.on(Event::Idle(outcome)));
                        }
                        for ev in events.into_iter().filter(|e| *e != Event::Retry) {
                            actions.extend(m.on(ev));
                        }
                    }
                    Err(e) => {
                        last_error = Some(e);
                        actions.extend(m.on(Event::SyncFailed));
                    }
                }
            }
            Action::Disconnect => {
                if let Some(mut c) = conn.take() {
                    let _ = c.logout().await;
                }
                if m.state == State::Offline {
                    ctx.status(Status::Offline, None);
                }
            }
            Action::Wait(d) => {
                if let State::Backoff { .. } = m.state {
                    ctx.status(Status::Retrying { in_secs: d.as_secs() }, last_error.as_ref());
                }
                let until = tokio::time::Instant::now() + d;
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(until) => {
                            actions.extend(m.on(Event::Timer));
                            break;
                        }
                        _ = wake.notified() => {
                            let events = take_events(ctx, queue, m.state, &force_full);
                            if events.is_empty() {
                                continue;
                            }
                            for ev in events {
                                actions.extend(m.on(ev));
                            }
                            break;
                        }
                    }
                }
            }
            Action::AskToSignIn => ctx.status(Status::SignIn, last_error.as_ref()),
        }
    }
}

/// The on-demand connection: bodies, attachments and server search, one request at a time, so
/// the user does not wait for a long sync. Reconnects once when the connection broke.
async fn on_demand<C: Connector>(ctx: Rc<Ctx<C>>, mut rx: mpsc::UnboundedReceiver<Command>) {
    let mut conn: Option<C::Imap> = None;
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Command::FetchBody { message_id, reply } => {
                let mut result = Err(WorkerHandle::gone());
                for _ in 0..2 {
                    let c = match connected(&ctx, &mut conn).await {
                        Ok(c) => c,
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    };
                    result = fetch_body(c, &ctx.store, message_id).await;
                    if !matches!(&result, Err(e) if is_transient(e)) {
                        break;
                    }
                    conn = None;
                }
                let _ = reply.send(result);
            }
            Command::FetchAttachment { attachment_id, reply } => {
                let mut result = Err(WorkerHandle::gone());
                for _ in 0..2 {
                    // A kept message needs no connection.
                    if let Ok(a) = ctx.store.attachment(attachment_id)
                        && ctx.store.raw_message(a.message_id).ok().flatten().is_some()
                    {
                        let mut none = NoConnection;
                        result = fetch_attachment(&mut none, &ctx.store, attachment_id).await;
                        break;
                    }
                    let c = match connected(&ctx, &mut conn).await {
                        Ok(c) => c,
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    };
                    result = fetch_attachment(c, &ctx.store, attachment_id).await;
                    if !matches!(&result, Err(e) if is_transient(e)) {
                        break;
                    }
                    conn = None;
                }
                let _ = reply.send(result);
            }
            Command::ServerSearch { mailbox_ids, query, reply } => {
                let result = match connected(&ctx, &mut conn).await {
                    Ok(c) => server_search(c, &ctx.store, &mailbox_ids, &query).await,
                    Err(e) => Err(e),
                };
                if matches!(&result, Err(e) if is_transient(e)) {
                    conn = None;
                }
                let _ = reply.send(result);
            }
            _ => {}
        }
    }
    if let Some(mut c) = conn {
        let _ = c.logout().await;
    }
}

async fn connected<'a, C: Connector>(ctx: &Ctx<C>, conn: &'a mut Option<C::Imap>) -> Result<&'a mut C::Imap> {
    if conn.is_none() {
        *conn = Some(ctx.connector.imap().await?);
    }
    Ok(conn.as_mut().expect("just connected"))
}

/// A stand-in for reading attachments of kept messages: it never talks to a server.
struct NoConnection;

impl NoConnection {
    fn refuse<T>() -> Result<T> {
        Err(Error::Network("offline".into()))
    }
}

impl ImapClient for NoConnection {
    async fn capabilities(&mut self) -> Result<Capabilities> {
        Self::refuse()
    }
    async fn list(&mut self) -> Result<Vec<crate::imap::MailboxEntry>> {
        Self::refuse()
    }
    async fn select(&mut self, _: &str, _: Option<crate::imap::QresyncParams>) -> Result<crate::imap::Selected> {
        Self::refuse()
    }
    async fn uids(&mut self) -> Result<Vec<u32>> {
        Self::refuse()
    }
    async fn fetch_headers(&mut self, _: crate::imap::UidRange) -> Result<Vec<crate::imap::FetchedHeader>> {
        Self::refuse()
    }
    async fn fetch_flags(&mut self, _: crate::imap::UidRange, _: Option<u64>) -> Result<Vec<crate::imap::FlagUpdate>> {
        Self::refuse()
    }
    async fn fetch_message(&mut self, _: u32) -> Result<Vec<u8>> {
        Self::refuse()
    }
    async fn fetch_structure(&mut self, _: u32) -> Result<Vec<crate::imap::BodyPart>> {
        Self::refuse()
    }
    async fn fetch_section(&mut self, _: u32, _: &str) -> Result<Vec<u8>> {
        Self::refuse()
    }
    async fn idle(&mut self, _: Duration, _: &Notify) -> Result<IdleOutcome> {
        Self::refuse()
    }
    async fn store(&mut self, _: &[u32], _: &[&str], _: bool) -> Result<()> {
        Self::refuse()
    }
    async fn copy(&mut self, _: &[u32], _: &str) -> Result<Vec<(u32, u32)>> {
        Self::refuse()
    }
    async fn move_to(&mut self, _: &[u32], _: &str) -> Result<Vec<(u32, u32)>> {
        Self::refuse()
    }
    async fn expunge(&mut self, _: &[u32]) -> Result<()> {
        Self::refuse()
    }
    async fn append(&mut self, _: &str, _: &[&str], _: &[u8]) -> Result<Option<u32>> {
        Self::refuse()
    }
    async fn search(&mut self, _: &Search) -> Result<Vec<u32>> {
        Self::refuse()
    }
    async fn create(&mut self, _: &str) -> Result<()> {
        Self::refuse()
    }
    async fn delete(&mut self, _: &str) -> Result<()> {
        Self::refuse()
    }
    async fn rename(&mut self, _: &str, _: &str) -> Result<()> {
        Self::refuse()
    }
    async fn logout(&mut self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------- the setup's check

/// The result of the setup's connection check, per server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub imap: Option<Problem>,
    pub smtp: Option<Problem>,
    /// The text of an error that is not a [`Problem`] (an invalid setting).
    pub error: Option<String>,
    /// Folders the IMAP server listed (a sign the login works).
    pub folders: usize,
}

impl CheckResult {
    pub fn ok(&self) -> bool {
        self.imap.is_none() && self.smtp.is_none() && self.error.is_none()
    }
}

fn as_problem(e: Error) -> (Option<Problem>, Option<String>) {
    match e {
        Error::Connection(p) => (Some(p), None),
        Error::Network(t) | Error::Imap(t) => (Some(Problem::Protocol { detail: t }), None),
        other => (None, Some(other.to_string())),
    }
}

/// Signs in to the IMAP server (and lists the folders) and to the SMTP server, side by side.
pub async fn check_account(transport: &Transport, password: &str) -> CheckResult {
    check_account_with(transport, &Credential::Password(password.to_string())).await
}

/// [`check_account`] with a password or an OAuth access token.
pub async fn check_account_with(transport: &Transport, credential: &Credential) -> CheckResult {
    let Transport::Imap { imap, smtp, username, .. } = transport else {
        return CheckResult {
            imap: None,
            smtp: None,
            error: Some(
                tr!("Nur IMAP-Konten lassen sich so prüfen.", "Only IMAP accounts can be checked this way.").into(),
            ),
            folders: 0,
        };
    };
    let imap_check = async {
        let mut s = ImapSession::connect_with(imap, username, credential).await?;
        let n = s.list().await?.len();
        let _ = s.logout().await;
        Ok::<usize, Error>(n)
    };
    let smtp_check = smtp::check_with(smtp, username, credential);
    let (i, s) = tokio::join!(imap_check, smtp_check);
    let mut out = CheckResult { imap: None, smtp: None, error: None, folders: 0 };
    match i {
        Ok(n) => out.folders = n,
        Err(e) => (out.imap, out.error) = as_problem(e),
    }
    if let Err(e) = s {
        let (p, err) = as_problem(e);
        out.smtp = p;
        out.error = out.error.or(err);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{AccountDraft, Provider, preset};
    use crate::imap::fake::{FakeImap, SharedFake};
    use crate::parse::tests::sample;

    type Outgoing = Arc<Mutex<Vec<(String, Vec<String>, Vec<u8>)>>>;

    #[derive(Clone, Default)]
    struct Mailer {
        sent: Outgoing,
        refuse: Arc<Mutex<Option<Problem>>>,
        smtp_down: Arc<Mutex<bool>>,
        password: Arc<Mutex<String>>,
    }

    struct FakeConnector {
        server: SharedFake,
        mail: Mailer,
    }

    impl Connector for FakeConnector {
        type Imap = SharedFake;
        async fn imap(&self) -> Result<SharedFake> {
            if let Some(p) = self.mail.refuse.lock().unwrap().clone() {
                return Err(Error::Connection(p));
            }
            Ok(self.server.connection())
        }
        async fn send(&self, from: &str, to: &[String], raw: &[u8]) -> Result<()> {
            if *self.mail.smtp_down.lock().unwrap() {
                return Err(Error::Connection(Problem::Refused { host: "smtp".into(), port: 587 }));
            }
            self.mail.sent.lock().unwrap().push((from.into(), to.to_vec(), raw.to_vec()));
            Ok(())
        }
        fn files_sent(&self) -> bool {
            false
        }
        fn set_password(&self, password: String) {
            *self.mail.password.lock().unwrap() = password;
            *self.mail.refuse.lock().unwrap() = None;
        }
    }

    struct Setup {
        db: PathBuf,
        store: Store,
        account: i64,
        server: SharedFake,
        mail: Mailer,
        notices: Arc<Mutex<Vec<Notice>>>,
    }

    fn setup(name: &str) -> Setup {
        let dir = std::env::temp_dir().join(format!("mail-engine-{name}-{}-{}", std::process::id(), now()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("mail.db");
        let store = Store::open(&db).unwrap();
        let account = store
            .add_account(&AccountDraft {
                provider: Provider::Imap,
                name: String::new(),
                email: "maria@example.org".into(),
                sender_name: "Maria".into(),
                transport: preset(Provider::Imap, "maria@example.org").unwrap(),
            })
            .unwrap();
        let mut fake = FakeImap::default().with_all_folders();
        fake.deliver("INBOX", &sample("a@x", "Willkommen", None, "Hallo"), 1_760_000_000);
        Setup { db, store, account, server: SharedFake::new(fake), mail: Mailer::default(), notices: Arc::default() }
    }

    impl Setup {
        fn start(&self) -> Worker {
            let notices = self.notices.clone();
            let sink: Sink = Arc::new(move |n| notices.lock().unwrap().push(n));
            let c = FakeConnector { server: self.server.clone(), mail: self.mail.clone() };
            Worker::spawn(self.account, self.db.clone(), c, sink).unwrap()
        }

        async fn until(&self, what: &str, f: impl Fn(&Setup) -> bool) {
            let start = Instant::now();
            while !f(self) {
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "timed out: {what}; notices: {:?}",
                    self.notices.lock().unwrap()
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        fn inbox(&self) -> i64 {
            self.store.mailbox_by_role(self.account, Role::Inbox).unwrap().unwrap().id
        }

        fn status(&self) -> Option<Status> {
            self.notices.lock().unwrap().iter().rev().find_map(|n| match n {
                Notice::Status { status, .. } => Some(status.clone()),
                _ => None,
            })
        }
    }

    #[tokio::test]
    async fn a_background_check_syncs_the_inbox_once() {
        let s = setup("check-once");
        let connector = || FakeConnector { server: s.server.clone(), mail: s.mail.clone() };
        check_once(s.account, Store::open(&s.db).unwrap(), connector()).await.unwrap();
        assert_eq!(s.store.mailboxes(s.account).unwrap().len(), 6, "every folder the first time");
        assert_eq!(s.store.messages(s.inbox(), 10, 0).unwrap()[0].subject, "Willkommen");

        // New mail and a change made offline: the change goes out, the new message comes back.
        s.server.with(|f| f.deliver("INBOX", &sample("b@x", "Neu da", None, "x"), 1_760_000_100));
        outbox::enqueue(
            &s.store,
            s.account,
            &Op::Flags { mailbox: "INBOX".into(), uids: vec![1], seen: None, flagged: Some(true) },
        )
        .unwrap();
        let new = check_once(s.account, Store::open(&s.db).unwrap(), connector()).await.unwrap();
        assert_eq!(new.len(), 1, "{new:?}");
        assert_eq!(s.store.summaries(&new).unwrap()[0].subject, "Neu da");
        assert!(outbox::next_due(&s.store, s.account, now() + 60).unwrap().is_none(), "queue replayed");
        assert!(s.server.with(|f| f.mailboxes["INBOX"].messages[&1].flags.flagged));
        let again = check_once(s.account, Store::open(&s.db).unwrap(), connector()).await.unwrap();
        assert!(again.is_empty(), "nothing new: {again:?}");

        // A refused sign-in is the caller's to report.
        *s.mail.refuse.lock().unwrap() = Some(Problem::Auth { detail: "no".into() });
        assert!(check_once(s.account, Store::open(&s.db).unwrap(), connector()).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn syncs_waits_in_idle_and_tells_about_new_mail() {
        let s = setup("idle");
        let _w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        assert_eq!(s.store.mailboxes(s.account).unwrap().len(), 6);
        assert_eq!(s.store.messages(s.inbox(), 10, 0).unwrap()[0].subject, "Willkommen");
        s.server.with(|f| f.deliver("INBOX", &sample("b@x", "Neu da", None, "x"), 1_760_000_100));
        s.until("new mail", |s| {
            s.notices
                .lock()
                .unwrap()
                .iter()
                .any(|n| matches!(n, Notice::NewMail { messages, .. } if messages.len() == 1))
        })
        .await;
        assert_eq!(s.store.messages(s.inbox(), 10, 0).unwrap().len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn replays_changes_and_sends_with_a_copy_in_sent() {
        let s = setup("replay");
        let w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        let id = s.store.messages(s.inbox(), 10, 0).unwrap()[0].id;
        s.store.mark_seen(id, true).unwrap();
        outbox::enqueue(
            &s.store,
            s.account,
            &Op::Flags { mailbox: "INBOX".into(), uids: vec![1], seen: Some(true), flagged: Some(true) },
        )
        .unwrap();
        s.store.set_hidden(&[id], true).unwrap();
        outbox::enqueue(
            &s.store,
            s.account,
            &Op::Move { from: "INBOX".into(), to: "Archive".into(), uids: vec![1], ids: vec![id] },
        )
        .unwrap();
        let raw = sample("out@example.org", "Antwort", None, "Danke!");
        let b = base64::engine::general_purpose::STANDARD.encode(&raw);
        outbox::enqueue(
            &s.store,
            s.account,
            &Op::Send {
                raw_b64: b,
                from: "maria@example.org".into(),
                recipients: vec!["jonas@example.org".into()],
                sent_b64: None,
                sent_mailbox: Some("Sent".into()),
                draft: None,
                answered: None,
            },
        )
        .unwrap();
        w.send(Command::Kick);
        s.until("queue empty", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
        assert_eq!(s.server.with(|f| f.subjects("Archive")), ["Willkommen"]);
        assert!(s.server.with(|f| f.mailboxes["Archive"].messages[&1].flags.flagged), "flags before the move");
        assert_eq!(s.server.with(|f| f.subjects("Sent")), ["Antwort"]);
        assert_eq!(s.mail.sent.lock().unwrap()[0].1, ["jonas@example.org"]);
        s.until("archived copy synced", |s| {
            let a = s.store.mailbox_by_role(s.account, Role::Archive).unwrap().unwrap();
            s.store.messages(a.id, 10, 0).unwrap().len() == 1
        })
        .await;
        assert!(s.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::Sent { .. })));
    }

    /// Each queue notice comes after the queue changed: the UI reads the queue when it hears
    /// of it, and must not see a finished step still waiting.
    #[tokio::test(flavor = "multi_thread")]
    async fn queue_notices_follow_the_queue() {
        let s = setup("notices");
        let seen: Arc<Mutex<Vec<u32>>> = Arc::default();
        let (seen2, db) = (seen.clone(), s.db.clone());
        let notices = s.notices.clone();
        let sink: Sink = Arc::new(move |n| {
            if let Notice::Outbox { account_id } = &n {
                let store = Store::open(&db).unwrap();
                seen2.lock().unwrap().push(outbox::pending(&store, *account_id).unwrap());
            }
            notices.lock().unwrap().push(n);
        });
        let c = FakeConnector { server: s.server.clone(), mail: s.mail.clone() };
        let w = Worker::spawn(s.account, s.db.clone(), c, sink).unwrap();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        outbox::enqueue(&s.store, s.account, &Op::CreateFolder { name: "Projekte".into() }).unwrap();
        outbox::enqueue(&s.store, s.account, &Op::CreateFolder { name: "Kunden".into() }).unwrap();
        w.send(Command::Kick);
        s.until("replayed", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
        let start = Instant::now();
        while seen.lock().unwrap().last() != Some(&0) {
            assert!(start.elapsed() < Duration::from_secs(10), "notices saw {:?}", seen.lock().unwrap());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(*seen.lock().unwrap(), [1, 0], "one notice per step, after it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_changes_wait_and_replay_in_order_when_back() {
        let s = setup("offline");
        *s.mail.refuse.lock().unwrap() = Some(Problem::Refused { host: "imap".into(), port: 993 });
        let w = s.start();
        s.until("backoff", |s| matches!(s.status(), Some(Status::Retrying { .. }))).await;
        outbox::enqueue(&s.store, s.account, &Op::CreateFolder { name: "Projekte".into() }).unwrap();
        outbox::enqueue(&s.store, s.account, &Op::RenameFolder { from: "Projekte".into(), to: "Kunden".into() })
            .unwrap();
        w.send(Command::Kick);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(outbox::pending(&s.store, s.account).unwrap(), 2, "nothing while offline");
        *s.mail.refuse.lock().unwrap() = None;
        w.send(Command::SyncNow);
        s.until("replayed", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
        assert!(s.server.with(|f| f.mailboxes.contains_key("Kunden") && !f.mailboxes.contains_key("Projekte")));
        s.until("folder synced", |s| s.store.mailbox_by_name(s.account, "Kunden").unwrap().is_some()).await;
    }

    /// A reconnect request (sleep, network change) ends a long backoff at once.
    #[tokio::test(flavor = "multi_thread")]
    async fn reconnects_at_once_after_a_network_change() {
        let s = setup("reconnect");
        *s.mail.refuse.lock().unwrap() = Some(Problem::Refused { host: "imap".into(), port: 993 });
        let w = s.start();
        s.until("backoff", |s| matches!(s.status(), Some(Status::Retrying { .. }))).await;
        outbox::enqueue(&s.store, s.account, &Op::CreateFolder { name: "Projekte".into() }).unwrap();
        *s.mail.refuse.lock().unwrap() = None;
        let start = Instant::now();
        w.send(Command::Reconnect);
        s.until("replayed", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
        assert!(start.elapsed() < Duration::from_secs(4), "not after the backoff");
        s.until("ready", |s| s.status() == Some(Status::Ready)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refused_steps_are_parked_and_smtp_failures_wait() {
        let s = setup("refused");
        let w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        *s.mail.smtp_down.lock().unwrap() = true;
        outbox::enqueue(&s.store, s.account, &Op::DeleteFolder { name: "Gibt es nicht".into() }).unwrap();
        let raw = base64::engine::general_purpose::STANDARD.encode(sample("o@x", "Später", None, "x"));
        let send = Op::Send {
            raw_b64: raw,
            from: "maria@example.org".into(),
            recipients: vec!["jonas@example.org".into()],
            sent_b64: None,
            sent_mailbox: None,
            draft: None,
            answered: None,
        };
        outbox::enqueue(&s.store, s.account, &send).unwrap();
        w.send(Command::Kick);
        s.until("parked and waiting", |s| {
            let e = outbox::entries(&s.store).unwrap();
            e.len() == 2 && e[0].state == "parked" && e[1].state == "failed"
        })
        .await;
        assert_eq!(s.status(), Some(Status::Ready), "the IMAP connection stays");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_login_waits_for_the_password() {
        let s = setup("signin");
        *s.mail.refuse.lock().unwrap() = Some(Problem::Auth { detail: "wrong".into() });
        let w = s.start();
        s.until("sign in", |s| s.status() == Some(Status::SignIn)).await;
        w.send(Command::SignedIn("neu".into()));
        s.until("ready", |s| s.status() == Some(Status::Ready)).await;
        assert_eq!(*s.mail.password.lock().unwrap(), "neu");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bodies_attachments_and_search_on_demand() {
        let s = setup("demand");
        let w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        let id = s.store.messages(s.inbox(), 10, 0).unwrap()[0].id;
        w.fetch_body(id).await.unwrap();
        assert!(s.store.message(id).unwrap().has_body);
        let hits = w.server_search(vec![s.inbox()], "hallo".into()).await.unwrap();
        assert_eq!(hits, [id]);
        drop(w);
        s.until("stopped", |s| s.status() == Some(Status::Stopped)).await;
    }
}
