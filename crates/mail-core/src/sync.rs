//! The sync engine: brings the local store of a folder in line with the server, and the state
//! machine that decides when to connect, sync, wait in IDLE, poll and retry.
//!
//! Per folder ([`sync_mailbox`]) the cheapest way the server allows is taken:
//! * **QRESYNC** (RFC 7162): `SELECT … (QRESYNC (uidvalidity modseq))` answers with the changed
//!   flags and the vanished UIDs since the last sync; only new headers are fetched.
//! * **CONDSTORE**: flags `CHANGEDSINCE` the stored modseq, new headers, and a UID list only when
//!   the message count says something was removed.
//! * **Plain**: new headers above the stored UIDNEXT, all flags, and the UID list.
//! * **Full**: first sync, or UIDVALIDITY changed (the old UIDs mean nothing): the folder is
//!   cleared and fetched again.
//!
//! Bodies are not part of a folder sync; they are fetched when a message is opened
//! ([`fetch_body`]): whole up to [`FULL_FETCH_LIMIT`] (the message is kept, so its attachments
//! open offline), larger ones part by part, their attachments when opened
//! ([`fetch_attachment`]). The account worker (`crate::engine`) runs all of it.

use std::time::Duration;

use serde::Serialize;
use tokio::sync::Notify;

use crate::error::{Error, Result};
use crate::imap::{Capabilities, IdleOutcome, ImapClient, QresyncParams, Search, SelectInfo, UidRange};
use crate::parse::{AttachmentInfo, ParsedMessage, parse, snippet_of};
use crate::store::{PartialBody, Store, SyncState};
use crate::tr;

/// Headers fetched per round trip in a first sync.
pub const HEADER_BATCH: u32 = 500;
/// IDLE is renewed after this long (servers drop it after 30 minutes, RFC 2177).
pub const IDLE_RENEW: Duration = Duration::from_secs(25 * 60);
/// How often a server without IDLE (or a folder other than the inbox) is checked.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Messages up to this size are fetched whole and kept; larger ones part by part.
pub const FULL_FETCH_LIMIT: u32 = 2 * 1024 * 1024;
/// Pictures of a large HTML message fetched with its text (larger ones stay out).
pub const INLINE_IMAGE_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    Full,
    Qresync,
    Condstore,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    pub mailbox_id: i64,
    pub strategy: Strategy,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// New unread messages that arrived since the last sync (not the first fetch of a folder):
    /// what a notification tells about.
    pub new_unread: Vec<i64>,
}

impl SyncReport {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

/// The `QRESYNC` parameters for the `SELECT`, when the server has it and the folder was synced.
pub fn qresync_params(local: SyncState, caps: Capabilities) -> Option<QresyncParams> {
    match (caps.qresync, local.uid_validity, local.highest_modseq) {
        (true, Some(uid_validity), Some(modseq)) => Some(QresyncParams { uid_validity, modseq }),
        _ => None,
    }
}

/// How to sync after the `SELECT`.
pub fn strategy(local: SyncState, server: SelectInfo, caps: Capabilities, used_qresync: bool) -> Strategy {
    if local.uid_validity != Some(server.uid_validity) {
        Strategy::Full
    } else if used_qresync {
        Strategy::Qresync
    } else if caps.condstore && local.highest_modseq.is_some() && server.highest_modseq.is_some() {
        Strategy::Condstore
    } else {
        Strategy::Plain
    }
}

/// Fetches and stores the headers of `from:*`, in batches; returns the new ids and whether
/// each is unread.
async fn fetch_new<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_id: i64,
    from: u32,
    uid_next: u32,
) -> Result<Vec<(i64, bool)>> {
    let mut added = vec![];
    let mut start = from.max(1);
    while start < uid_next {
        let to = start.saturating_add(HEADER_BATCH - 1).min(uid_next - 1);
        let headers = client.fetch_headers(UidRange { from: start, to: Some(to) }).await?;
        store.batch(|| {
            for h in &headers {
                // A header that does not parse still gets a row (with an empty subject).
                let p = parse(&h.header).unwrap_or_default();
                added.push((store.insert_message(mailbox_id, h, &p)?, !h.flags.seen));
            }
            Ok(())
        })?;
        start = to + 1;
    }
    Ok(added)
}

