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
//!   cleared and fetched again, the newest [`SyncOptions::first_headers`] first (a large mailbox
//!   is usable within seconds); the older mail follows a part per round ([`backfill`]).
//!
//! New headers come with their preview line (the beginning of the body), so the list shows the
//! first lines before any body is fetched.
//!
//! Bodies are not part of a folder sync; they are fetched when a message is opened
//! ([`fetch_body`]): whole up to [`FULL_FETCH_LIMIT`] (the message is kept, so its attachments
//! open offline), larger ones part by part, their attachments when opened
//! ([`fetch_attachment`]). The account worker (`crate::engine`) runs all of it.

use std::time::Duration;

use serde::Serialize;
use tokio::sync::Notify;

use crate::error::{Error, Result};
use crate::imap::{Capabilities, IdleOutcome, ImapClient, MailboxEntry, QresyncParams, Search, SelectInfo, UidRange};
use crate::parse::{AttachmentInfo, ParsedMessage, parse, snippet_of};
use crate::store::{PartialBody, Store, SyncState};
use crate::tr;

/// Headers fetched per round trip.
pub const HEADER_BATCH: u32 = 500;
/// The first round trip of a folder's first sync is small: the newest mail shows within
/// seconds, also on a slow phone network.
pub const FIRST_BATCH: u32 = 100;
/// Headers a first sync fetches per folder (the newest); the older ones follow a part per round
/// ([`backfill`]). The setting `initial_headers` changes it.
pub const FIRST_HEADERS: u32 = 1000;
/// Older headers fetched per round while a folder's older mail waits.
pub const BACKFILL_PER_ROUND: u32 = 5000;
/// Headers the first sync of a folder other than the inbox fetches at most (labels, All Mail):
/// they are not the first thing anyone looks at, the older mail follows.
pub const FOLDER_FIRST_HEADERS: u32 = 250;
/// The newest messages of a folder whose preview line comes with the header.
pub const PREVIEW_COUNT: usize = 200;
/// Headers stored per transaction: between two, work the user waits for goes first ([`Gate`]).
pub const STORE_CHUNK: usize = 100;
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
    /// Every message that arrived since the last sync (not the first fetch): what the rules
    /// and the priority look at.
    pub new: Vec<i64>,
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

/// Lets what the user waits for go first: the on-demand connection holds it while it fetches a
/// body (it runs on the worker's thread too), and the sync waits at each step until it is free.
#[derive(Debug, Default)]
pub struct Gate {
    busy: std::cell::Cell<u32>,
    free: Notify,
}

impl Gate {
    pub fn enter(&self) {
        self.busy.set(self.busy.get() + 1);
    }

    pub fn leave(&self) {
        self.busy.set(self.busy.get().saturating_sub(1));
        if self.busy.get() == 0 {
            self.free.notify_waiters();
        }
    }

    pub fn busy(&self) -> bool {
        self.busy.get() > 0
    }

    /// Gives other tasks a turn, then waits while the gate is held.
    pub async fn pass(&self) {
        tokio::task::yield_now().await;
        loop {
            let free = self.free.notified();
            tokio::pin!(free);
            free.as_mut().enable();
            if !self.busy() {
                return;
            }
            free.await;
        }
    }
}

/// How a folder sync goes besides the connection and the store.
#[derive(Clone, Copy)]
pub struct SyncOptions<'a> {
    /// Headers the first sync of a folder fetches (the newest); older mail waits for [`backfill`].
    pub first_headers: u32,
    /// Called with the folder after each stored part (and again when its preview lines came),
    /// so the list shows the newest mail before the rest arrived.
    pub on_chunk: Option<&'a dyn Fn(i64)>,
    /// Waited on before each step: work that goes first (what the user waits for; for the
    /// background, the inbox).
    pub gates: &'a [&'a Gate],
    /// Where the steps' times go („Sync-Details“).
    pub log: Option<&'a crate::diag::SyncLog>,
}

impl Default for SyncOptions<'_> {
    fn default() -> Self {
        SyncOptions { first_headers: FIRST_HEADERS, on_chunk: None, gates: &[], log: None }
    }
}

impl SyncOptions<'_> {
    async fn pass(&self) {
        for g in self.gates {
            g.pass().await;
        }
    }

    fn note(&self, what: impl FnOnce() -> String, since: std::time::Instant) {
        if let Some(log) = self.log {
            log.note(&what(), since.elapsed());
        }
    }

    fn shown(&self, mailbox_id: i64) {
        if let Some(f) = self.on_chunk {
            f(mailbox_id);
        }
    }
}

/// Fetches and stores the headers of one UID range, shows them, then (with `previews`) their
/// preview lines; returns the ids and whether each is unread.
async fn fetch_range<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_id: i64,
    range: UidRange,
    previews: bool,
    opts: &SyncOptions<'_>,
) -> Result<Vec<(i64, bool)>> {
    let t = std::time::Instant::now();
    let mut headers = client.fetch_headers(range).await?;
    opts.note(|| format!("{} {}", tr!("Kopfzeilen", "headers"), headers.len()), t);
    let t = std::time::Instant::now();
    let mut added = vec![];
    let mut parsed: std::collections::HashMap<u32, (i64, ParsedMessage, Vec<u8>)> = Default::default();
    // The newest first, so a pause between two parts leaves the newest stored.
    headers.sort_by_key(|h| std::cmp::Reverse(h.uid));
    for part in headers.chunks(STORE_CHUNK) {
        opts.pass().await;
        store.batch(|| {
            for h in part {
                // A header that does not parse still gets a row (with an empty subject).
                let p = parse(&h.header).unwrap_or_default();
                let id = store.insert_message(mailbox_id, h, &p)?;
                added.push((id, !h.flags.seen));
                if previews {
                    parsed.insert(h.uid, (id, p, h.header.clone()));
                }
            }
            Ok(())
        })?;
    }
    opts.note(|| format!("{} {}", tr!("gespeichert", "stored"), added.len()), t);
    opts.shown(mailbox_id);
    if previews && !parsed.is_empty() {
        let t = std::time::Instant::now();
        let bodies = client.fetch_previews(range).await?;
        opts.note(|| format!("{} {}", tr!("Vorschauen", "previews"), bodies.len()), t);
        store.batch(|| {
            for (uid, body) in &bodies {
                if let Some((id, p, header)) = parsed.get_mut(uid) {
                    p.snippet = crate::parse::preview(header, body);
                    store.set_preview(*id, p)?;
                }
            }
            Ok(())
        })?;
        opts.shown(mailbox_id);
    }
    Ok(added)
}

/// Whether the preview lines of a folder come with its headers (not for Gmail's All Mail,
/// Starred and Important: their messages are the other folders' again).
fn wants_previews(store: &Store, mailbox_id: i64) -> Result<bool> {
    Ok(!store.mailbox(mailbox_id)?.role.is_some_and(crate::imap::Role::is_virtual))
}

