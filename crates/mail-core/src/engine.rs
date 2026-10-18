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
use crate::outbox::{self, Op, Queued};
use crate::store::{Store, now};
use crate::sync::{
    Action, BACKFILL_PER_ROUND, Event, FOLDER_FIRST_HEADERS, Gate, IDLE_RENEW, POLL_INTERVAL, State, SyncMachine,
    SyncOptions, backfill, fetch_attachment, fetch_body, server_search, sync_folders, sync_mailbox_with,
};
use crate::{autoconfig, smtp, tr, trf};

/// How a worker reaches the account's servers.
pub trait Connector: Send + 'static {
    type Imap: ImapClient + 'static;
    /// A new, signed-in IMAP connection.
    fn imap(&self) -> impl Future<Output = Result<Self::Imap>>;
    /// The connection for what the user waits for (bodies, attachments, search) next to the
    /// worker's own.
    fn imap_on_demand(&self) -> impl Future<Output = Result<Self::Imap>> {
        self.imap()
    }
    /// How long one try of an on-demand request may take before its connection counts as dead
    /// (a phone that changed networks keeps a socket that never answers) and a new one tries.
    fn on_demand_timeout(&self) -> Duration {
        ON_DEMAND_TIMEOUT
    }
    /// Sends a finished message.
    fn send(&self, from: &str, to: &[String], raw: &[u8]) -> impl Future<Output = Result<()>>;
    /// Whether the server files sent mail in Sent itself (Gmail, Microsoft).
    fn files_sent(&self) -> bool;
    /// The user signed in again.
    fn set_password(&self, password: String);
}

/// See [`Connector::on_demand_timeout`].
pub const ON_DEMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// An on-demand connection unused this long is made again before the next request: after a
/// while on a phone it is more likely dead than alive.
pub const ON_DEMAND_FRESH: Duration = Duration::from_secs(120);

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
    /// Held while the on-demand connection serves the user: the syncs wait at their next step.
    user_gate: Rc<Gate>,
    /// Held while the inbox's round runs: the background waits at its next step.
    inbox_gate: Gate,
    /// Counts the reconnects (sleep, a network change): the on-demand connection of an older
    /// one is dropped.
    network: Rc<Cell<u32>>,
    /// The worker stops: the background ends at its next step.
    stopping: Rc<Cell<bool>>,
    /// A full round asks the background for the other folders.
    background_full: Cell<bool>,
    /// The background fetches bodies ahead.
    background_bodies: Cell<bool>,
    background_wake: Notify,
    /// The steps of the last full sync, for „Sync-Details“.
    sync_log: RefCell<Rc<crate::diag::SyncLog>>,
    /// Folders changed since the last [`Notice::Changed`] (empty: all), and when it went out.
    pending: RefCell<(Option<Vec<i64>>, Option<Instant>)>,
}

/// [`Notice::Changed`] goes out at most this often while a sync stores part after part: the
/// list reloads once per step, not once per hundred messages of each folder.
pub const CHANGED_EVERY: Duration = Duration::from_millis(300);

impl<C: Connector> Ctx<C> {
    fn new(account_id: i64, store: Store, connector: C, sink: Sink) -> Self {
        Ctx {
            account_id,
            store: Rc::new(store),
            connector: Rc::new(connector),
            sink,
            user_gate: Rc::default(),
            inbox_gate: Gate::default(),
            network: Rc::default(),
            stopping: Rc::default(),
            background_full: Cell::new(false),
            background_bodies: Cell::new(false),
            background_wake: Notify::new(),
            sync_log: RefCell::default(),
            pending: RefCell::new((None, None)),
        }
    }

    fn notice(&self, n: Notice) {
        (self.sink)(n)
    }

    /// Notes changed folders; tells at most every [`CHANGED_EVERY`] (the rest with [`Self::flush`]).
    fn changed(&self, mailboxes: Vec<i64>) {
        let due = {
            let mut p = self.pending.borrow_mut();
            p.0 = Some(match p.0.take() {
                None => mailboxes,
                Some(old) if old.is_empty() || mailboxes.is_empty() => vec![],
                Some(mut old) => {
                    old.extend(mailboxes.into_iter().filter(|m| !old.contains(m)).collect::<Vec<_>>());
                    old
                }
            });
            p.1.is_none_or(|at| at.elapsed() >= CHANGED_EVERY)
        };
        if due {
            self.flush();
        }
    }

    /// Tells about the changed folders noted so far.
    fn flush(&self) {
        let waiting = {
            let mut p = self.pending.borrow_mut();
            let w = p.0.take();
            if w.is_some() {
                p.1 = Some(Instant::now());
            }
            w
        };
        if let Some(mailboxes) = waiting {
            self.notice(Notice::Changed { account_id: self.account_id, mailboxes });
        }
    }

    fn log(&self) -> Rc<crate::diag::SyncLog> {
        self.sync_log.borrow().clone()
    }

    /// Starts the steps of a new sync for „Sync-Details“.
    fn new_log(&self) {
        *self.sync_log.borrow_mut() = Rc::default();
    }