/// Brings one folder in line with the server.
pub async fn sync_mailbox<C: ImapClient>(
    client: &mut C,
    store: &Store,
    caps: Capabilities,
    mailbox_id: i64,
) -> Result<SyncReport> {
    let mb = store.mailbox(mailbox_id)?;
    let local = mb.sync;
    let q = qresync_params(local, caps);
    let selected = client.select(&mb.name, q).await?;
    let server = selected.info;
    let strategy = strategy(local, server, caps, q.is_some());
    let mut report = SyncReport { mailbox_id, strategy, added: 0, updated: 0, removed: 0, new_unread: vec![] };
    let known_next = local.uid_next.unwrap_or(1);
    let new = match strategy {
        Strategy::Full => {
            report.removed = store.uids(mailbox_id)?.len();
            store.clear_mailbox(mailbox_id)?;
            fetch_new(client, store, mailbox_id, 1, server.uid_next).await?
        }
        Strategy::Qresync => {
            report.removed = store.remove_uids(mailbox_id, &selected.vanished)?;
            store.batch(|| {
                for c in selected.changed.iter().filter(|c| c.uid < known_next) {
                    store.set_flags(mailbox_id, c.uid, &c.flags, c.modseq)?;
                    report.updated += 1;
                }
                Ok(())
            })?;
            fetch_new(client, store, mailbox_id, known_next, server.uid_next).await?
        }
        Strategy::Condstore => {
            let mut new = vec![];
            if server.highest_modseq != local.highest_modseq {
                let since = local.highest_modseq;
                let changes = client
                    .fetch_flags(UidRange { from: 1, to: Some(known_next.saturating_sub(1).max(1)) }, since)
                    .await?;
                store.batch(|| {
                    for c in &changes {
                        store.set_flags(mailbox_id, c.uid, &c.flags, c.modseq)?;
                        report.updated += 1;
                    }
                    Ok(())
                })?;
                new = fetch_new(client, store, mailbox_id, known_next, server.uid_next).await?;
            }
            // Without QRESYNC the server does not say what was removed: compare the counts first.
            if store.uids(mailbox_id)?.len() as u32 != server.exists {
                report.removed = remove_gone(client, store, mailbox_id).await?;
            }
            new
        }
        Strategy::Plain => {
            let changes = client.fetch_flags(UidRange { from: 1, to: None }, None).await?;
            store.batch(|| {
                for c in changes.iter().filter(|c| c.uid < known_next) {
                    store.set_flags(mailbox_id, c.uid, &c.flags, c.modseq)?;
                    report.updated += 1;
                }
                Ok(())
            })?;
            let new = fetch_new(client, store, mailbox_id, known_next, server.uid_next).await?;
            report.removed = remove_gone(client, store, mailbox_id).await?;
            new
        }
    };
    report.added = new.len();
    // The first fetch of a folder is the existing mail, not new mail.
    if local.uid_validity.is_some() && strategy != Strategy::Full {
        report.new_unread = new.iter().filter(|(_, unread)| *unread).map(|(id, _)| *id).collect();
    }
    store.set_sync_state(
        mailbox_id,
        SyncState {
            uid_validity: Some(server.uid_validity),
            uid_next: Some(server.uid_next),
            highest_modseq: server.highest_modseq,
        },
    )?;
    Ok(report)
}

