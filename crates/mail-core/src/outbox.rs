//! The offline queue: what the user did (send, flags, move, delete, drafts, folders) is applied
//! locally at once and replayed to the server in order when it is reachable. A step that
//! failed on the way (no connection) waits longer each time ([`backoff`]) and keeps its place;
//! a step the server refused is parked ([`park`]) with its error until the user retries or
//! discards it. Nothing is dropped silently.
//!
//! Undo: moves, deletions and sending wait a few seconds before they are due
//! ([`enqueue_at`]); while a step has not started, [`cancel`] takes it back.

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
            Op::Send { .. } => "send",
            Op::AppendSent { .. } => "append_sent",
            Op::Flags { .. } => "flags",
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Queued {
    pub id: i64,
    pub account_id: i64,
    pub op: Op,
    pub attempts: u32,
    pub last_error: Option<String>,
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

/// Queues a step that is due at `at` (later than now: the undo window).
pub fn enqueue_at(store: &Store, account_id: i64, op: &Op, at: i64) -> Result<i64> {
    store.conn().execute(
        "INSERT INTO outbox (account_id, kind, payload, next_try, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![account_id, op.kind(), serde_json::to_string(op)?, at, now()],
    )?;
    Ok(store.conn().last_insert_rowid())
}

/// The oldest step of an account that is due at `at`. Steps run strictly in order: while the
/// oldest one waits, the later ones wait too (a move must not overtake the flag change before it).
/// Parked steps are left out.
pub fn next_due(store: &Store, account_id: i64, at: i64) -> Result<Option<Queued>> {
    let row = store
        .conn()
        .query_row(
            "SELECT id, payload, attempts, last_error, next_try FROM outbox
             WHERE account_id = ?1 AND state <> 'parked' ORDER BY id LIMIT 1",
            [account_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u32>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((id, payload, attempts, last_error, next_try)) = row else { return Ok(None) };
    if next_try > at {
        return Ok(None);
    }
    Ok(Some(Queued { id, account_id, op: serde_json::from_str(&payload)?, attempts, last_error }))
}

/// When the next step of an account is due (to wake the worker in time), if any.
pub fn next_due_at(store: &Store, account_id: i64) -> Result<Option<i64>> {
    Ok(store
        .conn()
        .query_row(
            "SELECT next_try FROM outbox WHERE account_id = ?1 AND state <> 'parked' ORDER BY id LIMIT 1",
            [account_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Marks a step as started: from now on it cannot be taken back.
pub fn start(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("UPDATE outbox SET state = 'running' WHERE id = ?1", [id])?;
    Ok(())
}

/// Steps left running by a crash: they run again.
pub fn recover(store: &Store) -> Result<()> {
    store.conn().execute("UPDATE outbox SET state = 'queued' WHERE state = 'running'", [])?;
    Ok(())
}

pub fn done(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("DELETE FROM outbox WHERE id = ?1", [id])?;
    Ok(())
}

/// Replaces a step by what is left of it (sent, now to be filed).
pub fn replace(store: &Store, id: i64, op: &Op) -> Result<()> {
    store.conn().execute(
        "UPDATE outbox SET kind = ?2, payload = ?3, state = 'queued' WHERE id = ?1",
        params![id, op.kind(), serde_json::to_string(op)?],
    )?;
    Ok(())
}

/// The step failed on the way (no connection, a timeout): it keeps its place and waits.
pub fn failed(store: &Store, id: i64, error: &str, at: i64) -> Result<()> {
    let attempts: u32 = store.conn().query_row("SELECT attempts + 1 FROM outbox WHERE id = ?1", [id], |r| r.get(0))?;
    store.conn().execute(
        "UPDATE outbox SET attempts = ?2, last_error = ?3, state = 'failed', next_try = ?4 WHERE id = ?1",
        params![id, attempts, error, at + backoff(attempts)],
    )?;
    Ok(())
}

/// The server refused the step: it waits for the user („Erneut versuchen“ or discard), the
/// steps after it go on.
pub fn park(store: &Store, id: i64, error: &str) -> Result<()> {
    store.conn().execute(
        "UPDATE outbox SET attempts = attempts + 1, last_error = ?2, state = 'parked' WHERE id = ?1",
        params![id, error],
    )?;
    Ok(())
}

/// „Erneut versuchen“: a parked or waiting step is due now.
pub fn retry(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("UPDATE outbox SET state = 'queued', next_try = ?2 WHERE id = ?1", params![id, now()])?;
    Ok(())
}

/// Takes a step back while it has not started (undo); `None` when it is gone or running.
pub fn cancel(store: &Store, id: i64) -> Result<Option<Op>> {
    let row: Option<String> = store
        .conn()
        .query_row("SELECT payload FROM outbox WHERE id = ?1 AND state <> 'running'", [id], |r| r.get(0))
        .optional()?;
    let Some(payload) = row else { return Ok(None) };
    store.conn().execute("DELETE FROM outbox WHERE id = ?1 AND state <> 'running'", [id])?;
    Ok(Some(serde_json::from_str(&payload)?))
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
        start(&s, later).unwrap();
        assert_eq!(cancel(&s, later).unwrap(), None, "running: too late to undo");
        recover(&s).unwrap();
        assert!(cancel(&s, later).unwrap().is_some());
        let list = entries(&s).unwrap();
        assert_eq!((list.len(), list[0].state.as_str()), (1, "parked"));
        retry(&s, refused).unwrap();
        assert_eq!(next_due(&s, account, now()).unwrap().unwrap().id, refused);
        replace(&s, refused, &Op::DeleteFolder { name: "Z".into() }).unwrap();
        assert_eq!(next_due(&s, account, now()).unwrap().unwrap().op, Op::DeleteFolder { name: "Z".into() });
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
