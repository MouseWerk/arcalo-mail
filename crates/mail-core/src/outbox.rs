//! The offline queue: what the user did without a connection (send, flags, move, delete) is
//! applied locally at once and replayed to the server in order when it is reachable. A failed
//! step waits longer each time ([`backoff`]) and stays visible with its error; nothing is
//! dropped silently.

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
    },
    Delete {
        mailbox: String,
        uids: Vec<u32>,
    },
}

impl Op {
    fn kind(&self) -> &'static str {
        match self {
            Op::Send { .. } => "send",
            Op::Flags { .. } => "flags",
            Op::Move { .. } => "move",
            Op::Delete { .. } => "delete",
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

/// Seconds to wait after the `attempts`-th failure: 30 s, doubling, at most an hour.
pub fn backoff(attempts: u32) -> i64 {
    (30i64 << attempts.saturating_sub(1).min(7)).min(3600)
}

pub fn enqueue(store: &Store, account_id: i64, op: &Op) -> Result<i64> {
    let t = now();
    store.conn().execute(
        "INSERT INTO outbox (account_id, kind, payload, next_try, created_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![account_id, op.kind(), serde_json::to_string(op)?, t],
    )?;
    Ok(store.conn().last_insert_rowid())
}

/// The oldest step of an account that is due at `at`. Steps run strictly in order: while the
/// oldest one waits, the later ones wait too (a move must not overtake the flag change before it).
pub fn next_due(store: &Store, account_id: i64, at: i64) -> Result<Option<Queued>> {
    let row = store
        .conn()
        .query_row(
            "SELECT id, payload, attempts, last_error, next_try FROM outbox WHERE account_id = ?1 ORDER BY id LIMIT 1",
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

pub fn done(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("DELETE FROM outbox WHERE id = ?1", [id])?;
    Ok(())
}

pub fn failed(store: &Store, id: i64, error: &str, at: i64) -> Result<()> {
    let attempts: u32 = store.conn().query_row("SELECT attempts + 1 FROM outbox WHERE id = ?1", [id], |r| r.get(0))?;
    store.conn().execute(
        "UPDATE outbox SET attempts = ?2, last_error = ?3, state = 'failed', next_try = ?4 WHERE id = ?1",
        params![id, attempts, error, at + backoff(attempts)],
    )?;
    Ok(())
}

pub fn pending(store: &Store, account_id: i64) -> Result<u32> {
    Ok(store.conn().query_row("SELECT COUNT(*) FROM outbox WHERE account_id = ?1", [account_id], |r| r.get(0))?)
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
        let mv = Op::Move { from: "INBOX".into(), to: "Archive".into(), uids: vec![1] };
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
}