/// Removes the stored UIDs the server no longer lists.
async fn remove_gone<C: ImapClient>(client: &mut C, store: &Store, mailbox_id: i64) -> Result<usize> {
    let on_server: std::collections::HashSet<u32> = client.uids().await?.into_iter().collect();
    let gone: Vec<u32> = store.uids(mailbox_id)?.into_iter().filter(|u| !on_server.contains(u)).collect();
    store.remove_uids(mailbox_id, &gone)
}

/// Syncs the folder list of an account.
pub async fn sync_folders<C: ImapClient>(client: &mut C, store: &Store, account_id: i64) -> Result<()> {
    let entries = client.list().await?;
    for e in &entries {
        store.upsert_mailbox(account_id, e)?;
    }
    store.retain_mailboxes(account_id, &entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>())
}

/// Syncs the folder list of an account and then every folder, the inbox first.
pub async fn sync_account<C: ImapClient>(client: &mut C, store: &Store, account_id: i64) -> Result<Vec<SyncReport>> {
    let caps = client.capabilities().await?;
    sync_folders(client, store, account_id).await?;
    let mut reports = vec![];
    for mb in store.mailboxes(account_id)?.into_iter().filter(|m| m.selectable) {
        reports.push(sync_mailbox(client, store, caps, mb.id).await?);
    }
    Ok(reports)
}

/// The header fields of a stored message, for the search entry of a body fetched in parts.
fn header_of(store: &Store, message_id: i64) -> Result<ParsedMessage> {
    let d = store.message(message_id)?;
    Ok(ParsedMessage {
        subject: d.summary.subject,
        from: d.summary.from,
        to: d.to,
        cc: d.cc,
        ..ParsedMessage::default()
    })
}

/// The decoded content of a part fetched with its MIME header.
fn part_content(bytes: &[u8]) -> Vec<u8> {
    mail_parser::MessageParser::default()
        .parse(bytes)
        .and_then(|m| m.part(0).map(|p| p.contents().to_vec()))
        .unwrap_or_default()
}

/// Fetches the body of a stored header: the whole message up to [`FULL_FETCH_LIMIT`], else
/// the text, the HTML and its pictures (attachments stay on the server until opened).
pub async fn fetch_body<C: ImapClient>(client: &mut C, store: &Store, message_id: i64) -> Result<()> {
    let m = store.message(message_id)?;
    let mb = store.mailbox(m.summary.mailbox_id)?;
    let uid = m.summary.uid;
    client.select(&mb.name, None).await?;
    let parts = if m.size > FULL_FETCH_LIMIT { client.fetch_structure(uid).await? } else { vec![] };
    if parts.len() <= 1 {
        let raw = client.fetch_message(uid).await?;
        let p = parse(&raw)?;
        let keep = raw.len() as u64 <= FULL_FETCH_LIMIT as u64 * 2;
        return store.set_body_raw(message_id, &p, keep.then_some(&raw[..]));
    }
    let mut body = PartialBody::default();
    let first = |mime: &str| parts.iter().find(|p| p.mime_type == mime && !p.is_attachment()).cloned();
    if let Some(p) = first("text/plain") {
        body.text = parse(&client.fetch_section(uid, &p.section).await?)?.text;
    }
    if let Some(p) = first("text/html") {
        let html = parse(&client.fetch_section(uid, &p.section).await?)?;
        if body.text.is_none() {
            body.text = html.text.clone();
        }
        body.html = html.html;
    }
    body.snippet = snippet_of(body.text.as_deref().unwrap_or_default());
    for p in parts.iter().filter(|p| !p.is_attachment() && p.content_id.is_some() && p.mime_type.starts_with("image/"))
    {
        if p.size <= INLINE_IMAGE_LIMIT {
            let data = part_content(&client.fetch_section(uid, &p.section).await?);
            body.inline.push((p.content_id.clone().unwrap_or_default(), p.mime_type.clone(), data));
        }
    }
    body.attachments = parts
        .iter()
        .filter(|p| p.is_attachment())
        .enumerate()
        .map(|(i, p)| {
            let info = AttachmentInfo {
                index: i as u32,
                filename: p.filename.clone(),
                mime_type: p.mime_type.clone(),
                size: p.size,
                content_id: p.content_id.clone(),
                inline: p.inline,
            };
            (info, p.section.clone())
        })
        .collect();
    store.set_body_parts(message_id, &header_of(store, message_id)?, &body)
}