/// Fetches and stores the given UIDs (ascending, all on the server), the newest first in
/// parts; the first part is small. The parts among the newest `previews` come with their
/// preview lines.
async fn fetch_uids<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_id: i64,
    uids: &[u32],
    first: bool,
    previews: usize,
    opts: &SyncOptions<'_>,
) -> Result<Vec<(i64, bool)>> {
    let previews = if wants_previews(store, mailbox_id)? { previews } else { 0 };
    let mut added = vec![];
    let mut end = uids.len();
    let mut batch = if first { FIRST_BATCH } else { HEADER_BATCH } as usize;
    while end > 0 {
        let start = end.saturating_sub(batch);
        // The UIDs between two listed ones are not on the server: a range fetches just these.
        let range = UidRange { from: uids[start], to: Some(uids[end - 1]) };
        let with_previews = uids.len() - end < previews;
        added.extend(fetch_range(client, store, mailbox_id, range, with_previews, opts).await?);
        end = start;
        batch = HEADER_BATCH as usize;
    }
    Ok(added)
}

/// Fetches and stores the headers of `from:*` (new mail since the last sync); a large gap goes
/// by the server's UID list, the newest first.
async fn fetch_new<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_id: i64,
    from: u32,
    uid_next: u32,
    opts: &SyncOptions<'_>,
) -> Result<Vec<(i64, bool)>> {
    let from = from.max(1);
    if from >= uid_next {
        return Ok(vec![]);
    }
    if uid_next - from <= HEADER_BATCH {
        let previews = wants_previews(store, mailbox_id)?;
        return fetch_range(client, store, mailbox_id, UidRange { from, to: Some(uid_next - 1) }, previews, opts).await;
    }
    let uids: Vec<u32> = client.uids().await?.into_iter().filter(|u| *u >= from && *u < uid_next).collect();
    fetch_uids(client, store, mailbox_id, &uids, false, PREVIEW_COUNT, opts).await
}

/// The first fetch of a folder: the newest [`SyncOptions::first_headers`] headers, the newest
/// first; what is older waits for [`backfill`].
async fn fetch_first<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_id: i64,
    exists: u32,
    opts: &SyncOptions<'_>,
) -> Result<Vec<(i64, bool)>> {
    if exists == 0 {
        store.set_backfill(mailbox_id, None, 0)?;
        return Ok(vec![]);
    }
    let t = std::time::Instant::now();
    let all = client.uids().await?;
    opts.note(|| format!("{} {}", tr!("UID-Liste", "UID list"), all.len()), t);
    let take = all.len().min(opts.first_headers.max(1) as usize);
    let newest = &all[all.len() - take..];
    let added = fetch_uids(client, store, mailbox_id, newest, true, PREVIEW_COUNT, opts).await?;
    let left = (all.len() - take) as u32;
    store.set_backfill(mailbox_id, (left > 0).then(|| newest[0]), left)?;
    Ok(added)
}

/// Fetches up to `max` older headers of a folder whose first sync left them on the server, the
/// newest of them first; returns how many were stored. Nothing waits: nothing to do.
pub async fn backfill<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_id: i64,
    max: u32,
    opts: &SyncOptions<'_>,
) -> Result<usize> {
    let Some((below, _)) = store.backfill(mailbox_id)? else { return Ok(0) };
    let mb = store.mailbox(mailbox_id)?;
    let sel = client.select(&mb.name, None).await?;
    if mb.sync.uid_validity != Some(sel.info.uid_validity) {
        // Renumbered on the server: the next sync of the folder starts over.
        return Ok(0);
    }
    let t = std::time::Instant::now();
    let older: Vec<u32> = client.uids().await?.into_iter().filter(|u| *u < below).collect();
    opts.note(
        || {
            format!(
                "{} {}: {} {}",
                tr!("Älteres", "older mail"),
                crate::diag::folder_label(&mb),
                tr!("UID-Liste", "UID list"),
                older.len()
            )
        },
        t,
    );
    let stop = older.len() - older.len().min(max as usize);
    let mut end = older.len();
    let mut added = 0;
    if older.is_empty() {
        store.set_backfill(mailbox_id, None, 0)?;
    }
    // Part by part, each noted when stored: an app closed meanwhile goes on from there.
    while end > stop {
        let start = end.saturating_sub(HEADER_BATCH as usize).max(stop);
        added += fetch_uids(client, store, mailbox_id, &older[start..end], false, 0, opts).await?.len();
        store.set_backfill(mailbox_id, (start > 0).then(|| older[start]), start as u32)?;
        end = start;
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
    sync_mailbox_with(client, store, caps, mailbox_id, &SyncOptions::default()).await
}

/// [`sync_mailbox`] with the options of the account's worker.
pub async fn sync_mailbox_with<C: ImapClient>(
    client: &mut C,
    store: &Store,
    caps: Capabilities,
    mailbox_id: i64,
    opts: &SyncOptions<'_>,
) -> Result<SyncReport> {
    let mb = store.mailbox(mailbox_id)?;
    let local = mb.sync;
    let q = qresync_params(local, caps);
    let t = std::time::Instant::now();
    let selected = client.select(&mb.name, q).await?;
    let server = selected.info;
    let strategy = strategy(local, server, caps, q.is_some());
    opts.note(
        || {
            format!(
                "SELECT {} ({} {}, {strategy:?})",
                crate::diag::folder_label(&mb),
                server.exists,
                tr!("Nachrichten", "messages")
            )
        },
        t,
    );
    let mut report =
        SyncReport { mailbox_id, strategy, added: 0, updated: 0, removed: 0, new_unread: vec![], new: vec![] };
    let known_next = local.uid_next.unwrap_or(1);
    // Messages whose flag change has not reached the server yet keep what the user did here.
    let pending = crate::outbox::pending_flag_uids(store, mb.account_id, &mb.name)?;
    let set_flags = |uid: u32, flags: &crate::imap::Flags, modseq: Option<u64>| -> Result<bool> {
        if pending.contains(&uid) {
            return Ok(false);
        }
        store.set_flags(mailbox_id, uid, flags, modseq)?;
        Ok(true)
    };
    let new = match strategy {
        Strategy::Full => {
            report.removed = store.uids(mailbox_id)?.len();
            store.clear_mailbox(mailbox_id)?;
            fetch_first(client, store, mailbox_id, server.exists, opts).await?
        }
        Strategy::Qresync => {
            report.removed = store.remove_uids(mailbox_id, &selected.vanished)?;
            store.batch(|| {
                for c in selected.changed.iter().filter(|c| c.uid < known_next) {
                    if set_flags(c.uid, &c.flags, c.modseq)? {
                        report.updated += 1;
                    }
                }
                Ok(())
            })?;
            fetch_new(client, store, mailbox_id, known_next, server.uid_next, opts).await?
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
                        if set_flags(c.uid, &c.flags, c.modseq)? {
                            report.updated += 1;
                        }
                    }
                    Ok(())
                })?;
                new = fetch_new(client, store, mailbox_id, known_next, server.uid_next, opts).await?;
            }
            // Without QRESYNC the server does not say what was removed: compare the counts first
            // (with the older messages that wait for the backfill).
            let waiting = store.backfill(mailbox_id)?.map_or(0, |(_, left)| left);
            if store.uids(mailbox_id)?.len() as u32 + waiting != server.exists {
                report.removed = remove_gone(client, store, mailbox_id).await?;
            }
            new
        }
        Strategy::Plain => {
            let changes = client.fetch_flags(UidRange { from: 1, to: None }, None).await?;
            store.batch(|| {
                for c in changes.iter().filter(|c| c.uid < known_next) {
                    if set_flags(c.uid, &c.flags, c.modseq)? {
                        report.updated += 1;
                    }
                }
                Ok(())
            })?;
            let new = fetch_new(client, store, mailbox_id, known_next, server.uid_next, opts).await?;
            report.removed = remove_gone(client, store, mailbox_id).await?;
            new
        }
    };
    report.added = new.len();
    // The first fetch of a folder is the existing mail, not new mail.
    if local.uid_validity.is_some() && strategy != Strategy::Full {
        report.new_unread = new.iter().filter(|(_, unread)| *unread).map(|(id, _)| *id).collect();
        report.new = new.iter().map(|(id, _)| *id).collect();
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

/// Removes the stored UIDs the server no longer lists (and counts again what waits for the
/// backfill).
async fn remove_gone<C: ImapClient>(client: &mut C, store: &Store, mailbox_id: i64) -> Result<usize> {
    let listed = client.uids().await?;
    if let Some((below, _)) = store.backfill(mailbox_id)? {
        let left = listed.iter().filter(|u| **u < below).count() as u32;
        store.set_backfill(mailbox_id, (left > 0).then_some(below), left)?;
    }
    let on_server: std::collections::HashSet<u32> = listed.into_iter().collect();
    let gone: Vec<u32> = store.uids(mailbox_id)?.into_iter().filter(|u| !on_server.contains(u)).collect();
    store.remove_uids(mailbox_id, &gone)
}

/// Syncs the folder list of an account. Folders created or renamed here that the queue has not
/// brought to the server yet stay as they are here (the LIST does not know them, or still has
/// the old name).
pub async fn sync_folders<C: ImapClient>(client: &mut C, store: &Store, account_id: i64) -> Result<()> {
    let entries = client.list().await?;
    let (waiting, leaving) = crate::outbox::pending_folders(store, account_id)?;
    let under = |name: &str, root: &str, delim: Option<char>| {
        name == root || delim.is_some_and(|d| name.strip_prefix(root).is_some_and(|rest| rest.starts_with(d)))
    };
    let entries: Vec<MailboxEntry> =
        entries.into_iter().filter(|e| !leaving.iter().any(|l| under(&e.name, l, e.delimiter))).collect();
    let roles = crate::imap::roles(&entries);
    for (e, role) in entries.iter().zip(roles) {
        store.upsert_mailbox_with_role(account_id, e, role)?;
    }
    let mut keep: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    for m in store.mailboxes(account_id)? {
        let delim = m.delimiter.as_deref().and_then(|d| d.chars().next());
        if waiting.iter().any(|w| under(&m.name, w, delim)) {
            keep.push(m.name);
        }
    }
    store.retain_mailboxes(account_id, &keep)
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
    client.select(&mb.name, None).await?;
    fetch_selected(client, store, message_id).await
}

/// Messages of a folder fetched ahead after a sync (the newest first): the list shows their
/// first lines, search finds their text, and they open offline.
pub const PREFETCH_COUNT: u32 = 60;
/// Larger messages wait until they are opened.
pub const PREFETCH_SIZE: u32 = 256 * 1024;

/// Fetches the bodies of the newest messages of a folder that have none yet, as the setting
/// says (see [`PREFETCH_COUNT`]); returns how many were fetched.
pub async fn prefetch_bodies<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_id: i64,
    policy: crate::settings::Prefetch,
) -> Result<usize> {
    prefetch_bodies_with(client, store, mailbox_id, policy, &[]).await
}