    fn publish_log(&self) {
        crate::diag::set_sync_details(self.account_id, self.log().text());
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

/// Selects the folder of a step's UIDs; refused when the folder is not the one the UIDs were
/// taken from (its UIDVALIDITY changed: the server rebuilt it, and the same numbers may name
/// other messages now).
async fn select_for<I: ImapClient>(conn: &mut I, mailbox: &str, uid_validity: Option<u32>) -> Result<()> {
    let sel = conn.select(mailbox, None).await?;
    match uid_validity {
        Some(v) if v != sel.info.uid_validity => Err(Error::Invalid(trf!(
            "Der Ordner „{mailbox}“ wurde auf dem Server neu aufgebaut, seit du das getan hast; die Nachrichten haben neue Nummern. Damit keine falsche Nachricht betroffen ist, wartet diese Änderung: Verwirf sie und mach sie noch einmal.",
            "The folder “{mailbox}” was rebuilt on the server since you did this; its messages have new numbers. So that no wrong message is affected, this change waits: discard it and do it again."
        ))),
        _ => Ok(()),
    }
}

/// Whether the server filed a message with this Message-ID in Sent already (it went out on
/// an earlier attempt whose answer was lost).
async fn already_in_sent<I: ImapClient>(conn: &mut I, sent: &str, raw: &[u8]) -> Result<bool> {
    let Some(mid) = crate::parse::parse(raw).ok().and_then(|p| p.message_id) else { return Ok(false) };
    conn.select(sent, None).await?;
    Ok(!conn.search(&Search::MessageId(mid)).await?.is_empty())
}

/// After a message (and its Bcc copies) went out: the notice, then filing it in Sent as a step
/// that never sends again.
async fn sent_and_filed<C: Connector>(
    ctx: &Ctx<C>,
    conn: &mut C::Imap,
    q: &Queued,
    sent: Option<String>,
    sent_mailbox: &Option<String>,
    draft: &Option<String>,
    answered: &Option<(String, u32)>,
) -> Result<Vec<String>> {
    ctx.notice(Notice::Sent { account_id: ctx.account_id, outbox_id: q.id });
    let rest = Op::AppendSent {
        mailbox: if ctx.connector.files_sent() { None } else { sent_mailbox.clone() },
        raw_b64: sent,
        draft: draft.clone(),
        answered: answered.clone(),
    };
    outbox::replace(&ctx.store, q.id, &rest)?;
    let next = Queued { op: rest, ..q.clone() };
    let mut touched = Box::pin(apply(ctx, conn, &next)).await?;
    // The server filed it (Gmail, Microsoft, JMAP): Sent is synced right away.
    if ctx.connector.files_sent()
        && let Some(sent) = sent_mailbox
    {
        touched.push(sent.clone());
    }
    Ok(touched)
}

/// Carries out one step of the queue; returns the folders it touched.
async fn apply<C: Connector>(ctx: &Ctx<C>, conn: &mut C::Imap, q: &Queued) -> Result<Vec<String>> {
    let (id, op) = (q.id, &q.op);
    match op {
        Op::Send { raw_b64, from, recipients, sent_b64, sent_mailbox, draft, answered, copies } => {
            let raw = b64(raw_b64)?;
            // A server that files sent mail itself shows whether an earlier attempt went out.
            let filed = async |conn: &mut C::Imap| match sent_mailbox {
                Some(sent) if ctx.connector.files_sent() => match already_in_sent(conn, sent, &raw).await {
                    Ok(found) => Ok(found),
                    Err(e) if is_transient(&e) => Err(e),
                    Err(_) => Ok(false),
                },
                _ => Ok(false),
            };
            if q.attempts == 0 || !filed(conn).await? {
                match ctx.connector.send(from, recipients, &raw).await {
                    Ok(()) => {}
                    // The answer to the message went missing: the server's Sent says whether it
                    // went out; else the user decides (never sent twice by itself).
                    Err(Error::Unconfirmed(e)) => {
                        if !filed(conn).await.unwrap_or(false) {
                            return Err(Error::Unconfirmed(e));
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            let sent = sent_b64.clone().or_else(|| Some(raw_b64.clone()));
            if let Some((next, rest)) = copies.split_first() {
                // The Bcc copies each wait as a step of their own: a retry never sends one twice.
                let more = Op::SendCopy {
                    from: from.clone(),
                    copy: next.clone(),
                    rest: rest.to_vec(),
                    sent_b64: sent,
                    sent_mailbox: sent_mailbox.clone(),
                    draft: draft.clone(),
                    answered: answered.clone(),
                };
                outbox::replace(&ctx.store, id, &more)?;
                let next = Queued { op: more, attempts: 0, last_error: None, ..q.clone() };
                return Box::pin(apply(ctx, conn, &next)).await;
            }
            sent_and_filed(ctx, conn, q, sent, sent_mailbox, draft, answered).await
        }
        Op::SendCopy { from, copy, rest, sent_b64, sent_mailbox, draft, answered } => {
            // A copy has the message's Message-ID, so Sent cannot tell whether it went out: an
            // unconfirmed one waits for the user like the message itself.
            ctx.connector.send(from, &copy.recipients, &b64(&copy.raw_b64)?).await?;
            if let Some((next, more)) = rest.split_first() {
                let op = Op::SendCopy {
                    from: from.clone(),
                    copy: next.clone(),
                    rest: more.to_vec(),
                    sent_b64: sent_b64.clone(),
                    sent_mailbox: sent_mailbox.clone(),
                    draft: draft.clone(),
                    answered: answered.clone(),
                };
                outbox::replace(&ctx.store, id, &op)?;
                let next = Queued { op, attempts: 0, last_error: None, ..q.clone() };
                return Box::pin(apply(ctx, conn, &next)).await;
            }
            sent_and_filed(ctx, conn, q, sent_b64.clone(), sent_mailbox, draft, answered).await
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
                match select_for(conn, mb, q.uid_validity).await {
                    Ok(()) => {
                        conn.store(&[*uid], &[ANSWERED], true).await?;
                        touched.push(mb.clone());
                    }
                    Err(e) if is_transient(&e) => return Err(e),
                    // The folder is gone or was rebuilt: the UID may name another message now,
                    // so the mark stays off.
                    Err(_) => {}
                }
            }
            Ok(touched)
        }
        Op::Flags { mailbox, uids, seen, flagged } => {
            select_for(conn, mailbox, q.uid_validity).await?;
            if let Some(s) = seen {
                conn.store(uids, &[SEEN], *s).await?;
            }
            if let Some(f) = flagged {
                conn.store(uids, &[FLAGGED], *f).await?;
            }
            Ok(vec![mailbox.clone()])
        }
        Op::Keyword { mailbox, uids, keyword, add } => {
            select_for(conn, mailbox, q.uid_validity).await?;
            conn.store(uids, &[keyword.as_str()], *add).await?;
            Ok(vec![mailbox.clone()])
        }
        Op::Move { from, to, uids, ids } => {
            select_for(conn, from, q.uid_validity).await?;
            conn.move_to(uids, to).await?;
            ctx.store.remove_messages(ids)?;
            Ok(vec![from.clone(), to.clone()])
        }
        Op::Delete { mailbox, uids, ids } => {
            select_for(conn, mailbox, q.uid_validity).await?;
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

/// Runs a step while renewing its claim, so no other worker takes it meanwhile.
async fn apply_claimed<C: Connector>(ctx: &Ctx<C>, conn: &mut C::Imap, q: &Queued) -> Result<Vec<String>> {
    let work = apply(ctx, conn, q);
    tokio::pin!(work);
    loop {
        tokio::select! {
            r = &mut work => return r,
            _ = tokio::time::sleep(outbox::LEASE_RENEW) => {
                let _ = outbox::renew(&ctx.store, q.id);
            }
        }
    }
}

/// Replays the due steps of the queue, each lane in order. A connection failure stops the
/// replay (the caller reconnects); a refusal parks the step and goes on; a message the SMTP
/// server did not take waits without holding up the rest.
async fn replay<C: Connector>(ctx: &Ctx<C>, conn: &mut C::Imap) -> Result<Vec<String>> {
    let mut touched = vec![];
    while let Some(q) = outbox::next_due(&ctx.store, ctx.account_id, now())? {
        // Taken back a moment ago, or another worker of the account runs it.
        if !outbox::start(&ctx.store, q.id)? {
            continue;
        }
        let result = apply_claimed(ctx, conn, &q).await;
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
            // A send that could not reach the SMTP server: the IMAP connection is fine, and
            // the other steps go on.
            if matches!(q.op, Op::Send { .. } | Op::SendCopy { .. }) {
                continue;
            }
            return Err(e);
        }
    }
    Ok(touched)
}

/// Syncs one folder and tells about it; for the inbox also the Screener, the rules, the
/// priority and the new mail notice. A folder the server refuses waits for its next try.
async fn sync_folder<C: Connector>(
    ctx: &Ctx<C>,
    conn: &mut C::Imap,
    caps: Capabilities,
    id: i64,
    settings: &crate::settings::Settings,
    opts: &SyncOptions<'_>,
    touched: &[String],
) -> Result<()> {
    let inbox = ctx.inbox().map(|m| m.id);
    let r = match sync_mailbox_with(conn, &ctx.store, caps, id, opts).await {
        Ok(r) => {
            ctx.store.set_folder_error(id, None)?;
            r
        }
        Err(e) if is_transient(&e) => return Err(e),
        // The server refused this folder (no read right, gone since the LIST): the others
        // go on, the folder shows why it is not up to date.
        Err(e) => {
            crate::diag::log(&format!("account-{}", ctx.account_id), &format!("folder {id} skipped: {e}"));
            ctx.store.set_folder_error(id, Some(&e.to_string()))?;
            ctx.changed(vec![]);
            return Ok(());
        }
    };
    if r.changed() || touched.iter().any(|n| ctx.mailbox_named(n) == Some(id)) {
        ctx.changed(vec![id]);
    }
    let mut screened = vec![];
    if Some(id) == inbox && r.added > 0 {
        // Mail of senders the user turned away (the Screener) leaves the inbox at once.
        let moved = crate::triage::move_blocked(&ctx.store, Some(ctx.account_id))?;
        if !moved.hidden.is_empty() {
            ctx.changed(vec![id]);
        }
        screened = moved.hidden;
    }
    // What the Screener moved away is neither news nor the rules' to handle.
    let new: Vec<i64> = r.new.iter().copied().filter(|m| !screened.contains(m)).collect();
    let mut new_unread = r.new_unread;
    new_unread.retain(|m| !screened.contains(m));
    if Some(id) == inbox && !new.is_empty() {
        // The local rules first: what they move away or mark read is no news.
        match crate::rules::run_new(&ctx.store, ctx.account_id, &new) {
            Ok(ran) if ran.matched > 0 => {
                new_unread.retain(|m| !ran.removed.contains(m));
                let still_unread: Vec<i64> =
                    ctx.store.summaries(&new_unread)?.into_iter().filter(|s| !s.seen).map(|s| s.id).collect();
                new_unread = still_unread;
                ctx.changed(vec![id]);
            }
            Ok(_) => {}
            Err(e) => crate::diag::log("rules", &format!("account {}: {e}", ctx.account_id)),
        }
        if settings.ai.on()
            && settings.ai.priority
            && let Err(e) = crate::ai::priority::mark_new(&ctx.store, &new)
        {
            crate::diag::log("priority", &e.to_string());
        }
    }
    if Some(id) == inbox && !new_unread.is_empty() {
        ctx.flush();
        ctx.notice(Notice::NewMail { account_id: ctx.account_id, messages: new_unread });
    }
    Ok(())
}

/// Folders the server would not open lately wait for their next try.
fn due(ctx: &Ctx<impl Connector>, id: i64) -> bool {
    let at = now();
    ctx.store.mailbox(id).is_ok_and(|m| m.selectable && m.sync_retry_at.is_none_or(|t| t <= at))
}

/// Holds a [`Gate`] while it lives.
struct Held<'a>(&'a Gate);

impl<'a> Held<'a> {
    fn new(g: &'a Gate) -> Self {
        g.enter();
        Held(g)
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.leave();
    }
}

/// The inbox's round on the worker's own connection: the queue, the folder list (`full`, or
/// when the account has no inbox yet), the inbox and the folders the queue touched. What else
/// there is to do (the other folders, bodies ahead, older mail) is the background's
/// ([`background`]), so nothing of it delays the inbox; it waits while this runs.
async fn sync_round<C: Connector>(ctx: &Ctx<C>, conn: &mut C::Imap, caps: Capabilities, full: bool) -> Result<()> {
    let _inbox_first = Held::new(&ctx.inbox_gate);
    // The settings may have changed since the last round.
    let settings = crate::settings::load(&ctx.store).unwrap_or_default();
    ctx.store.set_subject_threads(settings.subject_threads);
    let log = ctx.log();
    // Each stored part shows at once: the newest mail within seconds.
    let shown = |mailbox: i64| ctx.changed(vec![mailbox]);
    let gates: [&Gate; 1] = [&ctx.user_gate];
    let opts = SyncOptions {
        first_headers: settings.initial_headers,
        on_chunk: Some(&shown),
        gates: &gates,
        log: Some(&*log),
    };
    let t = Instant::now();
    let touched = replay(ctx, conn).await?;
    if !touched.is_empty() {
        log.note(tr!("Warteschlange", "queue"), t.elapsed());
    }
    if full || ctx.inbox().is_none() {
        let t = Instant::now();
        sync_folders(conn, &ctx.store, ctx.account_id).await?;
        log.note(
            &format!("LIST ({} {})", ctx.store.mailboxes(ctx.account_id)?.len(), tr!("Ordner", "folders")),
            t.elapsed(),
        );
        ctx.changed(vec![]);
    }
    let mut ids: Vec<i64> = ctx.inbox().map(|m| m.id).into_iter().collect();
    ids.extend(touched.iter().filter_map(|n| ctx.mailbox_named(n)));
    ids.dedup();
    ids.retain(|id| due(ctx, *id));
    for id in ids {
        sync_folder(ctx, conn, caps, id, &settings, &opts, &touched).await?;
    }
    ctx.flush();
    ctx.publish_log();
    Ok(())
}

/// The other folders of a full round on `conn`: the labels and folders first, Gmail's All
/// Mail, Starred and Important last (their messages find the bodies of the copies synced
/// before). Their first sync fetches fewer headers than the inbox's.
async fn other_folders<C: Connector>(
    ctx: &Ctx<C>,
    conn: &mut C::Imap,
    caps: Capabilities,
    settings: &crate::settings::Settings,
    opts: &SyncOptions<'_>,
) -> Result<()> {
    let inbox = ctx.inbox().map(|m| m.id);
    let mut list: Vec<_> = ctx.store.mailboxes(ctx.account_id)?.into_iter().filter(|m| Some(m.id) != inbox).collect();
    list.sort_by_key(|m| m.role.is_some_and(Role::is_virtual));
    let opts = SyncOptions { first_headers: settings.initial_headers.min(FOLDER_FIRST_HEADERS), ..*opts };
    for m in list {
        if ctx.stopping.get() {
            break;
        }
        if due(ctx, m.id) {
            sync_folder(ctx, conn, caps, m.id, settings, &opts, &[]).await?;
            ctx.flush();
        }
    }
    Ok(())
}

/// Bodies fetched ahead (the first lines, search, reading offline): the inbox first, then the
/// other folders but Gmail's All Mail and the like (their copies get the bodies of the others).
async fn bodies_ahead<C: Connector>(
    ctx: &Ctx<C>,
    conn: &mut C::Imap,
    settings: &crate::settings::Settings,
    gates: &[&Gate],
    inbox_only: bool,
) -> Result<()> {
    let inbox = ctx.inbox().map(|m| m.id);
    let mut list: Vec<_> = ctx
        .store
        .mailboxes(ctx.account_id)?
        .into_iter()
        .filter(|m| m.selectable && !m.role.is_some_and(Role::is_virtual))
        .filter(|m| (Some(m.id) == inbox) == inbox_only)
        .collect();
    list.sort_by_key(|m| Some(m.id) != inbox);
    for m in list {
        if ctx.stopping.get() || !due(ctx, m.id) {
            continue;
        }
        let t = Instant::now();
        match crate::sync::prefetch_bodies_with(conn, &ctx.store, m.id, settings.prefetch, gates).await {
            Ok(n) if n > 0 => {
                ctx.log()
                    .note(&format!("{} {}: {n}", tr!("Inhalte", "bodies"), crate::diag::folder_label(&m)), t.elapsed());
                ctx.changed(vec![m.id]);
            }
            Ok(_) => {}
            Err(e) if is_transient(&e) => return Err(e),
            Err(e) => crate::diag::log(&format!("account-{}", ctx.account_id), &format!("folder {}: {e}", m.id)),
        }
    }
    ctx.flush();
    Ok(())
}

/// One part of the older mail that waits ([`BACKFILL_PER_ROUND`] headers), of the inbox
/// (`inbox_only`) or of any folder: the inbox first, Gmail's All Mail and the like last.
/// Returns whether more of it waits.
async fn backfill_part<C: Connector>(
    ctx: &Ctx<C>,
    conn: &mut C::Imap,
    opts: &SyncOptions<'_>,
    inbox_only: bool,
) -> Result<bool> {
    let mut order = backfill_folders(ctx)?;
    if inbox_only {
        order.retain(|m| m.role == Some(Role::Inbox));
    }
    if order.is_empty() {
        return Ok(false);
    }
    order.sort_by_key(|m| (m.role != Some(Role::Inbox), m.role.is_some_and(Role::is_virtual)));
    let m = &order[0];
    let t = Instant::now();
    match backfill(conn, &ctx.store, m.id, BACKFILL_PER_ROUND, opts).await {
        Ok(n) => ctx
            .log()
            .note(&format!("{} {}: {n}", tr!("Älteres", "older mail"), crate::diag::folder_label(m)), t.elapsed()),
        Err(e) if is_transient(&e) => return Err(e),
        Err(e) => {
            crate::diag::log(&format!("account-{}", ctx.account_id), &format!("folder {}: {e}", m.id));
            // The server refuses the folder: it waits for its next try, as in a sync.
            ctx.store.set_folder_error(m.id, Some(&e.to_string()))?;
        }
    }
    ctx.flush();
    ctx.publish_log();
    Ok(backfill_folders(ctx)?.iter().any(|f| !inbox_only || f.role == Some(Role::Inbox)))
}

/// The folders whose older mail waits and that the server opens (or whose next try is due).
fn backfill_folders<C: Connector>(ctx: &Ctx<C>) -> Result<Vec<crate::store::Mailbox>> {
    let pending = ctx.store.backfill_pending(ctx.account_id)?;
    if pending.is_empty() {
        return Ok(vec![]);
    }
    let at = now();
    Ok(ctx
        .store
        .mailboxes(ctx.account_id)?
        .into_iter()
        .filter(|m| pending.contains(&m.id) && m.sync_retry_at.is_none_or(|t| t <= at))
        .collect())
}

/// The background's work on a connection of its own, next to the inbox's: after each full
/// round the other folders, then the bodies ahead, then the older mail a part at a time. It
/// steps aside while the inbox's round runs and while the user waits for something
/// ([`Gate`]s), and starts over when a new full round asks for it.
async fn background<C: Connector>(ctx: Rc<Ctx<C>>) {
    let mut conn: Option<C::Imap> = None;
    let mut caps = Capabilities::default();
    let mut failures = 0u32;
    loop {
        if ctx.stopping.get() {
            break;
        }
        let full = ctx.background_full.replace(false);
        let older = !backfill_folders(&ctx).map(|f| f.is_empty()).unwrap_or(true);
        if !full && !older && !ctx.background_bodies.get() {
            // Woken by the next full round (or to stop).
            ctx.background_wake.notified().await;
            continue;
        }
        let work = async {
            if conn.is_none() {
                let t = Instant::now();
                let mut c = ctx.connector.imap().await?;
                caps = c.capabilities().await.unwrap_or_default();
                ctx.log().note(tr!("Verbindung im Hintergrund", "background connection"), t.elapsed());
                conn = Some(c);
            }
            let c = conn.as_mut().expect("just connected");
            let settings = crate::settings::load(&ctx.store).unwrap_or_default();
            let log = ctx.log();
            let gates: [&Gate; 2] = [&ctx.user_gate, &ctx.inbox_gate];
            let shown = |mailbox: i64| ctx.changed(vec![mailbox]);
            let opts = SyncOptions {
                first_headers: settings.initial_headers,
                on_chunk: Some(&shown),
                gates: &gates,
                log: Some(&*log),
            };
            // The inbox's bodies first: what was just listed opens at once, also offline.
            let bodies = ctx.background_bodies.replace(false) || full;
            if bodies {
                bodies_ahead(&ctx, c, &settings, &gates, true).await?;
                ctx.publish_log();
            }
            if full {
                ctx.inbox_gate.pass().await;
                other_folders(&ctx, c, caps, &settings, &opts).await?;
                ctx.publish_log();
            }
            if bodies {
                // The inbox's older mail before the other folders' bodies.
                while !ctx.stopping.get() && !ctx.background_full.get() {
                    if !backfill_part(&ctx, c, &opts, true).await? {
                        break;
                    }
                }
                bodies_ahead(&ctx, c, &settings, &gates, false).await?;
                ctx.publish_log();
            }
            // The older mail, a part at a time, until a full round asks for the folders again.
            while !ctx.stopping.get() && !ctx.background_full.get() {
                if !backfill_part(&ctx, c, &opts, false).await? {
                    break;
                }
            }
            Ok::<(), Error>(())
        };
        match work.await {
            Ok(()) => failures = 0,
            Err(e) => {
                crate::diag::log(&format!("account-{}", ctx.account_id), &format!("background: {e}"));
                conn = None;
                failures += 1;
                if !is_transient(&e) {
                    // A refusal of the server's: the next full round tries again.
                    ctx.background_full.set(false);
                    ctx.background_bodies.set(false);
                    if backfill_waits(&ctx) {
                        tokio::select! {
                            _ = ctx.background_wake.notified() => {}
                            _ = tokio::time::sleep(POLL_INTERVAL) => {}
                        }
                    }
                    continue;
                }
                // What the broken connection did not finish is done again on the next.
                ctx.background_full.set(ctx.background_full.get() || full);
                ctx.background_bodies.set(true);
                tokio::select! {
                    _ = ctx.background_wake.notified() => {}
                    _ = tokio::time::sleep(crate::sync::reconnect_delay(failures)) => {}
                }
            }
        }
    }
    if let Some(mut c) = conn {
        let _ = c.logout().await;
    }
}

/// Whether older mail of the account still waits.
fn backfill_waits<C: Connector>(ctx: &Ctx<C>) -> bool {
    backfill_folders(ctx).is_ok_and(|f| !f.is_empty())
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
    let ctx = Rc::new(Ctx::new(account_id, store, connector, sink));
    let wake = Rc::new(Notify::new());
    let queue: Rc<RefCell<VecDeque<Command>>> = Rc::default();
    let stop = ctx.stopping.clone();
    let (itx, irx) = mpsc::unbounded_channel::<Command>();
    {
        let (wake, queue, stop) = (wake.clone(), queue.clone(), stop.clone());
        let background = ctx.clone();
        let (gate, network) = (ctx.user_gate.clone(), ctx.network.clone());
        tokio::task::spawn_local(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Command::FetchBody { .. } | Command::FetchAttachment { .. } | Command::ServerSearch { .. } => {
                        // The sync steps aside until the user has it (`on_demand` leaves).
                        gate.enter();
                        if itx.send(cmd).is_err() {
                            gate.leave();
                        }
                    }
                    Command::Reconnect | Command::NetworkDown => {
                        network.set(network.get() + 1);
                        queue.borrow_mut().push_back(cmd);
                        wake.notify_one();
                    }
                    Command::Stop => break,
                    other => {
                        queue.borrow_mut().push_back(other);
                        wake.notify_one();
                    }
                }
            }
            stop.set(true);
            background.background_wake.notify_one();
            queue.borrow_mut().push_back(Command::Stop);
            wake.notify_one();
        });
    }
    tokio::task::spawn_local(on_demand(ctx.clone(), irx));
    tokio::task::spawn_local(background(ctx.clone()));
    main_loop(&ctx, &wake, &queue, &stop).await;
}

/// One check without a worker, for the phones' background check (WorkManager on Android,
/// background app refresh on iOS), where no connection may stay open: connect, replay the
/// offline queue, sync the inbox (every folder when the account has none yet) and fetch its
/// first lines, log out. Returns the new unread messages of the inbox. The caller bounds the
/// time it may take.
pub async fn check_once<C: Connector>(account_id: i64, store: Store, connector: C) -> Result<Vec<i64>> {
    let found: Arc<Mutex<Vec<i64>>> = Arc::default();
    let into = found.clone();
    let sink: Sink = Arc::new(move |n| {
        if let Notice::NewMail { messages, .. } = n {
            into.lock().unwrap_or_else(|e| e.into_inner()).extend(messages);
        }
    });
    let ctx = Ctx::new(account_id, store, connector, sink);
    let mut conn = ctx.connector.imap().await?;
    let caps = conn.capabilities().await.unwrap_or_default();
    let full = ctx.inbox().is_none();
    // The other folders only the first time; their bodies and the older mail of a large folder
    // wait for the app: a background check stays short.
    let mut r = sync_round(&ctx, &mut conn, caps, full).await;
    if r.is_ok() && full {
        let settings = crate::settings::load(&ctx.store).unwrap_or_default();
        r = other_folders(&ctx, &mut conn, caps, &settings, &SyncOptions::default()).await;
    }
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
    // The log of the next full sync was started by connecting.
    let mut fresh_log = false;
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
                ctx.new_log();
                fresh_log = true;
                let t = Instant::now();
                match ctx.connector.imap().await {
                    Ok(mut c) => {
                        let took = c.connect_times();
                        if took.is_empty() {
                            ctx.log().note(tr!("Verbinden und Anmelden", "connect and sign in"), t.elapsed());
                        }
                        for (what, d) in took {
                            ctx.log().note(what, d);
                        }
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
                    if !std::mem::take(&mut fresh_log) {
                        ctx.new_log();
                    }
                }
                match sync_round(ctx, c, caps, full).await {
                    Ok(()) => {
                        if full {
                            last_full = Some(Instant::now());
                            force_full.set(false);
                            ctx.background_full.set(true);
                        }
                        // The other folders, the bodies ahead and the older mail: the background's.
                        ctx.background_bodies.set(true);
                        ctx.background_wake.notify_one();
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
                let known = ctx.inbox();
                let inbox = known.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "INBOX".into());
                let r = match c.select(&inbox, None).await {
                    // Mail that arrived during the round (after its fetch, before this SELECT)
                    // is not news to IDLE: sync again right away.
                    Ok(sel) if known.and_then(|m| m.sync.uid_next).is_some_and(|n| sel.info.uid_next > n) => {
                        Ok(IdleOutcome::Changed)
                    }
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
                // Without IDLE: wake for the next full sync and the next due step of the queue
                // (an undo window that ends), as the Idle branch does.
                let d = if m.state == State::Polling {
                    let mut d = d.min(POLL_INTERVAL.saturating_sub(last_full.map_or(Duration::ZERO, |t| t.elapsed())));
                    if let Ok(Some(due)) = outbox::next_due_at(&ctx.store, ctx.account_id) {
                        d = d.min(Duration::from_secs((due - now()).max(0) as u64 + 1));
                    }
                    d.max(Duration::from_secs(1))
                } else {
                    d
                };
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
/// the user does not wait for a long sync. While it works the sync steps aside ([`Gate`]). A
/// connection from before a network change or unused for [`ON_DEMAND_FRESH`] is made again; a
/// body that takes longer than [`Connector::on_demand_timeout`] is tried once more on a new one.
async fn on_demand<C: Connector>(ctx: Rc<Ctx<C>>, mut rx: mpsc::UnboundedReceiver<Command>) {
    let mut conn: Option<C::Imap> = None;
    // Ready before the first message is opened (connecting and signing in take the longest).
    if connected(&ctx, &mut conn).await.is_err() {
        conn = None;
    }
    let mut last_used = Instant::now();
    let mut network = ctx.network.get();
    while let Some(cmd) = rx.recv().await {
        if network != ctx.network.get() || last_used.elapsed() >= ON_DEMAND_FRESH {
            // Likely dead: not even a LOGOUT, which would wait for a socket that never answers.
            conn = None;
            network = ctx.network.get();
        }
        match cmd {
            Command::FetchBody { message_id, reply } => {
                let mut result = Err(WorkerHandle::gone());
                for _ in 0..2 {
                    let attempt = async {
                        let c = connected(&ctx, &mut conn).await?;
                        fetch_body(c, &ctx.store, message_id).await
                    };
                    result = match tokio::time::timeout(ctx.connector.on_demand_timeout(), attempt).await {
                        Ok(r) => r,
                        Err(_) => Err(Error::Connection(Problem::Timeout { host: String::new(), port: 0 })),
                    };
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
        last_used = Instant::now();
        ctx.user_gate.leave();
    }
    if let Some(mut c) = conn {
        let _ = c.logout().await;
    }
}

async fn connected<'a, C: Connector>(ctx: &Ctx<C>, conn: &'a mut Option<C::Imap>) -> Result<&'a mut C::Imap> {
    if conn.is_none() {
        *conn = Some(ctx.connector.imap_on_demand().await?);
    }
    Ok(conn.as_mut().expect("just connected"))
}

/// A stand-in for reading attachments of kept messages: it never talks to a server.
struct NoConnection;

impl NoConnection {
    fn refuse<T>() -> Result<T> {
        Err(Error::Network(crate::tr!("Keine Verbindung.", "No connection.").into()))
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
    if let (Transport::Pop3 { pop3, smtp, username, .. }, Credential::Password(password)) = (transport, credential) {
        let (p, s) =
            tokio::join!(crate::pop3::check(pop3, username, password), smtp::check_with(smtp, username, credential));
        let mut out = CheckResult { imap: None, smtp: None, error: None, folders: 0 };
        match p {
            Ok(_) => out.folders = 1,
            Err(e) => (out.imap, out.error) = as_problem(e),
        }
        if let Err(e) = s {
            let (p, err) = as_problem(e);
            out.smtp = p;
            out.error = out.error.or(err);
        }
        return out;
    }
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
        /// How long a send takes (a large attachment, a slow server).
        slow: Arc<Mutex<Duration>>,
        /// Sends under way.
        in_flight: Arc<std::sync::atomic::AtomicU32>,
        /// The server files sent mail in Sent itself (as Gmail).
        files_sent: Arc<Mutex<bool>>,
        /// The answer to the message gets lost (it went out all the same).
        lose_answer: Arc<Mutex<bool>>,
        /// The SMTP server goes away once this many messages went out.
        down_after: Arc<Mutex<Option<usize>>>,
        /// How long one try of an on-demand request may take (the default when not set).
        on_demand_timeout: Arc<Mutex<Option<Duration>>>,
        /// How long connecting and signing in takes (TLS, login, capabilities).
        connect_delay: Arc<Mutex<Duration>>,
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
            let delay = *self.mail.connect_delay.lock().unwrap();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            Ok(self.server.connection())
        }
        async fn send(&self, from: &str, to: &[String], raw: &[u8]) -> Result<()> {
            let gone = self.mail.down_after.lock().unwrap().is_some_and(|n| self.mail.sent.lock().unwrap().len() >= n);
            if *self.mail.smtp_down.lock().unwrap() || gone {
                return Err(Error::Connection(Problem::Refused { host: "smtp".into(), port: 587 }));
            }
            use std::sync::atomic::Ordering;
            self.mail.in_flight.fetch_add(1, Ordering::SeqCst);
            let slow = *self.mail.slow.lock().unwrap();
            tokio::time::sleep(slow).await;
            self.mail.sent.lock().unwrap().push((from.into(), to.to_vec(), raw.to_vec()));
            self.mail.in_flight.fetch_sub(1, Ordering::SeqCst);
            if *self.mail.files_sent.lock().unwrap() {
                self.server.with(|f| f.deliver("Sent", raw, 1_760_000_900));
            }
            if *self.mail.lose_answer.lock().unwrap() {
                return Err(Error::Unconfirmed("connection closed after the message".into()));
            }
            Ok(())
        }
        fn files_sent(&self) -> bool {
            *self.mail.files_sent.lock().unwrap()
        }
        fn on_demand_timeout(&self) -> Duration {
            self.mail.on_demand_timeout.lock().unwrap().unwrap_or(ON_DEMAND_TIMEOUT)
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
        setup_with(name, FakeImap::default().with_all_folders())
    }

    fn setup_with(name: &str, mut fake: FakeImap) -> Setup {
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

        fn connector(&self) -> FakeConnector {
            FakeConnector { server: self.server.clone(), mail: self.mail.clone() }
        }

        fn send_op(&self, subject: &str) -> Op {
            Op::Send {
                raw_b64: base64::engine::general_purpose::STANDARD.encode(sample("out@x", subject, None, "Danke")),
                from: "maria@example.org".into(),
                recipients: vec!["jonas@example.org".into()],
                sent_b64: None,
                sent_mailbox: Some("Sent".into()),
                draft: None,
                answered: None,
                copies: vec![],
            }
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
    async fn rules_leave_alone_what_the_screener_moved_away() {
        use crate::rules::{self, Action, Condition, Field, Op, Place, Rule, RuleBody};
        let s = setup("screened-rules");
        crate::triage::block_sender(&s.store, "spam@example.net").unwrap();
        rules::save(
            &s.store,
            &Rule {
                id: 0,
                account_id: None,
                name: "Angebote".into(),
                place: Place::Local,
                enabled: true,
                position: 0,
                body: RuleBody {
                    match_all: true,
                    conditions: vec![Condition { field: Field::Subject, op: Op::Contains, value: "Angebot".into() }],
                    actions: vec![
                        Action::Forward { to: "chef@example.org".into() },
                        Action::Move { folder: "Angebote".into() },
                    ],
                    stop: false,
                },
                server_ref: None,
            },
        )
        .unwrap();
        let _w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        let spam = String::from_utf8(sample("spam@x", "Angebot der Woche", None, "x"))
            .unwrap()
            .replace("From: Maria Muster <maria@example.org>", "From: Werbung <spam@example.net>");
        s.server.with(|f| f.deliver("INBOX", spam.as_bytes(), 1_760_000_100));
        s.server.with(|f| f.deliver("INBOX", &sample("p@x", "Hallo", None, "x"), 1_760_000_101));
        s.until("new mail", |s| s.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::NewMail { .. }))).await;
        s.until("screened out on the server", |s| {
            s.server.with(|f| {
                f.mailboxes.get(crate::triage::BLOCKED_FOLDER).is_some_and(|m| !m.messages.is_empty())
                    && outbox::next_due(&s.store, s.account, now() + 60).unwrap().is_none()
            })
        })
        .await;
        let told: Vec<i64> = s
            .notices
            .lock()
            .unwrap()
            .iter()
            .filter_map(|n| match n {
                Notice::NewMail { messages, .. } => Some(messages.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let subjects: Vec<String> = s.store.summaries(&told).unwrap().into_iter().map(|m| m.subject).collect();
        assert_eq!(subjects, ["Hallo"]);
        assert!(
            !s.server.with(|f| f.log.iter().any(|l| l.contains("MOVE") && l.ends_with("Angebote"))),
            "no move of a message that is gone: {:?}",
            s.server.with(|f| f.log.clone())
        );
        let waiting: i64 = s
            .store
            .conn()
            .query_row("SELECT COUNT(*) FROM outbox WHERE state IN ('parked', 'failed')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(waiting, 0);
        assert!(s.mail.sent.lock().unwrap().is_empty(), "the screened mail was not forwarded");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rules_run_on_new_mail_and_what_they_move_is_no_news() {
        use crate::rules::{self, Action, Condition, Field, Op, Place, Rule, RuleBody};
        let s = setup("rules");
        rules::save(
            &s.store,
            &Rule {
                id: 0,
                account_id: None,
                name: "Shop".into(),
                place: Place::Local,
                enabled: true,
                position: 0,
                body: RuleBody {
                    match_all: true,
                    conditions: vec![Condition { field: Field::Subject, op: Op::Contains, value: "Rechnung".into() }],
                    actions: vec![Action::MarkRead, Action::Move { folder: "Rechnungen".into() }],
                    stop: false,
                },
                server_ref: None,
            },
        )
        .unwrap();
        let _w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        s.server.with(|f| f.deliver("INBOX", &sample("r@x", "Ihre Rechnung", None, "42 Euro"), 1_760_000_100));
        s.server.with(|f| f.deliver("INBOX", &sample("p@x", "Hallo", None, "x"), 1_760_000_101));
        s.until("new mail", |s| s.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::NewMail { .. }))).await;
        let told: Vec<i64> = s
            .notices
            .lock()
            .unwrap()
            .iter()
            .filter_map(|n| match n {
                Notice::NewMail { messages, .. } => Some(messages.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let subjects: Vec<String> = s.store.summaries(&told).unwrap().into_iter().map(|m| m.subject).collect();
        assert_eq!(subjects, ["Hallo"], "the bill went into its folder without a notification");
        s.until("moved on the server", |s| {
            s.server.with(|f| f.mailboxes.get("Rechnungen").is_some_and(|m| m.messages.values().any(|x| x.flags.seen)))
        })
        .await;
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
                copies: vec![],
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
            copies: vec![],
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

    /// A server without IDLE: a step whose undo window ends goes out then, not at the next
    /// poll five minutes later.
    #[tokio::test(flavor = "multi_thread")]
    async fn due_steps_go_out_on_time_without_idle() {
        let caps = Capabilities { idle: false, ..FakeImap::default().caps };
        let s = setup_with("poll-due", FakeImap::new(caps).with_all_folders());
        let w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        outbox::enqueue_at(&s.store, s.account, &s.send_op("Bald"), now() + 1).unwrap();
        w.send(Command::Kick);
        s.until("sent when due", |s| !s.mail.sent.lock().unwrap().is_empty()).await;
    }

    /// A message that cannot reach the SMTP server holds up no other change: read flags and a
    /// new folder reach the IMAP server, and the folder stays here meanwhile.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_send_that_cannot_go_out_holds_up_no_other_change() {
        let s = setup("send-lane");
        let w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        *s.mail.smtp_down.lock().unwrap() = true;
        outbox::enqueue(&s.store, s.account, &s.send_op("Später")).unwrap();
        w.send(Command::Kick);
        s.until("send failed", |s| outbox::entries(&s.store).unwrap().iter().any(|e| e.state == "failed")).await;
        let id = s.store.messages(s.inbox(), 10, 0).unwrap()[0].id;
        crate::actions::set_flags(&s.store, &[id], Some(true), None).unwrap();
        crate::actions::create_folder(&s.store, s.account, None, "Projekte").unwrap();
        w.send(Command::Kick);
        w.send(Command::SyncNow);
        s.until("flag and folder on the server", |s| {
            s.server.with(|f| f.mailboxes["INBOX"].messages[&1].flags.seen && f.mailboxes.contains_key("Projekte"))
        })
        .await;
        s.until("only the send waits", |s| {
            outbox::entries(&s.store).unwrap().iter().map(|e| e.kind.clone()).collect::<Vec<_>>() == ["send"]
        })
        .await;
        assert!(s.store.mailbox_by_name(s.account, "Projekte").unwrap().is_some());
        assert!(s.store.summaries(&[id]).unwrap()[0].seen);
        assert!(s.mail.sent.lock().unwrap().is_empty());
        // The SMTP server is back: the message goes out.
        *s.mail.smtp_down.lock().unwrap() = false;
        let send = outbox::entries(&s.store).unwrap()[0].id;
        outbox::retry(&s.store, send).unwrap();
        w.send(Command::Kick);
        s.until("sent", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
        assert_eq!(s.mail.sent.lock().unwrap().len(), 1);
    }

    /// An encrypted message's Bcc copies go out one by one after it, each exactly once: a copy
    /// that could not go out is tried again alone (the message and the copies before it are not
    /// sent again, although Sent already holds a message with their Message-ID), then the
    /// message is filed once.
    #[tokio::test(flavor = "multi_thread")]
    async fn bcc_copies_go_out_once_each() {
        let s = setup("bcc-copies");
        *s.mail.files_sent.lock().unwrap() = true;
        let w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        let enc = |to: &str| {
            base64::engine::general_purpose::STANDARD.encode(sample("out@x", &format!("Für {to}"), None, "x"))
        };
        let Op::Send { raw_b64, from, recipients, sent_mailbox, .. } = s.send_op("Geheim") else { panic!() };
        let op = Op::Send {
            raw_b64,
            from,
            recipients,
            sent_b64: None,
            sent_mailbox,
            draft: None,
            answered: None,
            copies: ["lea@example.org", "tom@example.org"]
                .iter()
                .map(|to| outbox::Copy { raw_b64: enc(to), recipients: vec![to.to_string()] })
                .collect(),
        };
        // The message and Lea's copy go out, then the SMTP server is gone.
        *s.mail.down_after.lock().unwrap() = Some(2);
        outbox::enqueue(&s.store, s.account, &op).unwrap();
        w.send(Command::Kick);
        s.until("failed at Tom's copy", |s| outbox::entries(&s.store).unwrap().iter().any(|e| e.state == "failed"))
            .await;
        assert_eq!(s.mail.sent.lock().unwrap().len(), 2);
        assert_eq!(outbox::entries(&s.store).unwrap()[0].kind, "send", "still in the sending lane");
        *s.mail.down_after.lock().unwrap() = None;
        let id = outbox::entries(&s.store).unwrap()[0].id;
        outbox::retry(&s.store, id).unwrap();
        w.send(Command::Kick);
        s.until("sent", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
        let to: Vec<Vec<String>> = s.mail.sent.lock().unwrap().iter().map(|m| m.1.clone()).collect();
        assert_eq!(
            to,
            [vec!["jonas@example.org".to_string()], vec!["lea@example.org".into()], vec!["tom@example.org".into()]]
        );
        assert_eq!(s.notices.lock().unwrap().iter().filter(|n| matches!(n, Notice::Sent { .. })).count(), 1);
    }

    /// A folder the server lists but will not open: the others are synced, the account gets to
    /// IDLE, and the folder says why it is empty.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_folder_the_server_will_not_open_holds_up_no_other_folder() {
        let mut fake = FakeImap::default().with_all_folders();
        fake.mailboxes.insert("Geteilt".into(), crate::imap::fake::FakeMailbox::new(&[]));
        fake.mailboxes.insert("Kunden".into(), crate::imap::fake::FakeMailbox::new(&[]));
        fake.refuse_select.push("Geteilt".into());
        let s = setup_with("noperm", fake);
        s.server.with(|f| f.deliver("Kunden", &sample("k@x", "Auftrag", None, "x"), 1_760_000_050));
        let _w = s.start();
        s.until("ready", |s| s.status() == Some(Status::Ready)).await;
        // The folders other than the inbox sync in the background, after „up to date“.
        let kunden = || s.store.mailbox_by_name(s.account, "Kunden").unwrap().unwrap();
        s.until("the folder after the refused one is synced", |_| kunden().total == 1).await;
        assert!(
            !s.notices
                .lock()
                .unwrap()
                .iter()
                .any(|n| matches!(n, Notice::Status { status: Status::Retrying { .. }, .. })),
            "no reconnect for one folder"
        );
        let selects = || s.server.with(|f| f.log.iter().filter(|l| l.as_str() == "SELECT Geteilt refused").count());
        assert_eq!(selects(), 1);
        // The next round leaves it out for a while.
        s.server.with(|f| f.deliver("INBOX", &sample("n@x", "Neu", None, "x"), 1_760_000_100));
        s.until("new mail", |s| s.store.messages(s.inbox(), 10, 0).unwrap().len() == 2).await;
        assert_eq!(selects(), 1, "not tried again at once");
    }

    /// A move or delete queued with the UIDs of one UIDVALIDITY never acts on another: the
    /// folder was rebuilt on the server, the step waits for the user.
    #[tokio::test]
    async fn queued_deletes_never_hit_messages_of_a_rebuilt_folder() {
        let s = setup("uidvalidity");
        s.server.with(|f| f.deliver("Trash", &sample("t1@x", "Alt 1", None, "x"), 1_760_000_001));
        s.server.with(|f| f.deliver("Trash", &sample("t2@x", "Wichtig behalten", None, "x"), 1_760_000_002));
        check_once(s.account, Store::open(&s.db).unwrap(), s.connector()).await.unwrap();
        let trash = s.store.mailbox_by_role(s.account, Role::Trash).unwrap().unwrap();
        let alt = s.store.messages(trash.id, 10, 0).unwrap().into_iter().find(|m| m.subject == "Alt 1").unwrap();
        crate::actions::delete(&s.store, &[alt.id]).unwrap();
        // Another client removes „Alt 1“, then the server renumbers the folder.
        s.server.with(|f| {
            f.expunge("Trash", 1);
            f.renumber("Trash");
        });
        s.store.conn().execute("UPDATE outbox SET next_try = 0", []).unwrap();
        check_once(s.account, Store::open(&s.db).unwrap(), s.connector()).await.unwrap();
        assert_eq!(s.server.with(|f| f.subjects("Trash")), ["Wichtig behalten"]);
        let e = outbox::entries(&s.store).unwrap();
        assert_eq!((e.len(), e[0].kind.as_str(), e[0].state.as_str()), (1, "delete", "parked"), "{e:?}");
    }

    /// A second worker of the same account (its servers changed, the phone came back to the
    /// front) while the first one still sends: the message goes out once.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_worker_never_sends_a_running_message_again() {
        use std::sync::atomic::Ordering;
        let s = setup("restart");
        *s.mail.slow.lock().unwrap() = Duration::from_millis(1500);
        let w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        outbox::enqueue(&s.store, s.account, &s.send_op("Einmal")).unwrap();
        w.send(Command::Kick);
        s.until("sending", |s| s.mail.in_flight.load(Ordering::SeqCst) > 0).await;
        drop(w);
        let w2 = s.start();
        // The phone's background check at the same time.
        let _ = check_once(s.account, Store::open(&s.db).unwrap(), s.connector()).await;
        s.until("done", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
        w2.send(Command::SyncNow);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(s.mail.sent.lock().unwrap().len(), 1, "sent once");
        assert_eq!(s.server.with(|f| f.subjects("Sent")), ["Einmal"]);
    }

    /// The answer to a sent message went missing: it is never sent again by itself. A server
    /// that files sent mail shows it in Sent (done); else the step waits for the user.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_send_whose_answer_was_lost_is_not_sent_again_by_itself() {
        for files_sent in [false, true] {
            let s = setup(if files_sent { "lost-answer-filed" } else { "lost-answer" });
            *s.mail.lose_answer.lock().unwrap() = true;
            *s.mail.files_sent.lock().unwrap() = files_sent;
            let w = s.start();
            s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
            outbox::enqueue(&s.store, s.account, &s.send_op("Vielleicht")).unwrap();
            w.send(Command::Kick);
            if files_sent {
                s.until("done", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
                assert!(s.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::Sent { .. })));
            } else {
                s.until("waits for the user", |s| {
                    outbox::entries(&s.store).unwrap().iter().any(|e| e.kind == "send" && e.state == "parked")
                })
                .await;
            }
            w.send(Command::SyncNow);
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(s.mail.sent.lock().unwrap().len(), 1, "files_sent {files_sent}: sent once");
        }
    }

    fn count_messages(s: &Setup) -> i64 {
        s.store.conn().query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap()
    }

    /// A large Gmail account on a slow network: the inbox's newest mail shows before the first
    /// sync is through, and a message opened meanwhile is fetched at once; the sync steps aside
    /// until the user has it. The older mail follows by itself.
    #[tokio::test(flavor = "multi_thread")]
    async fn opening_a_message_does_not_wait_for_the_first_sync() {
        let mut fake = FakeImap::gmail();
        fake.latency = Duration::from_millis(3);
        for i in 1..=2500u32 {
            let raw = sample(&format!("g{i}@x"), &format!("Mail {i}"), None, "Text");
            fake.deliver_gmail(&raw, 1_760_000_100 + i as i64 * 60, &["INBOX"]);
        }
        let s = setup_with("gmail-demand", fake);
        let w = s.start();
        s.until("the newest mail", |s| {
            s.store.mailbox_by_role(s.account, Role::Inbox).unwrap().is_some_and(|m| {
                s.store.messages(m.id, 1, 0).unwrap().first().is_some_and(|m| m.subject == "Mail 2500")
            })
        })
        .await;
        assert_ne!(s.status(), Some(Status::Ready), "shown while the first sync goes on");
        let oldest = *s.store.uids(s.inbox()).unwrap().first().unwrap();
        let id = s.store.ids_for_uids(s.inbox(), &[oldest]).unwrap()[0];
        let before = count_messages(&s);
        let started = Instant::now();
        w.fetch_body(id).await.unwrap();
        let grew = count_messages(&s) - before;
        assert!(s.store.message(id).unwrap().has_body);
        assert!(grew <= 2 * crate::sync::STORE_CHUNK as i64, "the sync went on meanwhile: {grew} more messages");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        // The rest follows: all of the inbox, the newest first.
        let start = Instant::now();
        while s.store.uids(s.inbox()).unwrap().len() < 2501 {
            assert!(start.elapsed() < Duration::from_secs(60), "backfill: {}", s.store.uids(s.inbox()).unwrap().len());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(s.store.backfill(s.inbox()).unwrap(), None);
        assert_eq!(s.store.messages(s.inbox(), 1, 0).unwrap()[0].subject, "Mail 2500");
    }

    /// A phone changed networks: the on-demand connection still looks open but never answers.
    /// The body comes over a new one after a short wait, not after minutes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_dead_on_demand_connection_is_replaced() {
        let s = setup("dead-demand");
        *s.mail.on_demand_timeout.lock().unwrap() = Some(Duration::from_millis(300));
        let w = s.start();
        s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
        let id = s.store.messages(s.inbox(), 1, 0).unwrap()[0].id;
        s.server.with(|f| f.stall.push("fetch_message".into()));
        let started = Instant::now();
        w.fetch_body(id).await.unwrap();
        assert!(s.store.message(id).unwrap().has_body);
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    }

    /// A message as Gmail delivers it: a header of about 4 KB (Received, ARC, DKIM, …) and a
    /// short text.
    fn gmail_like(i: u32, subject: &str) -> Vec<u8> {
        let mut h = String::with_capacity(4600);
        for hop in 0..6 {
            h.push_str(&format!(
                "Received: from mail-sor-f{hop}.google.com (mail-sor-f{hop}.google.com. [209.85.220.{hop}])\r\n        \
by mx.google.com with SMTPS id {i}x{hop}sor.2026.10.17.03.14.15\r\n        for <maria@example.org>\r\n        \
(Google Transport Security);\r\n        Sat, 17 Oct 2026 03:14:15 -0700 (PDT)\r\n"
            ));
        }
        for name in ["ARC-Seal", "ARC-Message-Signature", "DKIM-Signature", "X-Google-DKIM-Signature"] {
            h.push_str(&format!("{name}: i=1; a=rsa-sha256; d=example.net; s=20230601; c=relaxed/relaxed;\r\n"));
            for _ in 0..6 {
                h.push_str(
                    "        b=QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY3\r\n",
                );
            }
        }
        h.push_str("Authentication-Results: mx.google.com; dkim=pass header.i=@example.net; spf=pass; dmarc=pass\r\n");
        format!(
            "{h}From: Absender {n} <a{n}@example.net>\r\nTo: maria@example.org\r\nSubject: {subject}\r\n\
Date: Sat, 17 Oct 2026 03:14:15 -0700\r\nMessage-ID: <m{i}@example.net>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
Hallo Maria, hier ist Nachricht {i}. {}\r\n",
            "Ein wenig Text, damit es nach einer echten Nachricht aussieht. ".repeat(6),
            n = i % 400
        )
        .into_bytes()
    }

    /// A Gmail account as the owner's: `inbox` messages in the inbox, `all` in All Mail (the
    /// inbox's among them) and `labels` labels with `per_label` of the archived ones each.
    fn phone_account(inbox: u32, all: u32, labels: u32, per_label: u32) -> FakeImap {
        let mut f = FakeImap::gmail();
        let names: Vec<String> = (1..=labels).map(|l| format!("Label {l}")).collect();
        for n in &names {
            f.mailboxes.insert(n.clone(), crate::imap::fake::FakeMailbox::new(&[]));
        }
        let archived = all - inbox;
        let start = 1_700_000_000i64;
        for i in 1..=all {
            let at = start + i as i64 * 600;
            let mut into: Vec<&str> = vec![];
            if i > archived {
                into.push("INBOX");
            } else if labels > 0 && i % (archived / (labels * per_label).max(1)).max(1) == 0 {
                let l = (i as usize / 7) % names.len();
                if f.mailboxes[&names[l]].messages.len() < per_label as usize {
                    into.push(names[l].as_str());
                }
            }
            let raw = gmail_like(i, &format!("Mail {i}"));
            f.deliver_gmail(&raw, at, &into);
        }
        f.net = crate::imap::fake::NetProfile::phone();
        f
    }

    #[derive(Debug, Default)]
    struct PhaseTimes {
        /// „Synchronisiere …“ gone: the status says the account is up to date.
        ready: Option<Duration>,
        rows: Option<Duration>,
        previews: Option<Duration>,
        open: Option<Duration>,
        inbox_complete: Option<Duration>,
        caught_up: Option<Duration>,
    }

    /// Runs a worker against the phone profile until `until` (or everything caught up) and
    /// takes the times of the phases as the UI sees them (after the notice that tells it).
    async fn phone_run(s: &Setup, until: Duration, early: bool) -> PhaseTimes {
        let mut t = PhaseTimes::default();
        *s.mail.connect_delay.lock().unwrap() = Duration::from_millis(400);
        s.notices.lock().unwrap().clear();
        let w = s.start();
        let started = Instant::now();
        let inbox_told = |s: &Setup| s.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::Changed { .. }));
        let mut opener: Option<tokio::task::JoinHandle<Duration>> = None;
        while started.elapsed() < until {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if t.ready.is_none() && s.status() == Some(Status::Ready) {
                t.ready = Some(started.elapsed());
            }
            let Some(ib) = s.store.mailbox_by_role(s.account, Role::Inbox).unwrap() else { continue };
            let rows = s.store.messages(ib.id, 50, 0).unwrap();
            if t.rows.is_none() && rows.len() == 50 && inbox_told(s) {
                t.rows = Some(started.elapsed());
                // An older message nobody fetched yet, opened while the sync goes on.
                let id = rows[49].id;
                let h = w.handle();
                opener = Some(tokio::spawn(async move {
                    let at = Instant::now();
                    h.fetch_body(id).await.unwrap();
                    at.elapsed()
                }));
            }
            if t.rows.is_some() && t.previews.is_none() && rows.iter().all(|m| !m.snippet.is_empty()) {
                t.previews = Some(started.elapsed());
            }
            if t.open.is_none() && opener.as_ref().is_some_and(|o| o.is_finished()) {
                t.open = Some(opener.take().unwrap().await.unwrap());
            }
            if early && t.rows.is_some() && t.previews.is_some() && t.open.is_some() && t.ready.is_some() {
                break;
            }
            let waiting = s.store.backfill_pending(s.account).unwrap();
            if t.inbox_complete.is_none() && ib.sync.uid_next.is_some() && !waiting.contains(&ib.id) {
                t.inbox_complete = Some(started.elapsed());
            }
            if t.caught_up.is_none()
                && waiting.is_empty()
                && s.store.mailboxes(s.account).unwrap().iter().all(|m| !m.selectable || m.sync.uid_next.is_some())
            {
                t.caught_up = Some(started.elapsed());
                break;
            }
        }
        drop(w);
        let stopping = Instant::now();
        while s.status() != Some(Status::Stopped) && stopping.elapsed() < Duration::from_secs(900) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        println!("stopped after {:?}", stopping.elapsed());
        t
    }

    /// The targets on the phone profile (with room for a slow CI machine in a debug build):
    /// the newest inbox rows within 3 s of adding the account, their preview lines within
    /// 5 s, a message opened meanwhile within 2 s, the status up to date while the labels and
    /// All Mail still sync in the background; an app start with the account shows the stored
    /// mail at once and the new mail within about 2 s.
    #[tokio::test(flavor = "multi_thread")]
    async fn phone_profile_targets() {
        let s = setup_with("phone-targets", phone_account(30_000, 80_000, 40, 400));
        let slack = 2;
        let t = phone_run(&s, Duration::from_secs(40), true).await;
        println!("phone profile: {t:?}");
        let ok = |d: Option<Duration>, secs: u64| d.is_some_and(|d| d < Duration::from_secs(secs * slack));
        assert!(ok(t.rows, 3), "first rows: {t:?}");
        assert!(ok(t.previews, 5), "previews: {t:?}");
        assert!(ok(t.open, 2), "opening a message: {t:?}");
        assert!(ok(t.ready, 4), "up to date: {t:?}");
        // The labels and All Mail did not hold the inbox up: they came after it.
        s.server.with(|f| {
            for i in 0..5 {
                f.deliver_gmail(&gmail_like(900_000 + i, &format!("Neu {i}")), 1_800_000_000 + i as i64, &["INBOX"]);
            }
        });
        let ib = s.inbox();
        let cached = Instant::now();
        assert_eq!(s.store.messages(ib, 50, 0).unwrap().len(), 50);
        assert!(cached.elapsed() < Duration::from_millis(100), "stored mail at once: {:?}", cached.elapsed());
        *s.mail.connect_delay.lock().unwrap() = Duration::from_millis(400);
        let w = s.start();
        let started = Instant::now();
        while !s.store.messages(ib, 1, 0).unwrap()[0].subject.starts_with("Neu") {
            assert!(started.elapsed() < Duration::from_secs(2 * slack), "the new mail after a start");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(w);
    }

    /// The owner's Gmail account on a phone: 30,000 messages in the inbox, 80,000 in All Mail,
    /// 40 labels, 100 ms per round trip, 2 MB/s. Prints the times of the phases; run with
    /// `cargo test --release -p mail-core phone_profile -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "a measurement (minutes): run by hand in a release build"]
    async fn phone_profile_measurement() {
        let s = setup_with("phone-bench", phone_account(30_000, 80_000, 40, 400));
        let first = phone_run(&s, Duration::from_secs(900), false).await;
        println!("new account: {first:?}");
        // The app starts again with the account: cached mail at once, then the delta.
        s.server.with(|f| {
            for i in 0..10 {
                f.deliver_gmail(&gmail_like(900_000 + i, &format!("Neu {i}")), 1_800_000_000 + i as i64, &["INBOX"]);
            }
        });
        let ib = s.inbox();
        let cached = Instant::now();
        assert_eq!(s.store.messages(ib, 50, 0).unwrap().len(), 50);
        println!("cached rows read in {:?}", cached.elapsed());
        *s.mail.connect_delay.lock().unwrap() = Duration::from_millis(400);
        s.notices.lock().unwrap().clear();
        let w = s.start();
        let started = Instant::now();
        s.until("the new mail", |s| s.store.messages(ib, 1, 0).unwrap()[0].subject.starts_with("Neu")).await;
        println!("restart: delta in {:?}", started.elapsed());
        drop(w);
    }
}