/// The content of an attachment: from the kept message, else its part from the server.
pub async fn fetch_attachment<C: ImapClient>(client: &mut C, store: &Store, attachment_id: i64) -> Result<Vec<u8>> {
    let a = store.attachment(attachment_id)?;
    if let Some(raw) = store.raw_message(a.message_id)? {
        return crate::parse::attachment_data(&raw, a.part_index).ok_or_else(|| {
            Error::NotFound(
                tr!("Der Anhang fehlt in der Nachricht.", "The attachment is missing in the message.").into(),
            )
        });
    }
    let section = a
        .section
        .clone()
        .ok_or_else(|| Error::NotFound(tr!("Der Anhang fehlt.", "The attachment is missing.").into()))?;
    let m = store.message(a.message_id)?;
    let mb = store.mailbox(m.summary.mailbox_id)?;
    client.select(&mb.name, None).await?;
    Ok(part_content(&client.fetch_section(m.summary.uid, &section).await?))
}

/// Searches the folders on the server (`UID SEARCH TEXT`); the ids of the stored messages
/// that match. Finds words in bodies that were never downloaded.
pub async fn server_search<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_ids: &[i64],
    query: &str,
) -> Result<Vec<i64>> {
    let mut out = vec![];
    for id in mailbox_ids {
        let mb = store.mailbox(*id)?;
        if !mb.selectable {
            continue;
        }
        client.select(&mb.name, None).await?;
        let uids = client.search(&Search::Text(query.trim().to_string())).await?;
        out.extend(store.ids_for_uids(mb.id, &uids)?);
    }
    Ok(out)
}

/// Waits in IDLE on the selected folder; the caller syncs on [`IdleOutcome::Changed`].
pub async fn wait_for_change<C: ImapClient>(client: &mut C, timeout: Duration, wake: &Notify) -> Result<IdleOutcome> {
    client.idle(timeout.min(IDLE_RENEW), wake).await
}

// ---------------------------------------------------------------------- connection state machine

/// Where an account's connection is. The worker (`crate::engine`) runs one machine per account
/// and carries out its [`Action`]s; the machine itself does no I/O, so every transition is
/// tested here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum State {
    /// Not started, stopped, or the device is offline.
    Offline,
    Connecting {
        attempt: u32,
    },
    Syncing {
        idle: bool,
    },
    /// Waiting for the server to report a change (IDLE).
    Idling,
    /// Waiting for the next check (no IDLE).
    Polling,
    /// Waiting after a failure before the next attempt.
    Backoff {
        attempt: u32,
    },
    /// The server refused the login: nothing happens until the user signs in again.
    NeedsSignIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Start,
    Stop,
    NetworkUp,
    NetworkDown,
    Connected {
        caps: Capabilities,
    },
    /// Connecting failed; `auth`: the login was refused.
    ConnectFailed {
        auth: bool,
    },
    SyncDone,
    SyncFailed,
    Idle(IdleOutcome),
    /// The poll or backoff timer fired.
    Timer,
    /// The user signed in again, or asked for a sync now.
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Connect,
    Sync,
    Idle,
    Disconnect,
    /// Start the timer; [`Event::Timer`] follows.
    Wait(Duration),
    /// Show „Anmeldung erforderlich“ for the account.
    AskToSignIn,
}

/// Wait before reconnect attempt `attempt`: 5 s, doubling, at most 5 minutes.
pub fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs((5u64 << attempt.saturating_sub(1).min(6)).min(300))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncMachine {
    pub state: State,
    online: bool,
}