/// [`prefetch_bodies`] that steps aside at the [`Gate`] before each message.
pub async fn prefetch_bodies_with<C: ImapClient>(
    client: &mut C,
    store: &Store,
    mailbox_id: i64,
    policy: crate::settings::Prefetch,
    gates: &[&Gate],
) -> Result<usize> {
    use crate::settings::Prefetch;
    let ids = match policy {
        Prefetch::None => vec![],
        Prefetch::Recent => store.without_body(mailbox_id, PREFETCH_COUNT, PREFETCH_SIZE)?,
        Prefetch::Month => store.without_body_since(
            mailbox_id,
            crate::store::now() - 30 * 86400,
            PREFETCH_MONTH_COUNT,
            FULL_FETCH_LIMIT,
        )?,
    };
    if ids.is_empty() {
        return Ok(0);
    }
    let mb = store.mailbox(mailbox_id)?;
    client.select(&mb.name, None).await?;
    let mut fetched = 0;
    // Whole messages several to a round trip; the large ones (in parts) one by one.
    let mut small = vec![];
    for &id in &ids {
        let m = store.message(id)?;
        if m.size <= FULL_FETCH_LIMIT {
            small.push((m.summary.uid, id));
        } else {
            for g in gates {
                g.pass().await;
            }
            // Its copy in another folder brought it meanwhile.
            if !store.message(id)?.has_body {
                fetch_selected(client, store, id).await?;
                fetched += 1;
            }
        }
    }
    for part in small.chunks(PREFETCH_BATCH) {
        for g in gates {
            g.pass().await;
        }
        let mut wanted = vec![];
        for &(uid, id) in part {
            if !store.message(id)?.has_body {
                wanted.push(uid);
            }
        }
        for (uid, raw) in client.fetch_messages(&wanted).await? {
            let Some(&(_, id)) = part.iter().find(|(u, _)| *u == uid) else { continue };
            store.batch(|| store_whole(store, id, &raw))?;
            fetched += 1;
        }
    }
    Ok(fetched)
}

/// Messages fetched whole per round trip when bodies are fetched ahead.
pub const PREFETCH_BATCH: usize = 20;

/// Keeps a message fetched whole (up to twice [`FULL_FETCH_LIMIT`] with its raw form, for its
/// attachments offline) and gives it to its other copies.
fn store_whole(store: &Store, id: i64, raw: &[u8]) -> Result<()> {
    let p = parse(raw)?;
    let keep = raw.len() as u64 <= FULL_FETCH_LIMIT as u64 * 2;
    store.set_body_raw(id, &p, keep.then_some(raw))?;
    store.share_body(id)?;
    Ok(())
}

/// The most a folder fetches ahead in one round with [`crate::settings::Prefetch::Month`].
pub const PREFETCH_MONTH_COUNT: u32 = 500;

