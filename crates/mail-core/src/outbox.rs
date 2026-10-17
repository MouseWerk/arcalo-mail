//! The offline queue: what the user did (send, flags, move, delete, drafts, folders) is applied
//! locally at once and replayed to the server in order when it is reachable. A step that
//! failed on the way (no connection) waits longer each time ([`backoff`]) and keeps its place;
//! a step the server refused is parked ([`park`]) with its error until the user retries or
//! discards it. Nothing is dropped silently.
//!
//! Two lanes per account: sending (SMTP) and everything else (IMAP). Each keeps its order, but
//! a message the SMTP server does not take holds up no flag, move or folder change, and the
//! other way round ([`next_due`]).
//!
//! A worker claims a step before it runs it ([`start`]) and renews the claim while it runs
//! ([`renew`]); no other worker of the account (a new one after a settings change, the phone's
//! background check) takes a step whose claim is alive, so nothing is sent twice.
//!
//! Undo: moves, deletions and sending wait a few seconds before they are due
//! ([`enqueue_at`]); while a step has not started, [`cancel`] takes it back.

use std::sync::OnceLock;
use std::time::Duration;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::{Store, now};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Op {
    /// A finished message (RFC 5322 bytes, base64 in the payload) to send, then to file in Sent.
    Send {
        raw_b64: String,
        /// The envelope: the sender and every recipient (To, Cc, Bcc).
        #[serde(default)]
        from: String,
        #[serde(default)]
        recipients: Vec<String>,
        /// The copy for Sent (with Bcc), when the server does not file sent mail itself.
        #[serde(default)]
        sent_b64: Option<String>,
        #[serde(default)]
        sent_mailbox: Option<String>,
        /// The Message-ID of the draft this message came from (removed from Drafts).
        #[serde(default)]
        draft: Option<String>,
        /// The message answered (folder, UID): gets `\Answered`.
        #[serde(default)]
        answered: Option<(String, u32)>,
        /// Copies of an encrypted message for its Bcc recipients, each encrypted for its
        /// recipient alone; sent after the message, one by one.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        copies: Vec<Copy>,
    },
    /// The next Bcc copy of an encrypted message that went out ([`Op::Send`]'s `copies`), then
    /// the rest; filed in Sent as the message's step would be.
    SendCopy {
        from: String,
        copy: Copy,
        rest: Vec<Copy>,
        sent_b64: Option<String>,
        sent_mailbox: Option<String>,
        draft: Option<String>,
        answered: Option<(String, u32)>,
    },
    /// The second half of [`Op::Send`] after the message went out: never sent twice.
    AppendSent {
        mailbox: Option<String>,
        raw_b64: Option<String>,
        draft: Option<String>,
        answered: Option<(String, u32)>,
    },
    Flags {
        mailbox: String,
        uids: Vec<u32>,
        seen: Option<bool>,
        flagged: Option<bool>,
    },
    /// An IMAP keyword (a label) set or taken off.
    Keyword {
        mailbox: String,
        uids: Vec<u32>,
        keyword: String,
        add: bool,
    },
    Move {
        from: String,
        to: String,
        uids: Vec<u32>,
        /// The local rows, hidden until the server moved them.
        #[serde(default)]
        ids: Vec<i64>,
    },
    /// Removes messages for good (in the trash, or a server without one).
    Delete {
        mailbox: String,
        uids: Vec<u32>,
        #[serde(default)]
        ids: Vec<i64>,
    },
    /// Replaces the copy of a draft in Drafts (by its Message-ID).
    SaveDraft {
        mailbox: String,
        raw_b64: String,
        message_id: String,
    },
    DeleteDraft {
        mailbox: String,
        message_id: String,
    },
    CreateFolder {
        name: String,
    },
    RenameFolder {
        from: String,
        to: String,
    },
    DeleteFolder {
        name: String,
    },
}

impl Op {
    pub fn kind(&self) -> &'static str {
        match self {
            // A copy is sending too: the same lane, the same name in the list.
            Op::Send { .. } | Op::SendCopy { .. } => "send",
            Op::AppendSent { .. } => "append_sent",
            Op::Flags { .. } => "flags",
            Op::Keyword { .. } => "keyword",
            Op::Move { .. } => "move",
            Op::Delete { .. } => "delete",
            Op::SaveDraft { .. } => "save_draft",
            Op::DeleteDraft { .. } => "delete_draft",
            Op::CreateFolder { .. } => "create_folder",
            Op::RenameFolder { .. } => "rename_folder",
            Op::DeleteFolder { .. } => "delete_folder",
        }
    }
}