impl Default for SyncMachine {
    fn default() -> Self {
        SyncMachine { state: State::Offline, online: true }
    }
}

impl SyncMachine {
    pub fn on(&mut self, event: Event) -> Vec<Action> {
        use Action as A;
        use State as S;
        let (next, actions) = match (self.state, event) {
            (_, Event::Stop) => (S::Offline, vec![A::Disconnect]),
            (_, Event::NetworkDown) => {
                self.online = false;
                (S::Offline, vec![A::Disconnect])
            }
            (S::Offline, Event::NetworkUp) | (S::Offline, Event::Start) => {
                if event == Event::NetworkUp {
                    self.online = true;
                }
                if self.online { (S::Connecting { attempt: 1 }, vec![A::Connect]) } else { (S::Offline, vec![]) }
            }
            (S::NeedsSignIn, Event::Retry) => (S::Connecting { attempt: 1 }, vec![A::Connect]),
            (S::NeedsSignIn, _) => (S::NeedsSignIn, vec![]),
            (S::Connecting { .. }, Event::Connected { caps }) => (S::Syncing { idle: caps.idle }, vec![A::Sync]),
            (S::Connecting { .. }, Event::ConnectFailed { auth: true }) => (S::NeedsSignIn, vec![A::AskToSignIn]),
            (S::Connecting { attempt }, Event::ConnectFailed { auth: false }) => {
                (S::Backoff { attempt }, vec![A::Wait(reconnect_delay(attempt))])
            }
            (S::Backoff { attempt }, Event::Timer) => (S::Connecting { attempt: attempt + 1 }, vec![A::Connect]),
            (S::Backoff { .. }, Event::Retry) => (S::Connecting { attempt: 1 }, vec![A::Connect]),
            (S::Syncing { idle: true }, Event::SyncDone) => (S::Idling, vec![A::Idle]),
            (S::Syncing { idle: false }, Event::SyncDone) => (S::Polling, vec![A::Wait(POLL_INTERVAL)]),
            (S::Syncing { .. }, Event::SyncFailed) => {
                (S::Backoff { attempt: 1 }, vec![A::Disconnect, A::Wait(reconnect_delay(1))])
            }
            (S::Idling, Event::Idle(IdleOutcome::Changed | IdleOutcome::Interrupted)) => {
                (S::Syncing { idle: true }, vec![A::Sync])
            }
            // Renewing IDLE is a cheap moment to check the other folders too.
            (S::Idling, Event::Idle(IdleOutcome::Timeout)) | (S::Idling, Event::Retry) => {
                (S::Syncing { idle: true }, vec![A::Sync])
            }
            (S::Idling, Event::SyncFailed) => {
                (S::Backoff { attempt: 1 }, vec![A::Disconnect, A::Wait(reconnect_delay(1))])
            }
            (S::Polling, Event::Timer) | (S::Polling, Event::Retry) => (S::Syncing { idle: false }, vec![A::Sync]),
            (state, _) => (state, vec![]),
        };
        self.state = next;
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{AccountDraft, Provider, preset};
    use crate::imap::Flags;
    use crate::imap::fake::FakeImap;
    use crate::parse::tests::sample;

    fn setup(caps: Capabilities) -> (FakeImap, Store, i64) {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .add_account(&AccountDraft {
                provider: Provider::Imap,
                name: String::new(),
                email: "maria@example.org".into(),
                sender_name: String::new(),
                transport: preset(Provider::Imap, "maria@example.org").unwrap(),
            })
            .unwrap();
        let mut server = FakeImap::new(caps);
        for i in 1..=3 {
            server.deliver(
                "INBOX",
                &sample(&format!("m{i}@x"), &format!("Nachricht {i}"), None, "Text"),
                1_760_000_000 + i,
            );
        }
        (server, store, account)
    }

    fn inbox(store: &Store, account: i64) -> i64 {
        store.mailboxes(account).unwrap().into_iter().find(|m| m.name == "INBOX").unwrap().id
    }

    fn all_caps() -> Capabilities {
        FakeImap::default().caps
    }

    fn seen() -> Flags {
        Flags { seen: true, ..Flags::default() }
    }

    #[tokio::test]
    async fn first_sync_is_full_then_qresync() {
        let (mut server, store, account) = setup(all_caps());
        let reports = sync_account(&mut server, &store, account).await.unwrap();
        assert_eq!(reports.len(), 3, "INBOX, Sent, Trash");
        let ib = inbox(&store, account);
        let first = reports.iter().find(|r| r.mailbox_id == ib).unwrap();
        assert_eq!((first.strategy, first.added), (Strategy::Full, 3));
        assert!(first.new_unread.is_empty(), "the first fetch is no new mail");

        server.deliver("INBOX", &sample("m4@x", "Neu", None, "x"), 1_760_000_100);
        server.set_flags("INBOX", 1, seen());
        server.expunge("INBOX", 2);
        server.log.clear();
        let r = sync_mailbox(&mut server, &store, all_caps(), ib).await.unwrap();
        assert_eq!((r.strategy, r.added, r.updated, r.removed), (Strategy::Qresync, 1, 1, 1));
        assert_eq!(r.new_unread.len(), 1, "new mail to tell about");
        assert!(server.log[0].contains("QRESYNC"), "{:?}", server.log);
        assert!(!server.log.iter().any(|l| l == "UID SEARCH ALL"), "QRESYNC needs no UID list");
        assert_eq!(store.uids(ib).unwrap(), vec![1, 3, 4]);
        assert_eq!(store.mailbox(ib).unwrap().unread, 2);

        // Nothing changed: no header is fetched.
        server.log.clear();
        let r = sync_mailbox(&mut server, &store, all_caps(), ib).await.unwrap();
        assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
        assert_eq!(server.log.len(), 1, "{:?}", server.log);
    }

    #[tokio::test]
    async fn condstore_without_qresync() {
        let caps = Capabilities { qresync: false, ..all_caps() };
        let (mut server, store, account) = setup(caps);
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        server.set_flags("INBOX", 3, seen());
        server.expunge("INBOX", 1);
        let r = sync_mailbox(&mut server, &store, caps, ib).await.unwrap();
        assert_eq!((r.strategy, r.updated, r.removed), (Strategy::Condstore, 1, 1));
        assert_eq!(store.uids(ib).unwrap(), vec![2, 3]);
    }

    #[tokio::test]
    async fn plain_server() {
        let caps = Capabilities::default();
        let (mut server, store, account) = setup(caps);
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        server.deliver("INBOX", &sample("m9@x", "Neu", None, "x"), 1_760_000_200);
        server.expunge("INBOX", 3);
        let r = sync_mailbox(&mut server, &store, caps, ib).await.unwrap();
        assert_eq!((r.strategy, r.added, r.removed), (Strategy::Plain, 1, 1));
        assert_eq!(store.uids(ib).unwrap(), vec![1, 2, 4]);
    }

    #[tokio::test]
    async fn uidvalidity_change_refetches_the_folder() {
        let (mut server, store, account) = setup(all_caps());
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        server.expunge("INBOX", 1);
        server.renumber("INBOX");
        let r = sync_mailbox(&mut server, &store, all_caps(), ib).await.unwrap();
        assert_eq!((r.strategy, r.removed, r.added), (Strategy::Full, 3, 2));
        assert!(r.new_unread.is_empty());
        assert_eq!(store.uids(ib).unwrap(), vec![1, 2]);
    }

    #[tokio::test]
    async fn bodies_on_demand_and_idle() {
        let (mut server, store, account) = setup(all_caps());
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        let id = store.messages(ib, 1, 0).unwrap()[0].id;
        assert!(!store.message(id).unwrap().has_body);
        fetch_body(&mut server, &store, id).await.unwrap();
        assert!(store.message(id).unwrap().has_body);
        assert!(store.raw_message(id).unwrap().is_some(), "small messages are kept whole");
        let wake = Notify::new();
        server.changed = false;
        assert_eq!(wait_for_change(&mut server, IDLE_RENEW, &wake).await.unwrap(), IdleOutcome::Timeout);
        server.deliver("INBOX", &sample("n@x", "x", None, "x"), 0);
        assert_eq!(wait_for_change(&mut server, IDLE_RENEW, &wake).await.unwrap(), IdleOutcome::Changed);
    }

    /// A message with an attachment larger than [`FULL_FETCH_LIMIT`]: text and pictures come
    /// at once, the attachment only when it is opened.
    #[tokio::test]
    async fn large_messages_come_in_parts() {
        let (mut server, store, account) = setup(all_caps());
        let big = "QUJD".repeat((FULL_FETCH_LIMIT as usize) / 3);
        let raw = format!(
            "From: Lea <lea@example.org>\r\nTo: maria@example.org\r\nSubject: Pläne\r\nMessage-ID: <big@x>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: multipart/related; boundary=\"R\"\r\n\r\n\
--R\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hier die <b>Pläne</b> <img src=\"cid:logo@x\"></p>\r\n\
--R\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--R--\r\n\
--B\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"plaene.zip\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{big}\r\n--B--\r\n"
        );
        server.deliver("INBOX", raw.as_bytes(), 1_760_000_500);
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        let id = store.messages(ib, 10, 0).unwrap().into_iter().find(|m| m.subject == "Pläne").unwrap().id;
        server.log.clear();
        fetch_body(&mut server, &store, id).await.unwrap();
        assert!(!server.log.iter().any(|l| l.contains("BODY.PEEK[])")), "not fetched whole: {:?}", server.log);
        let d = store.message(id).unwrap();
        assert!(d.html.as_deref().unwrap().contains("<b>Pläne</b>"));
        assert!(d.text.as_deref().unwrap().contains("Pläne"));
        assert_eq!(store.inline_images(id).unwrap()[0].0, "logo@x");
        let att: Vec<_> = d.attachments.iter().filter(|a| a.section.is_some()).collect();
        assert_eq!(att.len(), 1);
        assert_eq!(att[0].filename.as_deref(), Some("plaene.zip"));
        let data = fetch_attachment(&mut server, &store, att[0].id).await.unwrap();
        assert_eq!(data.len(), big.len() / 4 * 3, "the whole attachment");
    }

    #[tokio::test]
    async fn attachments_of_kept_messages_open_offline() {
        let (mut server, store, account) = setup(all_caps());
        let raw = "From: a@example.org\r\nSubject: Bericht\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"X\"\r\n\r\n--X\r\nContent-Type: text/plain\r\n\r\nAnbei.\r\n\
--X\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"bericht.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--X--\r\n";
        server.deliver("INBOX", raw.as_bytes(), 1_760_000_600);
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        let id = store.messages(ib, 10, 0).unwrap().into_iter().find(|m| m.subject == "Bericht").unwrap().id;
        fetch_body(&mut server, &store, id).await.unwrap();
        let a = store.message(id).unwrap().attachments[0].clone();
        server.fail.push("SELECT".into());
        server.log.clear();
        assert_eq!(fetch_attachment(&mut server, &store, a.id).await.unwrap(), b"%PDF-1.4\n");
        assert!(server.log.is_empty(), "from the kept message");
    }

    #[tokio::test]
    async fn search_on_the_server() {
        let (mut server, store, account) = setup(all_caps());
        server.deliver("INBOX", &sample("s@x", "Reise", None, "Die Tickets für Lissabon"), 1_760_000_700);
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        let hits = server_search(&mut server, &store, &[ib], "lissabon").await.unwrap();
        assert_eq!(store.summaries(&hits).unwrap()[0].subject, "Reise");
    }

    #[test]
    fn strategies() {
        let info = SelectInfo { uid_validity: 7, uid_next: 10, exists: 9, highest_modseq: Some(50) };
        let synced = SyncState { uid_validity: Some(7), uid_next: Some(10), highest_modseq: Some(40) };
        let caps = all_caps();
        assert_eq!(strategy(SyncState::default(), info, caps, false), Strategy::Full);
        assert_eq!(strategy(SyncState { uid_validity: Some(6), ..synced }, info, caps, true), Strategy::Full);
        assert_eq!(strategy(synced, info, caps, true), Strategy::Qresync);
        assert_eq!(strategy(synced, info, caps, false), Strategy::Condstore);
        assert_eq!(strategy(synced, info, Capabilities::default(), false), Strategy::Plain);
        assert!(qresync_params(SyncState::default(), caps).is_none());
        assert!(qresync_params(synced, Capabilities::default()).is_none());
        assert_eq!(qresync_params(synced, caps), Some(QresyncParams { uid_validity: 7, modseq: 40 }));
    }

    #[test]
    fn machine_with_idle() {
        let mut m = SyncMachine::default();
        assert_eq!(m.on(Event::Start), vec![Action::Connect]);
        assert_eq!(m.on(Event::Connected { caps: all_caps() }), vec![Action::Sync]);
        assert_eq!(m.on(Event::SyncDone), vec![Action::Idle]);
        assert_eq!(m.state, State::Idling);
        assert_eq!(m.on(Event::Idle(IdleOutcome::Changed)), vec![Action::Sync]);
        assert_eq!(m.on(Event::SyncDone), vec![Action::Idle]);
        assert_eq!(m.on(Event::Idle(IdleOutcome::Interrupted)), vec![Action::Sync]);
        assert_eq!(m.on(Event::SyncDone), vec![Action::Idle]);
        assert_eq!(m.on(Event::Idle(IdleOutcome::Timeout)), vec![Action::Sync]);
        assert_eq!(m.on(Event::SyncFailed), vec![Action::Disconnect, Action::Wait(Duration::from_secs(5))]);
        assert_eq!(m.on(Event::Timer), vec![Action::Connect]);
        assert_eq!(m.state, State::Connecting { attempt: 2 });
    }

    #[test]
    fn machine_without_idle_polls() {
        let mut m = SyncMachine::default();
        m.on(Event::Start);
        m.on(Event::Connected { caps: Capabilities::default() });
        assert_eq!(m.on(Event::SyncDone), vec![Action::Wait(POLL_INTERVAL)]);
        assert_eq!(m.on(Event::Timer), vec![Action::Sync]);
    }

    #[test]
    fn machine_backoff_sign_in_and_network() {
        let mut m = SyncMachine::default();
        m.on(Event::Start);
        assert_eq!(m.on(Event::ConnectFailed { auth: false }), vec![Action::Wait(Duration::from_secs(5))]);
        m.on(Event::Timer);
        assert_eq!(m.on(Event::ConnectFailed { auth: false }), vec![Action::Wait(Duration::from_secs(10))]);
        m.on(Event::Timer);
        assert_eq!(m.on(Event::ConnectFailed { auth: true }), vec![Action::AskToSignIn]);
        assert_eq!(m.on(Event::Timer), vec![], "no retries with a refused login");
        assert_eq!(m.on(Event::Retry), vec![Action::Connect]);
        assert_eq!(m.on(Event::NetworkDown), vec![Action::Disconnect]);
        assert_eq!(m.on(Event::Start), vec![], "offline: nothing to do");
        assert_eq!(m.on(Event::NetworkUp), vec![Action::Connect]);
        assert_eq!(reconnect_delay(30), Duration::from_secs(300));
    }
}