async fn fetch_selected<C: ImapClient>(client: &mut C, store: &Store, message_id: i64) -> Result<()> {
    let m = store.message(message_id)?;
    let uid = m.summary.uid;
    let parts = if m.size > FULL_FETCH_LIMIT { client.fetch_structure(uid).await? } else { vec![] };
    if parts.len() <= 1 {
        let raw = client.fetch_message(uid).await?;
        return store_whole(store, message_id, &raw);
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
    // The pictures the HTML shows: inline ones, and those it names by Content-ID whatever their
    // disposition says (some mailers mark them as attachments, some as octet streams).
    let shown = body.html.as_deref().map(crate::parse::cid_refs).unwrap_or_default();
    for p in parts.iter().filter(|p| {
        let Some(cid) = &p.content_id else { return false };
        let named = shown.contains(&crate::parse::cid_key(cid));
        let picture = p.mime_type.starts_with("image/") || p.mime_type == "application/octet-stream";
        picture && (named || !p.is_attachment() && p.mime_type.starts_with("image/"))
    }) {
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
    // A meeting's calendar data comes along (small), so its invitation shows and applies.
    let mut calendar = vec![];
    for p in parts.iter().filter(|p| p.is_attachment() && is_calendar_part(p) && p.size <= CALENDAR_PART_LIMIT) {
        calendar.push((p.section.clone(), part_content(&client.fetch_section(uid, &p.section).await?)));
    }
    store.set_body_parts(message_id, &header_of(store, message_id)?, &body)?;
    for (section, data) in calendar {
        store.set_attachment_data(message_id, &section, &data)?;
    }
    store.share_body(message_id)?;
    Ok(())
}

/// The largest calendar part fetched with the body of a large message.
pub const CALENDAR_PART_LIMIT: u64 = 1024 * 1024;

fn is_calendar_part(p: &crate::imap::BodyPart) -> bool {
    let mime = p.mime_type.to_ascii_lowercase();
    mime == "text/calendar"
        || mime == "application/ics"
        || p.filename.as_deref().is_some_and(|f| f.to_ascii_lowercase().ends_with(".ics"))
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
    /// The computer woke up or changed networks: a connection may be dead without knowing it.
    Reconnect,
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
    /// Rounds that failed in a row (connecting worked each time): the wait grows with them.
    failed_rounds: u32,
}

impl Default for SyncMachine {
    fn default() -> Self {
        SyncMachine { state: State::Offline, online: true, failed_rounds: 0 }
    }
}

impl SyncMachine {
    pub fn on(&mut self, event: Event) -> Vec<Action> {
        use Action as A;
        use State as S;
        let (next, actions) = match (self.state, event) {
            (_, Event::Stop) => (S::Offline, vec![A::Disconnect]),
            (S::NeedsSignIn, Event::Reconnect) => (S::NeedsSignIn, vec![]),
            (_, Event::Reconnect) => {
                self.online = true;
                self.failed_rounds = 0;
                (S::Connecting { attempt: 1 }, vec![A::Disconnect, A::Connect])
            }
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
            (S::Syncing { idle: true }, Event::SyncDone) => {
                self.failed_rounds = 0;
                (S::Idling, vec![A::Idle])
            }
            (S::Syncing { idle: false }, Event::SyncDone) => {
                self.failed_rounds = 0;
                (S::Polling, vec![A::Wait(POLL_INTERVAL)])
            }
            (S::Syncing { .. }, Event::SyncFailed) => {
                self.failed_rounds += 1;
                let attempt = self.failed_rounds;
                (S::Backoff { attempt }, vec![A::Disconnect, A::Wait(reconnect_delay(attempt))])
            }
            (S::Idling, Event::Idle(IdleOutcome::Changed | IdleOutcome::Interrupted)) => {
                (S::Syncing { idle: true }, vec![A::Sync])
            }
            // Renewing IDLE is a cheap moment to check the other folders too.
            (S::Idling, Event::Idle(IdleOutcome::Timeout)) | (S::Idling, Event::Retry) => {
                (S::Syncing { idle: true }, vec![A::Sync])
            }
            (S::Idling, Event::SyncFailed) => {
                self.failed_rounds += 1;
                let attempt = self.failed_rounds;
                (S::Backoff { attempt }, vec![A::Disconnect, A::Wait(reconnect_delay(attempt))])
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
    use crate::imap::fake::FakeImap;
    use crate::imap::{Flags, Role};
    use crate::parse::tests::sample;
    use crate::settings::Prefetch;

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

    /// Gmail: labels are folders, All Mail holds every message again. Search shows one copy,
    /// archiving goes to All Mail, the roles are found.
    #[tokio::test]
    async fn gmail_folders() {
        use crate::imap::fake::FakeMailbox;
        let (mut server, store, account) = setup(all_caps());
        for (name, attr) in [
            ("[Gmail]/All Mail", "\\All"),
            ("[Gmail]/Sent Mail", "\\Sent"),
            ("[Gmail]/Important", "\\Important"),
            ("[Gmail]/Starred", "\\Flagged"),
            ("[Gmail]/Trash", "\\Trash"),
        ] {
            server.mailboxes.insert(name.into(), FakeMailbox::new(&[attr]));
        }
        server.mailboxes.insert("[Gmail]".into(), FakeMailbox::new(&["\\Noselect"]));
        for i in 1..=3 {
            let raw = sample(&format!("m{i}@x"), &format!("Nachricht {i}"), None, "Text");
            server.deliver("[Gmail]/All Mail", &raw, 1_760_000_000 + i);
        }
        sync_account(&mut server, &store, account).await.unwrap();
        let role = |r: Role| store.mailbox_by_role(account, r).unwrap().map(|m| m.name);
        assert_eq!(role(Role::Important).as_deref(), Some("[Gmail]/Important"));
        assert_eq!(role(Role::All).as_deref(), Some("[Gmail]/All Mail"));
        assert_eq!(crate::search::search(&store, "Nachricht 2", Some(account), 10).unwrap().len(), 1);
        let hit = &crate::search::search(&store, "Nachricht 2", Some(account), 10).unwrap()[0];
        assert_eq!(hit.mailbox_id, inbox(&store, account), "the inbox copy, not All Mail's");
        let undo = crate::actions::archive(&store, &[hit.id]).unwrap();
        assert_eq!(undo.outbox.len(), 1);
        assert!(store.mailbox_by_name(account, "Archive").unwrap().is_none(), "no folder made up");
        let raw: String =
            store.conn().query_row("SELECT payload FROM outbox WHERE id = ?1", [undo.outbox[0]], |r| r.get(0)).unwrap();
        assert!(raw.contains("[Gmail]/All Mail"), "{raw}");
        assert!(Role::All.is_virtual() && !Role::Inbox.is_virtual());
    }

    #[tokio::test]
    async fn newest_bodies_are_fetched_ahead() {
        let (mut server, store, account) = setup(all_caps());
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        let n = store.messages(ib, 100, 0).unwrap().len();
        assert_eq!(prefetch_bodies(&mut server, &store, ib, Prefetch::None).await.unwrap(), 0, "switched off");
        assert_eq!(prefetch_bodies(&mut server, &store, ib, Prefetch::Recent).await.unwrap(), n);
        assert!(store.messages(ib, 100, 0).unwrap().iter().all(|m| store.message(m.id).unwrap().has_body));
        assert!(!store.messages(ib, 100, 0).unwrap()[0].snippet.is_empty(), "the list shows the first lines");
        assert_eq!(prefetch_bodies(&mut server, &store, ib, Prefetch::Recent).await.unwrap(), 0, "nothing twice");
        server.deliver("INBOX", &"x".repeat(PREFETCH_SIZE as usize + 10).into_bytes(), 0);
        sync_mailbox(&mut server, &store, all_caps(), ib).await.unwrap();
        assert_eq!(
            prefetch_bodies(&mut server, &store, ib, Prefetch::Recent).await.unwrap(),
            0,
            "large ones wait until opened"
        );
        // A month ahead takes larger ones too, but only recent ones.
        assert_eq!(prefetch_bodies(&mut server, &store, ib, Prefetch::Month).await.unwrap(), 0, "delivered in 1970");
        server.deliver("INBOX", &"y".repeat(PREFETCH_SIZE as usize + 10).into_bytes(), crate::store::now());
        sync_mailbox(&mut server, &store, all_caps(), ib).await.unwrap();
        assert_eq!(prefetch_bodies(&mut server, &store, ib, Prefetch::Month).await.unwrap(), 1);
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

    /// An invitation with a large agenda attached: its calendar part comes with the text, so the
    /// card shows and the update applies without the whole message.
    #[tokio::test]
    async fn invitations_in_large_messages_come_with_the_text() {
        let (mut server, store, account) = setup(all_caps());
        let big = "QUJD".repeat((FULL_FETCH_LIMIT as usize) / 3);
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:big-1\r\nSEQUENCE:0\r\n\
DTSTART:20261103T090000Z\r\nDTEND:20261103T100000Z\r\nSUMMARY:Planung\r\nORGANIZER:mailto:lea@example.org\r\n\
ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:maria@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let raw = format!(
            "From: Lea <lea@example.org>\r\nTo: maria@example.org\r\nSubject: Planung\r\nMessage-ID: <bigics@x>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: multipart/alternative; boundary=\"A\"\r\n\r\n\
--A\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nDie Agenda ist angehängt.\r\n\
--A\r\nContent-Type: text/calendar; charset=utf-8; method=REQUEST\r\n\r\n{ics}\r\n--A--\r\n\
--B\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"agenda.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{big}\r\n--B--\r\n"
        );
        server.deliver("INBOX", raw.as_bytes(), 1_760_000_550);
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = inbox(&store, account);
        let id = store.messages(ib, 10, 0).unwrap().into_iter().find(|m| m.subject == "Planung").unwrap().id;
        fetch_body(&mut server, &store, id).await.unwrap();
        assert!(store.raw_message(id).unwrap().is_none(), "not kept whole");
        let data = crate::itip::calendar_data(&store, id).unwrap().expect("the calendar part");
        assert!(data.contains("UID:big-1") && data.contains("METHOD:REQUEST"));
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

    /// After sleep or a network change the connection is dropped and made again at once,
    /// whether it was idling or waiting out a long backoff.
    #[test]
    fn machine_reconnects_promptly() {
        let mut m = SyncMachine::default();
        m.on(Event::Start);
        m.on(Event::Connected { caps: all_caps() });
        m.on(Event::SyncDone);
        assert_eq!(m.state, State::Idling);
        assert_eq!(m.on(Event::Reconnect), vec![Action::Disconnect, Action::Connect]);
        for _ in 0..6 {
            m.on(Event::ConnectFailed { auth: false });
            m.on(Event::Timer);
        }
        m.on(Event::ConnectFailed { auth: false });
        assert!(matches!(m.state, State::Backoff { attempt } if attempt > 5));
        assert_eq!(m.on(Event::Reconnect), vec![Action::Disconnect, Action::Connect]);
        assert_eq!(m.state, State::Connecting { attempt: 1 }, "the backoff starts over");
        m.on(Event::NetworkDown);
        assert_eq!(m.on(Event::Reconnect), vec![Action::Disconnect, Action::Connect], "a new network is up");
        m.on(Event::ConnectFailed { auth: true });
        assert_eq!(m.on(Event::Reconnect), vec![], "waits for the password");
    }

    /// A sync before the queue reached the server keeps what the user changed here.
    #[tokio::test]
    async fn a_sync_before_the_replay_keeps_local_flags() {
        for caps in [Capabilities::default(), Capabilities { qresync: false, ..all_caps() }] {
            let (mut server, store, account) = setup(caps);
            sync_account(&mut server, &store, account).await.unwrap();
            let ib = inbox(&store, account);
            let id = store.messages(ib, 10, 0).unwrap()[0].id;
            crate::actions::set_flags(&store, &[id], Some(true), Some(true)).unwrap();
            // Something else changed on the server meanwhile (CONDSTORE then fetches flags).
            server.set_flags("INBOX", 1, Flags { answered: true, ..Flags::default() });
            server.set_flags("INBOX", 2, Flags { answered: true, ..Flags::default() });
            server.set_flags("INBOX", 3, Flags { answered: true, ..Flags::default() });
            sync_mailbox(&mut server, &store, caps, ib).await.unwrap();
            let m = &store.summaries(&[id]).unwrap()[0];
            assert!(m.seen && m.flagged, "{caps:?}: {m:?}");
        }
    }

    /// Folders created or renamed here wait for the server; a folder sync in between neither
    /// drops them nor brings the old name back.
    #[tokio::test]
    async fn folders_waiting_for_the_server_survive_a_folder_sync() {
        let (mut server, store, account) = setup(all_caps());
        server.do_create("Alt").unwrap();
        sync_account(&mut server, &store, account).await.unwrap();
        crate::actions::create_folder(&store, account, None, "Projekte").unwrap();
        let alt = store.mailbox_by_name(account, "Alt").unwrap().unwrap();
        crate::actions::rename_folder(&store, alt.id, "Neu").unwrap();
        sync_folders(&mut server, &store, account).await.unwrap();
        let names: Vec<String> = store.mailboxes(account).unwrap().into_iter().map(|m| m.name).collect();
        assert!(names.contains(&"Projekte".to_string()), "{names:?}");
        assert!(names.contains(&"Neu".to_string()) && !names.contains(&"Alt".to_string()), "{names:?}");
    }

    /// Roles by name only where the server marks no folder with that role, and never for a
    /// user's folder further down: deleting there moves to the real trash.
    #[tokio::test]
    async fn special_folders_by_name_only_where_the_server_does_not_say() {
        use crate::imap::fake::FakeMailbox;
        let (mut server, store, account) = setup(all_caps());
        for name in ["Archiv", "Archiv/Papierkorb", "Kunden", "Kunden/Spam", "Projekt.Trash"] {
            server.mailboxes.insert(name.into(), FakeMailbox::new(&[]));
        }
        sync_account(&mut server, &store, account).await.unwrap();
        let role = |name: &str| store.mailbox_by_name(account, name).unwrap().unwrap().role;
        assert_eq!(store.mailbox_by_role(account, Role::Trash).unwrap().unwrap().name, "Trash");
        assert_eq!(role("Archiv/Papierkorb"), None);
        assert_eq!(role("Kunden/Spam"), None);
        assert_eq!(role("Projekt.Trash"), None);
        assert_eq!(role("Archiv"), Some(Role::Archive), "no folder marked \\Archive: the name says it");
        server.deliver("Kunden/Spam", &sample("k@x", "Kundenmail", None, "x"), 1_760_000_500);
        let spam = store.mailbox_by_name(account, "Kunden/Spam").unwrap().unwrap();
        sync_mailbox(&mut server, &store, all_caps(), spam.id).await.unwrap();
        let id = store.messages(spam.id, 10, 0).unwrap()[0].id;
        crate::actions::delete(&store, &[id]).unwrap();
        let kinds: Vec<String> = crate::outbox::entries(&store).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["move"], "to the trash, not deleted for good");

        // A server without SPECIAL-USE: the names count, top level (and below INBOX) only.
        let (mut plain, store, account) = setup(Capabilities::default());
        plain.mailboxes.clear();
        for name in ["INBOX", "INBOX/Gesendet", "Papierkorb", "Kunden/Spam", "Spam", "Archiv/Papierkorb"] {
            plain.mailboxes.insert(name.into(), FakeMailbox::new(&[]));
        }
        sync_account(&mut plain, &store, account).await.unwrap();
        let role = |name: &str| store.mailbox_by_name(account, name).unwrap().unwrap().role;
        assert_eq!(role("INBOX/Gesendet"), Some(Role::Sent));
        assert_eq!(role("Papierkorb"), Some(Role::Trash));
        assert_eq!(role("Spam"), Some(Role::Junk));
        assert_eq!(role("Kunden/Spam"), None);
        assert_eq!(role("Archiv/Papierkorb"), None);
    }

    /// A message as Gmail keeps them: numbered, the oldest first, with its body as given.
    fn gmail_mail(i: u32, body: &str) -> Vec<u8> {
        format!(
            "From: Absender {i} <a{i}@example.net>\r\nTo: maria@example.org\r\nSubject: Mail {i}\r\n\
Message-ID: <m{i}@example.net>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    fn gmail_caps() -> Capabilities {
        FakeImap::gmail().caps
    }

    fn gmail_setup() -> (FakeImap, Store, i64) {
        let (_, store, account) = setup(all_caps());
        (FakeImap::gmail(), store, account)
    }

    fn folder(store: &Store, account: i64, name: &str) -> i64 {
        store.mailbox_by_name(account, name).unwrap().unwrap_or_else(|| panic!("no folder {name}")).id
    }

    /// A large Gmail inbox: the first sync fetches the newest headers first (a small first part,
    /// shown at once), stops after `first_headers`, and the older mail follows a part per round;
    /// the list is newest first all along.
    #[tokio::test]
    async fn a_large_inbox_comes_newest_first_and_the_rest_follows() {
        let (mut server, store, account) = gmail_setup();
        for i in 1..=3000u32 {
            // From 2015 on, one message a day.
            server.deliver_gmail(&gmail_mail(i, "Hallo"), 1_440_000_000 + i as i64 * 86_400, &["INBOX"]);
        }
        sync_folders(&mut server, &store, account).await.unwrap();
        let ib = folder(&store, account, "INBOX");
        let seen: std::cell::RefCell<Vec<(usize, String)>> = Default::default();
        let shown = |m: i64| {
            let top = store.messages(m, 1, 0).unwrap().first().map(|s| s.subject.clone()).unwrap_or_default();
            seen.borrow_mut().push((store.uids(m).unwrap().len(), top));
        };
        let opts = SyncOptions { first_headers: 1000, on_chunk: Some(&shown), ..SyncOptions::default() };
        server.log.clear();
        let r = sync_mailbox_with(&mut server, &store, gmail_caps(), ib, &opts).await.unwrap();
        assert_eq!((r.strategy, r.added), (Strategy::Full, 1000));
        let fetches: Vec<&String> = server.log.iter().filter(|l| l.contains("(HEADER)")).collect();
        assert_eq!(fetches[0], "UID FETCH 2901:3000 (HEADER)", "the newest first: {fetches:?}");
        assert_eq!(seen.borrow()[0], (FIRST_BATCH as usize, "Mail 3000".to_string()), "shown after the first part");
        let list = store.messages(ib, 2000, 0).unwrap();
        assert_eq!(list.len(), 1000);
        assert_eq!((list[0].subject.as_str(), list[999].subject.as_str()), ("Mail 3000", "Mail 2001"));
        assert_eq!(store.backfill(ib).unwrap(), Some((2001, 2000)), "the older mail waits");

        // New mail meanwhile comes as usual and is news.
        server.deliver_gmail(&gmail_mail(3001, "Neu"), 1_440_000_000 + 3001 * 86_400, &["INBOX"]);
        let r = sync_mailbox_with(&mut server, &store, gmail_caps(), ib, &opts).await.unwrap();
        assert_eq!((r.strategy, r.added, r.new_unread.len(), r.removed), (Strategy::Condstore, 1, 1, 0));

        // The older mail, a part per round, the newest of it first.
        server.log.clear();
        assert_eq!(backfill(&mut server, &store, ib, 1500, &opts).await.unwrap(), 1500);
        let fetches: Vec<&String> = server.log.iter().filter(|l| l.contains("(HEADER)")).collect();
        assert_eq!(fetches[0], "UID FETCH 1501:2000 (HEADER)", "{fetches:?}");
        assert_eq!(store.backfill(ib).unwrap(), Some((501, 500)));
        assert_eq!(backfill(&mut server, &store, ib, 1500, &opts).await.unwrap(), 500);
        assert_eq!(store.backfill(ib).unwrap(), None, "all here");
        assert_eq!(backfill(&mut server, &store, ib, 1500, &opts).await.unwrap(), 0);
        let list = store.messages(ib, 5000, 0).unwrap();
        assert_eq!(list.len(), 3001);
        assert_eq!((list[0].subject.as_str(), list[3000].subject.as_str()), ("Mail 3001", "Mail 1"));
        assert!(list.windows(2).all(|w| w[0].date >= w[1].date), "newest first");
        // Nothing removed, nothing fetched again: the counts agree.
        server.log.clear();
        let r = sync_mailbox_with(&mut server, &store, gmail_caps(), ib, &opts).await.unwrap();
        assert_eq!((r.added, r.removed), (0, 0));
        assert!(!server.log.iter().any(|l| l == "UID SEARCH ALL"), "{:?}", server.log);
    }

    /// A Gmail inbox of 50,000 messages: the first sync asks for the newest 1000 headers in
    /// three round trips of at most 500, with one UID list; nothing else is fetched.
    #[tokio::test]
    async fn fifty_thousand_messages_start_small() {
        let (mut server, store, account) = gmail_setup();
        let inbox = server.mailboxes.get_mut("INBOX").unwrap();
        for i in 1..=50_000u32 {
            inbox.messages.insert(
                i,
                crate::imap::fake::FakeMessage {
                    raw: gmail_mail(i, "x"),
                    flags: Flags::default(),
                    modseq: 1,
                    internal_date: 1_440_000_000 + i as i64 * 600,
                    gm_msgid: Some(i as u64),
                },
            );
        }
        inbox.uid_next = 50_001;
        sync_folders(&mut server, &store, account).await.unwrap();
        let ib = folder(&store, account, "INBOX");
        server.log.clear();
        let r = sync_mailbox(&mut server, &store, gmail_caps(), ib).await.unwrap();
        assert_eq!(r.added, FIRST_HEADERS as usize);
        let fetches: Vec<&String> = server.log.iter().filter(|l| l.contains("(HEADER)")).collect();
        assert_eq!(
            fetches,
            ["UID FETCH 49901:50000 (HEADER)", "UID FETCH 49401:49900 (HEADER)", "UID FETCH 49001:49400 (HEADER)"]
        );
        assert_eq!(server.log.iter().filter(|l| *l == "UID SEARCH ALL").count(), 1);
        assert_eq!(store.backfill(ib).unwrap(), Some((49_001, 49_000)));
        assert_eq!(store.messages(ib, 1, 0).unwrap()[0].subject, "Mail 50000");
    }

    /// Mail removed on the server while older mail still waits is removed here too, and the
    /// count of what waits stays right.
    #[tokio::test]
    async fn removals_while_older_mail_waits() {
        let (mut server, store, account) = gmail_setup();
        for i in 1..=30u32 {
            server.deliver_gmail(&gmail_mail(i, "x"), 1_440_000_000 + i as i64, &["INBOX"]);
        }
        sync_folders(&mut server, &store, account).await.unwrap();
        let ib = folder(&store, account, "INBOX");
        let opts = SyncOptions { first_headers: 10, ..SyncOptions::default() };
        sync_mailbox_with(&mut server, &store, gmail_caps(), ib, &opts).await.unwrap();
        assert_eq!(store.backfill(ib).unwrap(), Some((21, 20)));
        server.expunge("INBOX", 30);
        server.expunge("INBOX", 3);
        let r = sync_mailbox_with(&mut server, &store, gmail_caps(), ib, &opts).await.unwrap();
        assert_eq!(r.removed, 1);
        assert_eq!(store.backfill(ib).unwrap(), Some((21, 19)));
        assert_eq!(backfill(&mut server, &store, ib, 100, &opts).await.unwrap(), 19);
        assert_eq!(store.uids(ib).unwrap().len(), 28);
    }

    /// Gmail keeps a message once per label and once more in All Mail, all with one
    /// X-GM-MSGID: one body fetch serves every copy, search shows one. Messages that only look
    /// alike (six receipts of one day) stay six.
    #[tokio::test]
    async fn gmail_copies_are_one_message_and_look_alikes_stay_apart() {
        let (mut server, store, account) = gmail_setup();
        let receipt = |n: u32| {
            format!(
                "From: PlayStation Network <Sony@email.sonyentertainmentnetwork.com>\r\nTo: maria@example.org\r\n\
Subject: Danke für Ihren Einkauf\r\nDate: Mon, 24 Aug 2015 10:00:00 +0000\r\nMessage-ID: <psn{n}@sony.example>\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nArtikel {n}\r\n"
            )
            .into_bytes()
        };
        for n in 1..=6 {
            server.deliver_gmail(&receipt(n), 1_440_400_000, &["INBOX"]);
        }
        let confirm = sample("konto@x", "Bestätige dein Konto", None, "Bitte bestätigen");
        let gm = server.deliver_gmail(&confirm, 1_760_000_000, &["INBOX", "Account Confirmations"]);
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = folder(&store, account, "INBOX");
        let rows = store.messages(ib, 50, 0).unwrap();
        assert_eq!(rows.len(), 7, "six receipts and one more: {rows:?}");
        assert_eq!(rows.iter().filter(|m| m.subject == "Danke für Ihren Einkauf").count(), 6);
        let copies: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM messages WHERE gm_msgid = ?1", [gm as i64], |r| r.get(0))
            .unwrap();
        assert_eq!(copies, 3, "inbox, label and All Mail");
        let hits = crate::search::search(&store, "Bestätige", Some(account), 10).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].mailbox_id, ib, "the inbox copy");
        let id = hits[0].id;
        fetch_body(&mut server, &store, id).await.unwrap();
        let label = folder(&store, account, "Account Confirmations");
        let twin = store.messages(label, 1, 0).unwrap()[0].id;
        assert!(store.message(twin).unwrap().has_body, "the label's copy has it too");
        server.log.clear();
        assert_eq!(prefetch_bodies(&mut server, &store, label, Prefetch::Recent).await.unwrap(), 0);
        assert!(!server.log.iter().any(|l| l.contains("BODY.PEEK[]")), "not fetched again: {:?}", server.log);
        // Only X-GM-MSGID makes copies one: the six receipts are six.
        let psn = crate::search::search(&store, "Einkauf", Some(account), 10).unwrap();
        assert_eq!(psn.len(), 6, "{psn:?}");
    }

    /// The preview line comes with the header (a part of the body), before any body is fetched:
    /// plain, quoted-printable, base64, HTML only, and a long text cut in the middle.
    #[tokio::test]
    async fn preview_lines_come_with_the_headers() {
        let (mut server, store, account) = gmail_setup();
        let mail = |id: &str, subject: &str, rest: &str| {
            format!(
                "From: Lea <lea@example.org>\r\nTo: maria@example.org\r\nSubject: {subject}\r\nMessage-ID: <{id}>\r\n\
MIME-Version: 1.0\r\n{rest}"
            )
            .into_bytes()
        };
        let long = "Erste Zeile der langen Nachricht. ".repeat(200);
        let b64 = base64::engine::general_purpose::STANDARD.encode(format!("Base64 kommt an. {long}"));
        let b64_lines: Vec<String> =
            b64.as_bytes().chunks(76).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
        use base64::Engine;
        for raw in [
            mail("p1@x", "Plain", "Content-Type: text/plain; charset=utf-8\r\n\r\nGanz einfach.\r\n"),
            mail(
                "p2@x",
                "QP",
                "Content-Type: multipart/alternative; boundary=\"A\"\r\n\r\n--A\r\nContent-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\r\nGr=C3=BC=C3=9Fe aus K=C3=B6ln\r\n--A\r\nContent-Type: text/html\r\n\r\n<p>x</p>\r\n--A--\r\n",
            ),
            mail(
                "p3@x",
                "Base64",
                &format!(
                    "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
                    b64_lines.join("\r\n")
                ),
            ),
            mail(
                "p4@x",
                "HTML",
                "Content-Type: text/html; charset=utf-8\r\n\r\n<html><head><style>p{color:red}</style></head><body><p>Nur HTML hier</p></body></html>\r\n",
            ),
            mail("p5@x", "Lang", &format!("Content-Type: text/plain; charset=utf-8\r\n\r\n{long}\r\n")),
        ] {
            server.deliver_gmail(&raw, 1_760_000_000, &["INBOX"]);
        }
        sync_folders(&mut server, &store, account).await.unwrap();
        let ib = folder(&store, account, "INBOX");
        sync_mailbox(&mut server, &store, gmail_caps(), ib).await.unwrap();
        let rows = store.messages(ib, 10, 0).unwrap();
        let snippet = |subject: &str| rows.iter().find(|m| m.subject == subject).unwrap().snippet.clone();
        assert_eq!(snippet("Plain"), "Ganz einfach.");
        assert_eq!(snippet("QP"), "Grüße aus Köln");
        assert!(snippet("Base64").starts_with("Base64 kommt an. Erste Zeile"), "{}", snippet("Base64"));
        assert_eq!(snippet("HTML"), "Nur HTML hier");
        assert!(snippet("Lang").starts_with("Erste Zeile der langen Nachricht."));
        assert!(rows.iter().all(|m| !store.message(m.id).unwrap().has_body), "no body fetched");
        // All Mail's copies need none of their own.
        let all = folder(&store, account, "[Gmail]/Alle Nachrichten");
        server.log.clear();
        sync_mailbox(&mut server, &store, gmail_caps(), all).await.unwrap();
        assert!(!server.log.iter().any(|l| l.contains("BODY.PEEK[TEXT]")), "{:?}", server.log);
    }

    /// Gmail in German: the special folders below „[Gmail]“ by their attributes, a label with
    /// brackets as it is, and a user's own label „Entwürfe“ next to the real drafts is no second
    /// drafts folder.
    #[tokio::test]
    async fn gmail_folders_in_german() {
        let (mut server, store, account) = gmail_setup();
        sync_account(&mut server, &store, account).await.unwrap();
        let boxes = store.mailboxes(account).unwrap();
        let role = |r: Role| boxes.iter().filter(|m| m.role == Some(r)).map(|m| m.name.as_str()).collect::<Vec<_>>();
        assert_eq!(role(Role::Drafts), ["[Gmail]/Entwürfe"]);
        assert_eq!(role(Role::Sent), ["[Gmail]/Gesendet"]);
        assert_eq!(role(Role::Trash), ["[Gmail]/Papierkorb"]);
        assert_eq!(role(Role::Junk), ["[Gmail]/Spam"]);
        assert_eq!(role(Role::All), ["[Gmail]/Alle Nachrichten"]);
        assert_eq!(role(Role::Flagged), ["[Gmail]/Markiert"]);
        assert_eq!(role(Role::Important), ["[Gmail]/Wichtig"]);
        let label = |n: &str| boxes.iter().find(|m| m.name == n).unwrap_or_else(|| panic!("{n}: {boxes:?}"));
        assert_eq!(label("[Notion]").role, None);
        assert_eq!(label("Entwürfe").role, None, "the user's label");
        assert!(!label("[Gmail]").selectable);
    }

    /// A 1×1 PNG, base64.
    const PNG: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

    fn html_with_picture(id: &str, src: &str, part_header: &str, extra: &str) -> Vec<u8> {
        format!(
            "From: Lea <lea@example.org>\r\nTo: maria@example.org\r\nSubject: Bild {id}\r\nMessage-ID: <{id}@x>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"M\"\r\n\r\n\
--M\r\nContent-Type: multipart/related; boundary=\"R\"\r\n\r\n\
--R\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hier ist es: <img src=\"{src}\" alt=\"Bild\"></p>\r\n\
--R\r\n{part_header}Content-Transfer-Encoding: base64\r\n\r\n{PNG}\r\n--R--\r\n{extra}--M--\r\n"
        )
        .into_bytes()
    }

    /// Pictures inside a message always show: Gmail's inline ones, ones a mailer marks as an
    /// attachment or as an octet stream, `cid:` addresses in another case or %-escaped, and in a
    /// large message fetched part by part. Remote pictures the user allowed load over https.
    #[tokio::test]
    async fn pictures_inside_messages_always_show() {
        use crate::settings::RemoteContent;
        let (mut server, store, account) = gmail_setup();
        let big = format!(
            "--M\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"gross.zip\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{}\r\n",
            "QUJD".repeat((FULL_FETCH_LIMIT as usize) / 3)
        );
        let cases = [
            (
                "gmail",
                html_with_picture(
                    "gmail",
                    "cid:ii_kx1",
                    "Content-Type: image/png; name=\"image.png\"\r\nContent-Disposition: inline; filename=\"image.png\"\r\n\
Content-ID: <ii_kx1>\r\nX-Attachment-Id: ii_kx1\r\n",
                    "",
                ),
            ),
            (
                "octet",
                html_with_picture(
                    "octet",
                    "CID:Logo%40shop.example",
                    "Content-Type: application/octet-stream; name=\"logo\"\r\n\
Content-Disposition: attachment; filename=\"logo\"\r\nContent-ID: <Logo@shop.example>\r\n",
                    "",
                ),
            ),
            (
                "large",
                html_with_picture(
                    "large",
                    "cid:chart@x",
                    "Content-Type: image/png\r\nContent-Disposition: attachment; filename=\"chart.png\"\r\n\
Content-ID: <chart@x>\r\n",
                    &big,
                ),
            ),
        ];
        for (_, raw) in &cases {
            server.deliver_gmail(raw, 1_760_000_000, &["INBOX"]);
        }
        sync_account(&mut server, &store, account).await.unwrap();
        let ib = folder(&store, account, "INBOX");
        for (name, _) in &cases {
            let id = store
                .messages(ib, 10, 0)
                .unwrap()
                .into_iter()
                .find(|m| m.subject == format!("Bild {name}"))
                .unwrap()
                .id;
            fetch_body(&mut server, &store, id).await.unwrap();
            let view = crate::actions::message_view(&store, id, RemoteContent::Ask).unwrap();
            assert!(view.body.html.contains("src=\"data:image/png;base64,iVBORw0KGgo"), "{name}: {}", view.body.html);
            assert_eq!(view.body.blocked_remote, 0, "{name}");
        }
        let large = store.messages(ib, 10, 0).unwrap().into_iter().find(|m| m.subject == "Bild large").unwrap().id;
        assert!(store.raw_message(large).unwrap().is_none(), "fetched in parts");

        // Remote pictures: blocked until allowed, then over https (the app loads no plain http).
        let remote = "From: Shop <news@shop.example>\r\nTo: maria@example.org\r\nSubject: Remote\r\nMessage-ID: <r@x>\r\n\
MIME-Version: 1.0\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
<p><img src=\"http://img.shop.example/a.jpg\" width=\"300\"><img src=\"//cdn.shop.example/b.png\" width=\"300\">\
<img src=\"https://cdn.shop.example/c.png\" width=\"300\"></p>\r\n";
        server.deliver_gmail(remote.as_bytes(), 1_760_000_100, &["INBOX"]);
        sync_mailbox(&mut server, &store, gmail_caps(), ib).await.unwrap();
        let id = store.messages(ib, 1, 0).unwrap()[0].id;
        fetch_body(&mut server, &store, id).await.unwrap();
        let blocked = crate::actions::message_view(&store, id, RemoteContent::Ask).unwrap();
        assert_eq!(blocked.body.blocked_remote, 3);
        store.allow_remote(id).unwrap();
        let shown = crate::actions::message_view(&store, id, RemoteContent::Ask).unwrap().body.html;
        for url in
            ["https://img.shop.example/a.jpg", "https://cdn.shop.example/b.png", "https://cdn.shop.example/c.png"]
        {
            assert!(shown.contains(&format!("src=\"{url}\"")), "{url}: {shown}");
        }
        assert!(!shown.contains("http:"), "{shown}");
    }

    /// Failed rounds wait longer each time, also when connecting works (a folder or the
    /// server failing in the middle of a round).
    #[test]
    fn repeated_sync_failures_wait_longer() {
        let mut m = SyncMachine::default();
        m.on(Event::Start);
        let mut waits = vec![];
        for _ in 0..4 {
            m.on(Event::Connected { caps: all_caps() });
            let a = m.on(Event::SyncFailed);
            waits.extend(a.into_iter().filter_map(|a| if let Action::Wait(d) = a { Some(d.as_secs()) } else { None }));
            m.on(Event::Timer);
        }
        assert_eq!(waits, [5, 10, 20, 40]);
        m.on(Event::Connected { caps: all_caps() });
        m.on(Event::SyncDone);
        m.on(Event::Idle(IdleOutcome::Changed));
        assert_eq!(
            m.on(Event::SyncFailed),
            vec![Action::Disconnect, Action::Wait(Duration::from_secs(5))],
            "a good round resets it"
        );
    }
}