/// A copy of a message with its own envelope (an encrypted message's copy for a Bcc recipient).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copy {
    pub raw_b64: String,
    pub recipients: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Queued {
    pub id: i64,
    pub account_id: i64,
    pub op: Op,
    pub attempts: u32,
    pub last_error: Option<String>,
    /// The UIDVALIDITY of the folder of the step's UIDs ([`Op::uid_folder`]) when it was queued.
    pub uid_validity: Option<u32>,
}

impl Op {
    /// The folder whose UIDs the step names, if any.
    pub fn uid_folder(&self) -> Option<&str> {
        match self {
            Op::Flags { mailbox, .. } | Op::Keyword { mailbox, .. } | Op::Delete { mailbox, .. } => Some(mailbox),
            Op::Move { from, .. } => Some(from),
            Op::Send { answered, .. } | Op::SendCopy { answered, .. } | Op::AppendSent { answered, .. } => {
                answered.as_ref().map(|(m, _)| m.as_str())
            }
            _ => None,
        }
    }
}

/// How long a claimed step stays its worker's without a sign of life ([`renew`]).
pub const LEASE_SECS: i64 = 120;
/// How often a worker renews the claim of the step it runs.
pub const LEASE_RENEW: Duration = Duration::from_secs(30);

/// This process, as the owner of the steps its workers run.
fn process() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{}-{nanos}", std::process::id())
    })
}

/// A step as the UI lists it (the payload stays in the backend).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub id: i64,
    pub account_id: i64,
    pub kind: String,
    /// `queued`, `running`, `failed` (will retry), `parked` (refused, waits for the user).
    pub state: String,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub next_try: i64,
}

/// Seconds to wait after the `attempts`-th failure: 30 s, doubling, at most an hour.
pub fn backoff(attempts: u32) -> i64 {
    (30i64 << attempts.saturating_sub(1).min(7)).min(3600)
}

pub fn enqueue(store: &Store, account_id: i64, op: &Op) -> Result<i64> {
    enqueue_at(store, account_id, op, now())
}

/// Queues a step that is due at `at` (later than now: the undo window). The UIDVALIDITY of the
/// folder its UIDs belong to goes with it.
pub fn enqueue_at(store: &Store, account_id: i64, op: &Op, at: i64) -> Result<i64> {
    let uid_validity: Option<u32> = match op.uid_folder() {
        Some(name) => store
            .conn()
            .query_row(
                "SELECT uid_validity FROM mailboxes WHERE account_id = ?1 AND name = ?2",
                params![account_id, name],
                |r| r.get(0),
            )
            .optional()?
            .flatten(),
        None => None,
    };
    store.conn().execute(
        "INSERT INTO outbox (account_id, kind, payload, next_try, created_at, uid_validity)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![account_id, op.kind(), serde_json::to_string(op)?, at, now(), uid_validity],
    )?;
    Ok(store.conn().last_insert_rowid())
}

/// A step that still waits for its turn (without its payload, which can be large).
struct Waiting {
    id: i64,
    next_try: i64,
    /// Claimed by a worker that is alive.
    held: bool,
    /// Nothing older in its lane holds it up.
    head: bool,
}

/// The steps of an account that wait for their turn, oldest first. Parked steps are left out.
fn waiting(store: &Store, account_id: i64, at: i64) -> Result<Vec<Waiting>> {
    let mut st = store.conn().prepare_cached(
        "SELECT id, kind, next_try, state = 'running' AND lease_until IS NOT NULL AND lease_until >= ?2
         FROM outbox WHERE account_id = ?1 AND state <> 'parked' ORDER BY id",
    )?;
    let rows = st.query_map(params![account_id, at], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, bool>(3)?))
    })?;
    let (mut imap_busy, mut send_busy) = (false, false);
    let mut out = vec![];
    for row in rows {
        let (id, kind, next_try, held) = row?;
        let sending = kind == "send" || kind == "append_sent";
        // Filing a sent message also waits for the IMAP steps before it (the draft it
        // replaces may still be on its way to Drafts).
        let head = match kind.as_str() {
            "append_sent" => !send_busy && !imap_busy,
            _ if sending => !send_busy,
            _ => !imap_busy,
        };
        if sending {
            send_busy = true;
        } else {
            imap_busy = true;
        }
        out.push(Waiting { id, next_try, held, head });
    }
    Ok(out)
}

/// The oldest step of an account that may run at `at`. Each lane runs strictly in order: while
/// its oldest step waits (or runs), the later ones wait too (a move must not overtake the flag
/// change before it); the other lane goes on. Parked steps are left out.
pub fn next_due(store: &Store, account_id: i64, at: i64) -> Result<Option<Queued>> {
    let Some(w) = waiting(store, account_id, at)?.into_iter().find(|w| w.head && !w.held && w.next_try <= at) else {
        return Ok(None);
    };
    let (payload, attempts, last_error, uid_validity) = store.conn().query_row(
        "SELECT payload, attempts, last_error, uid_validity FROM outbox WHERE id = ?1",
        [w.id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<u32>>(3)?,
            ))
        },
    )?;
    Ok(Some(Queued { id: w.id, account_id, op: serde_json::from_str(&payload)?, attempts, last_error, uid_validity }))
}

/// When the next step of an account is due (to wake the worker in time), if any.
pub fn next_due_at(store: &Store, account_id: i64) -> Result<Option<i64>> {
    Ok(waiting(store, account_id, now())?.into_iter().filter(|w| w.head && !w.held).map(|w| w.next_try).min())
}

/// Claims a step for this worker before it runs: from now on it cannot be taken back. `false`
/// when it is gone (taken back a moment ago) or another worker runs it.
pub fn start(store: &Store, id: i64) -> Result<bool> {
    let at = now();
    let n = store.conn().execute(
        "UPDATE outbox SET state = 'running', owner = ?2, lease_until = ?3
         WHERE id = ?1 AND state <> 'parked'
           AND (state <> 'running' OR lease_until IS NULL OR lease_until < ?4)",
        params![id, process(), at + LEASE_SECS, at],
    )?;
    Ok(n == 1)
}

/// The worker running a step is still at it.
pub fn renew(store: &Store, id: i64) -> Result<()> {
    store.conn().execute(
        "UPDATE outbox SET lease_until = ?2 WHERE id = ?1 AND state = 'running'",
        params![id, now() + LEASE_SECS],
    )?;
    Ok(())
}

/// Steps left running by an earlier run of the app (it crashed or was closed mid-step): they
/// run again. Called once when the app starts; steps of this process stay with their workers.
pub fn recover(store: &Store) -> Result<()> {
    store.conn().execute(
        "UPDATE outbox SET state = 'queued', owner = NULL, lease_until = NULL
         WHERE state = 'running' AND (owner IS NULL OR owner <> ?1)",
        [process()],
    )?;
    Ok(())
}

pub fn done(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("DELETE FROM outbox WHERE id = ?1", [id])?;
    Ok(())
}

/// Replaces a step by what is left of it (sent, now to be filed); a running step stays its
/// worker's.
pub fn replace(store: &Store, id: i64, op: &Op) -> Result<()> {
    store.conn().execute(
        "UPDATE outbox SET kind = ?2, payload = ?3 WHERE id = ?1",
        params![id, op.kind(), serde_json::to_string(op)?],
    )?;
    Ok(())
}

/// The step failed on the way (no connection, a timeout): it keeps its place and waits.
pub fn failed(store: &Store, id: i64, error: &str, at: i64) -> Result<()> {
    let attempts: u32 = store.conn().query_row("SELECT attempts + 1 FROM outbox WHERE id = ?1", [id], |r| r.get(0))?;
    store.conn().execute(
        "UPDATE outbox SET attempts = ?2, last_error = ?3, state = 'failed', next_try = ?4, owner = NULL,
           lease_until = NULL WHERE id = ?1",
        params![id, attempts, error, at + backoff(attempts)],
    )?;
    Ok(())
}

/// The server refused the step: it waits for the user („Erneut versuchen“ or discard), the
/// steps after it go on.
pub fn park(store: &Store, id: i64, error: &str) -> Result<()> {
    store.conn().execute(
        "UPDATE outbox SET attempts = attempts + 1, last_error = ?2, state = 'parked', owner = NULL,
           lease_until = NULL WHERE id = ?1",
        params![id, error],
    )?;
    Ok(())
}

/// „Erneut versuchen“: a parked or waiting step is due now (one a worker runs stays with it).
pub fn retry(store: &Store, id: i64) -> Result<()> {
    store.conn().execute(
        "UPDATE outbox SET state = 'queued', next_try = ?2, owner = NULL, lease_until = NULL
         WHERE id = ?1 AND (state <> 'running' OR lease_until IS NULL OR lease_until < ?2)",
        params![id, now()],
    )?;
    Ok(())
}

/// Takes a step back while it has not started (undo); `None` when it is gone or running.
pub fn cancel(store: &Store, id: i64) -> Result<Option<Op>> {
    // One statement: a worker that claims the step at the same moment either gets it (and the
    // undo says too late) or finds it gone.
    let payload: Option<String> = store
        .conn()
        .query_row("DELETE FROM outbox WHERE id = ?1 AND state <> 'running' RETURNING payload", [id], |r| r.get(0))
        .optional()?;
    payload.map(|p| serde_json::from_str(&p).map_err(Into::into)).transpose()
}

/// The UIDs of a folder whose flag or keyword change waits for the server (a parked one, which
/// the server refused, does not count): a sync leaves their flags as the user set them.
pub fn pending_flag_uids(store: &Store, account_id: i64, mailbox: &str) -> Result<std::collections::HashSet<u32>> {
    let mut st = store.conn().prepare_cached(
        "SELECT payload FROM outbox WHERE account_id = ?1 AND kind IN ('flags', 'keyword') AND state <> 'parked'",
    )?;
    let mut out = std::collections::HashSet::new();
    for payload in st.query_map([account_id], |r| r.get::<_, String>(0))? {
        match serde_json::from_str(&payload?)? {
            Op::Flags { mailbox: m, uids, .. } | Op::Keyword { mailbox: m, uids, .. } if m == mailbox => {
                out.extend(uids)
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Folder changes that wait for the server: the names that exist here only so far (created,
/// renamed to), and the names that are gone here only (renamed from, deleted).
pub fn pending_folders(store: &Store, account_id: i64) -> Result<(Vec<String>, Vec<String>)> {
    let mut st = store.conn().prepare_cached(
        "SELECT payload FROM outbox WHERE account_id = ?1
           AND kind IN ('create_folder', 'rename_folder', 'delete_folder') ORDER BY id",
    )?;
    let (mut waiting, mut leaving) = (vec![], vec![]);
    for payload in st.query_map([account_id], |r| r.get::<_, String>(0))? {
        match serde_json::from_str(&payload?)? {
            Op::CreateFolder { name } => waiting.push(name),
            Op::RenameFolder { from, to } => {
                leaving.push(from);
                waiting.push(to);
            }
            Op::DeleteFolder { name } => leaving.push(name),
            _ => {}
        }
    }
    Ok((waiting, leaving))
}

/// Messages being sent at `at`: a send that started, or one that is due within the next half
/// minute (its undo window is running out). A send that failed and waits for the server does not
/// count. An update waits for these before it installs, so a quit does not cut one off.
pub fn sending(store: &Store, at: i64) -> Result<u32> {
    Ok(store.conn().query_row(
        "SELECT COUNT(*) FROM outbox WHERE kind = 'send' AND (state = 'running' OR (state = 'queued' AND next_try <= ?1))",
        [at + 30],
        |r| r.get(0),
    )?)
}

pub fn pending(store: &Store, account_id: i64) -> Result<u32> {
    Ok(store.conn().query_row("SELECT COUNT(*) FROM outbox WHERE account_id = ?1", [account_id], |r| r.get(0))?)
}

/// The steps of all accounts, oldest first.
pub fn entries(store: &Store) -> Result<Vec<Entry>> {
    let mut st = store
        .conn()
        .prepare("SELECT id, account_id, kind, state, attempts, last_error, next_try FROM outbox ORDER BY id")?;
    let rows = st.query_map([], |r| {
        Ok(Entry {
            id: r.get(0)?,
            account_id: r.get(1)?,
            kind: r.get(2)?,
            state: r.get(3)?,
            attempts: r.get(4)?,
            last_error: r.get(5)?,
            next_try: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::store_with_inbox;

    #[test]
    fn messages_being_sent_are_counted_until_they_are_out() {
        let (s, account, _) = store_with_inbox();
        let send = Op::Send {
            raw_b64: String::new(),
            from: "maria@example.org".into(),
            recipients: vec!["jonas@example.org".into()],
            sent_b64: None,
            sent_mailbox: None,
            draft: None,
            answered: None,
            copies: vec![],
        };
        let t = now();
        assert_eq!(sending(&s, t).unwrap(), 0);
        let soon = enqueue_at(&s, account, &send, t + 5).unwrap();
        let later = enqueue_at(&s, account, &send, t + 3600).unwrap();
        enqueue(&s, account, &Op::Flags { mailbox: "INBOX".into(), uids: vec![1], seen: Some(true), flagged: None })
            .unwrap();
        assert_eq!(sending(&s, t).unwrap(), 1, "the one in its undo window; not the one sent later, not a flag");
        assert!(start(&s, soon).unwrap());
        assert_eq!(sending(&s, t).unwrap(), 1, "running");
        failed(&s, soon, "offline", t).unwrap();
        assert_eq!(sending(&s, t).unwrap(), 0, "waiting for the server: no reason to hold an update");
        retry(&s, later).unwrap();
        assert_eq!(sending(&s, t).unwrap(), 1);
        done(&s, later).unwrap();
        assert_eq!(sending(&s, t).unwrap(), 0);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        assert_eq!(backoff(1), 30);
        assert_eq!(backoff(2), 60);
        assert_eq!(backoff(3), 120);
        assert_eq!(backoff(50), 3600);
    }

    #[test]
    fn steps_run_in_order_and_wait_after_a_failure() {
        let (s, account, _) = store_with_inbox();
        let flags = Op::Flags { mailbox: "INBOX".into(), uids: vec![1], seen: Some(true), flagged: None };
        let mv = Op::Move { from: "INBOX".into(), to: "Archive".into(), uids: vec![1], ids: vec![] };
        let a = enqueue(&s, account, &flags).unwrap();
        enqueue(&s, account, &mv).unwrap();
        let t = now();
        assert_eq!(next_due(&s, account, t).unwrap().unwrap().op, flags);
        failed(&s, a, "connection reset", t).unwrap();
        assert_eq!(next_due(&s, account, t).unwrap(), None, "the failed step blocks the later ones");
        let retry = next_due(&s, account, t + 30).unwrap().unwrap();
        assert_eq!((retry.attempts, retry.last_error.as_deref()), (1, Some("connection reset")));
        done(&s, a).unwrap();
        assert_eq!(next_due(&s, account, t + 30).unwrap().unwrap().op, mv);
        assert_eq!(pending(&s, account).unwrap(), 1);
    }

    #[test]
    fn refused_steps_are_parked_and_undo_takes_back_what_has_not_started() {
        let (s, account, _) = store_with_inbox();
        let t = now();
        let refused = enqueue(&s, account, &Op::CreateFolder { name: "X".into() }).unwrap();
        let later = enqueue_at(&s, account, &Op::DeleteFolder { name: "Y".into() }, t + 5).unwrap();
        park(&s, refused, "NO [ALREADYEXISTS]").unwrap();
        assert_eq!(next_due(&s, account, t).unwrap(), None, "the undo window");
        assert_eq!(next_due_at(&s, account).unwrap(), Some(t + 5));
        assert_eq!(next_due(&s, account, t + 5).unwrap().unwrap().id, later, "the parked step does not block");
        assert!(start(&s, later).unwrap());
        assert_eq!(cancel(&s, later).unwrap(), None, "running: too late to undo");
        recover(&s).unwrap();
        assert_eq!(cancel(&s, later).unwrap(), None, "a step this process runs stays running");
        // Left running by an earlier run of the app.
        s.conn().execute("UPDATE outbox SET owner = 'gone' WHERE id = ?1", [later]).unwrap();
        recover(&s).unwrap();
        assert!(cancel(&s, later).unwrap().is_some());
        let list = entries(&s).unwrap();
        assert_eq!((list.len(), list[0].state.as_str()), (1, "parked"));
        retry(&s, refused).unwrap();
        assert_eq!(next_due(&s, account, now()).unwrap().unwrap().id, refused);
        replace(&s, refused, &Op::DeleteFolder { name: "Z".into() }).unwrap();
        assert_eq!(next_due(&s, account, now()).unwrap().unwrap().op, Op::DeleteFolder { name: "Z".into() });
    }

    /// „Rückgängig“ between the worker picking a step and starting it: the step is gone and
    /// the worker leaves it; once started, undo is too late.
    #[test]
    fn undo_and_start_never_both_win() {
        let (s, account, _) = store_with_inbox();
        let id = enqueue(&s, account, &Op::CreateFolder { name: "X".into() }).unwrap();
        let q = next_due(&s, account, now()).unwrap().unwrap();
        assert!(cancel(&s, id).unwrap().is_some());
        assert!(!start(&s, q.id).unwrap(), "taken back: the worker must not run it");
        let id = enqueue(&s, account, &Op::CreateFolder { name: "Y".into() }).unwrap();
        assert!(start(&s, id).unwrap());
        assert_eq!(cancel(&s, id).unwrap(), None);
        assert!(!start(&s, id).unwrap(), "claimed once only");
    }

    /// A message the SMTP server does not take holds up no other change, and the other way
    /// round; filing a sent message waits for the IMAP steps before it.
    #[test]
    fn sending_and_the_other_changes_wait_in_lanes_of_their_own() {
        let (s, account, _) = store_with_inbox();
        let t = now();
        let send = Op::Send {
            raw_b64: String::new(),
            from: "maria@example.org".into(),
            recipients: vec!["jonas@example.org".into()],
            sent_b64: None,
            sent_mailbox: None,
            draft: None,
            answered: None,
            copies: vec![],
        };
        let flags = Op::Flags { mailbox: "INBOX".into(), uids: vec![1], seen: Some(true), flagged: None };
        let a = enqueue(&s, account, &send).unwrap();
        let b = enqueue(&s, account, &flags).unwrap();
        let c = enqueue(&s, account, &send).unwrap();
        failed(&s, a, "smtp down", t).unwrap();
        assert_eq!(next_due(&s, account, t).unwrap().unwrap().id, b, "the flag change goes on");
        assert!(start(&s, b).unwrap());
        assert_eq!(next_due(&s, account, t).unwrap(), None, "the later send waits for the earlier one");
        failed(&s, b, "imap down", t).unwrap();
        done(&s, a).unwrap();
        assert_eq!(next_due(&s, account, t).unwrap().unwrap().id, c, "sending goes on without IMAP");
        assert_eq!(next_due_at(&s, account).unwrap(), Some(t), "the send is due");
        let file = Op::AppendSent { mailbox: Some("Sent".into()), raw_b64: None, draft: None, answered: None };
        replace(&s, c, &file).unwrap();
        assert_eq!(next_due(&s, account, t).unwrap(), None, "filing waits for the IMAP step before it");
        done(&s, b).unwrap();
        assert_eq!(next_due(&s, account, t).unwrap().unwrap().id, c);
    }

    /// A step another worker runs is never taken; one whose worker is gone (its claim ran out)
    /// is.
    #[test]
    fn running_steps_stay_with_their_worker_while_it_lives() {
        let (s, account, _) = store_with_inbox();
        let id = enqueue(&s, account, &Op::CreateFolder { name: "X".into() }).unwrap();
        assert!(start(&s, id).unwrap());
        assert_eq!(next_due(&s, account, now()).unwrap(), None);
        assert!(!start(&s, id).unwrap());
        renew(&s, id).unwrap();
        assert_eq!(next_due(&s, account, now() + LEASE_SECS - 10).unwrap(), None, "renewed");
        s.conn().execute("UPDATE outbox SET lease_until = ?2 WHERE id = ?1", params![id, now() - 1]).unwrap();
        assert_eq!(next_due(&s, account, now()).unwrap().unwrap().id, id, "the claim ran out");
        assert!(start(&s, id).unwrap());
    }

    /// Steps remember the UIDVALIDITY of the folder their UIDs belong to.
    #[test]
    fn steps_remember_the_uidvalidity_of_their_uids() {
        let (s, account, inbox) = store_with_inbox();
        s.set_sync_state(
            inbox,
            crate::store::SyncState { uid_validity: Some(42), uid_next: Some(9), highest_modseq: None },
        )
        .unwrap();
        let flags = Op::Flags { mailbox: "INBOX".into(), uids: vec![1], seen: Some(true), flagged: None };
        enqueue(&s, account, &flags).unwrap();
        assert_eq!(next_due(&s, account, now()).unwrap().unwrap().uid_validity, Some(42));
        let folder = enqueue(&s, account, &Op::CreateFolder { name: "X".into() }).unwrap();
        done(&s, next_due(&s, account, now()).unwrap().unwrap().id).unwrap();
        let q = next_due(&s, account, now()).unwrap().unwrap();
        assert_eq!((q.id, q.uid_validity), (folder, None));
    }

    #[test]
    fn old_payloads_still_read() {
        let old = r#"{"kind":"send","raw_b64":"eA=="}"#;
        let op: Op = serde_json::from_str(old).unwrap();
        assert!(matches!(op, Op::Send { recipients, .. } if recipients.is_empty()));
        let old = r#"{"kind":"move","from":"INBOX","to":"Archiv","uids":[3]}"#;
        assert!(matches!(serde_json::from_str::<Op>(old).unwrap(), Op::Move { ids, .. } if ids.is_empty()));
    }
}
