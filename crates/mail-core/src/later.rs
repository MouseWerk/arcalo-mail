//! Time tools: snooze, send later, follow-up reminders, pin and read later.
//!
//! - **Snooze** takes a message out of the inbox until a time. A message with a Message-ID waits
//!   on the server in the folder [`SNOOZE_FOLDER`] (other clients see it out of the inbox too)
//!   and moves back when due, unread; one without waits on this device only
//!   (`messages.snoozed_until`). Back in the inbox it sorts as if it arrived then
//!   (`messages.bumped_at`), so it shows at the top.
//! - **Send later** keeps the finished message here (`scheduled`) and hands it to the offline
//!   queue when due.
//! - **Follow-up reminders** („Erinnern, wenn keine Antwort“) come due unless an answer to the
//!   message (In-Reply-To or References) arrived from someone else by then.
//! - **Pin** and **read later** are marks by Message-ID (`marks`), kept when a message moves.
//!
//! Nothing here waits by itself: [`tick`] does what is due. The desktop app calls it every few
//! seconds while it runs and once at its start (a message whose time passed while the computer
//! was off goes out then); a phone calls it while the app is in front and in its background
//! check.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::actions;
use crate::compose::{Draft, FileAttachment};
use crate::error::{Error, Result};
use crate::imap::Role;
use crate::outbox::{self, Op};
use crate::store::{MessageSummary, SUMMARY_COLS, Store, now, summary};
use crate::tr;

/// Where snoozed messages wait on the server.
pub const SNOOZE_FOLDER: &str = "Snoozed";

/// The key of a message for marks, snoozes and reminders: its Message-ID, else its row.
fn key_of(message_id: Option<&str>, id: i64) -> String {
    match message_id {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => format!("local:{id}"),
    }
}

/// The SQL of [`key_of`] for the messages table `m`.
const KEY_SQL: &str = "COALESCE(m.message_id, 'local:' || m.id)";

struct Row {
    id: i64,
    account_id: i64,
    mailbox_id: i64,
    message_id: Option<String>,
    subject: String,
}

fn rows(store: &Store, ids: &[i64]) -> Result<Vec<Row>> {
    let mut st =
        store.conn().prepare("SELECT id, account_id, mailbox_id, message_id, subject FROM messages WHERE id = ?1")?;
    let mut out = vec![];
    for id in ids {
        if let Some(r) = st
            .query_row([id], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    mailbox_id: r.get(2)?,
                    message_id: r.get(3)?,
                    subject: r.get(4)?,
                })
            })
            .optional()?
        {
            out.push(r);
        }
    }
    Ok(out)
}

fn accounts_of(rows: &[Row]) -> Vec<i64> {
    let mut a: Vec<i64> = rows.iter().map(|r| r.account_id).collect();
    a.sort_unstable();
    a.dedup();
    a
}

/// What the reading pane shows about a message's marks, snooze and reminder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Marks {
    pub pinned: bool,
    pub later: bool,
    pub snoozed_until: Option<i64>,
    pub followup_due: Option<i64>,
}

pub fn marks_of(store: &Store, id: i64) -> Result<Marks> {
    let Some(r) = rows(store, &[id])?.pop() else { return Ok(Marks::default()) };
    let key = key_of(r.message_id.as_deref(), r.id);
    let (pinned, later) = store
        .conn()
        .query_row(
            "SELECT pinned, later_at FROM marks WHERE account_id = ?1 AND message_key = ?2",
            params![r.account_id, key],
            |x| Ok((x.get::<_, bool>(0)?, x.get::<_, Option<i64>>(1)?.is_some())),
        )
        .optional()?
        .unwrap_or_default();
    let snoozed_until = store
        .conn()
        .query_row(
            "SELECT until FROM snoozes WHERE account_id = ?1 AND message_key = ?2 AND state = 'waiting'",
            params![r.account_id, key],
            |x| x.get(0),
        )
        .optional()?;
    let followup_due = store
        .conn()
        .query_row(
            "SELECT due_at FROM followups WHERE account_id = ?1 AND message_key = ?2 AND state = 'waiting'",
            params![r.account_id, key],
            |x| x.get(0),
        )
        .optional()?;
    Ok(Marks { pinned, later, snoozed_until, followup_due })
}

// ---------------------------------------------------------------------------- pin and read later

/// Pins messages (they stay at the top of their inbox tab and pass Focus) or takes the pin off.
pub fn set_pinned(store: &Store, ids: &[i64], on: bool) -> Result<()> {
    for r in rows(store, ids)? {
        store.conn().execute(
            "INSERT INTO marks (account_id, message_key, pinned) VALUES (?1, ?2, ?3)
             ON CONFLICT (account_id, message_key) DO UPDATE SET pinned = excluded.pinned",
            params![r.account_id, key_of(r.message_id.as_deref(), r.id), on],
        )?;
    }
    Ok(())
}

/// Puts messages on the read later list or takes them off.
pub fn set_later(store: &Store, ids: &[i64], on: bool) -> Result<()> {
    let at = on.then(now);
    for r in rows(store, ids)? {
        store.conn().execute(
            "INSERT INTO marks (account_id, message_key, later_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (account_id, message_key) DO UPDATE SET later_at = excluded.later_at",
            params![r.account_id, key_of(r.message_id.as_deref(), r.id), at],
        )?;
    }
    Ok(())
}

/// Rows of a list made from a table that names messages by key: `{from}` joins `m` (messages)
/// with a time `t` and a group `g`; one copy per group, the visible one first.
fn keyed_list(store: &Store, from: &str, order: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<MessageSummary>> {
    let sql = format!(
        "SELECT {SUMMARY_COLS}, t, MIN(hidden) FROM (SELECT m.*, x.t AS t, x.g AS g {from}) GROUP BY g ORDER BY {order}"
    );
    let mut st = store.conn().prepare(&sql)?;
    let rows = st.query_map(args, |r| {
        let mut s = summary(r)?;
        s.due = r.get(14)?;
        Ok(s)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The read later list, the latest added first.
pub fn later_list(store: &Store) -> Result<Vec<MessageSummary>> {
    keyed_list(
        store,
        &format!(
            "FROM (SELECT account_id, message_key, later_at AS t, account_id || ' ' || message_key AS g FROM marks
               WHERE later_at IS NOT NULL) x
             JOIN messages m ON m.account_id = x.account_id AND {KEY_SQL} = x.message_key
             JOIN mailboxes b ON b.id = m.mailbox_id WHERE m.hidden = 0 AND COALESCE(b.role, '') NOT IN ('trash', 'junk')"
        ),
        "t DESC",
        &[],
    )
}

// ---------------------------------------------------------------------------- snooze

/// Snoozes messages until `until` (Unix seconds); returns the accounts whose queue changed.
pub fn snooze(store: &Store, ids: &[i64], until: i64) -> Result<Vec<i64>> {
    if until <= now() {
        return Err(Error::Invalid(tr!("Der Zeitpunkt liegt in der Vergangenheit.", "That time has passed.").into()));
    }
    let list = rows(store, ids)?;
    for r in &list {
        let key = key_of(r.message_id.as_deref(), r.id);
        let mb = store.mailbox(r.mailbox_id)?;
        // Snoozed again: the new time replaces the old one.
        let earlier: Option<(i64, Option<String>)> = store
            .conn()
            .query_row(
                "SELECT id, folder FROM snoozes WHERE account_id = ?1 AND message_key = ?2 AND state = 'waiting'",
                params![r.account_id, key],
                |x| Ok((x.get(0)?, x.get(1)?)),
            )
            .optional()?;
        if let Some((id, _)) = earlier {
            store.conn().execute("UPDATE snoozes SET until = ?2 WHERE id = ?1", params![id, until])?;
            continue;
        }
        if r.message_id.is_some() && mb.role == Some(Role::Inbox) {
            let undo = actions::move_to_named(store, &[r.id], SNOOZE_FOLDER)?;
            store.conn().execute(
                "INSERT INTO snoozes (account_id, message_key, until, mailbox, folder, outbox_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![r.account_id, key, until, mb.name, SNOOZE_FOLDER, undo.outbox.last().copied(), now()],
            )?;
        } else {
            store.conn().execute("UPDATE messages SET snoozed_until = ?2 WHERE id = ?1", params![r.id, until])?;
            store.conn().execute(
                "INSERT INTO snoozes (account_id, message_key, until, mailbox, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![r.account_id, key, until, mb.name, now()],
            )?;
        }
    }
    Ok(accounts_of(&list))
}

/// Brings snoozed messages back now (the user took the snooze back); returns the accounts
/// whose queue changed.
pub fn unsnooze(store: &Store, ids: &[i64]) -> Result<Vec<i64>> {
    let list = rows(store, ids)?;
    let mut accounts = vec![];
    for r in &list {
        let key = key_of(r.message_id.as_deref(), r.id);
        let waiting: Vec<Snooze> = snoozes_where(
            store,
            "account_id = ?1 AND message_key = ?2 AND state = 'waiting'",
            params![r.account_id, key],
        )?;
        for s in waiting {
            if wake(store, &s, false)? {
                accounts.push(s.account_id);
            }
        }
    }
    accounts.sort_unstable();
    accounts.dedup();
    Ok(accounts)
}

#[derive(Debug, Clone)]
struct Snooze {
    id: i64,
    account_id: i64,
    key: String,
    mailbox: String,
    folder: Option<String>,
    outbox_id: Option<i64>,
}

fn snoozes_where(store: &Store, cond: &str, args: impl rusqlite::Params) -> Result<Vec<Snooze>> {
    let mut st = store.conn().prepare(&format!(
        "SELECT id, account_id, message_key, mailbox, folder, outbox_id FROM snoozes WHERE {cond} ORDER BY id"
    ))?;
    let rows = st.query_map(args, |r| {
        Ok(Snooze {
            id: r.get(0)?,
            account_id: r.get(1)?,
            key: r.get(2)?,
            mailbox: r.get(3)?,
            folder: r.get(4)?,
            outbox_id: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn bump(store: &Store, ids: &[i64], at: i64) -> Result<()> {
    for id in ids {
        store.conn().execute("UPDATE messages SET bumped_at = ?2 WHERE id = ?1", params![id, at])?;
    }
    Ok(())
}

/// Brings one snoozed message back: unread, at the top of the inbox (`unread`: false when the
/// user took the snooze back, the message stays as it was). `false` when it cannot be found yet
/// (its move to the server has not been synced); the next tick tries again.
fn wake(store: &Store, s: &Snooze, unread: bool) -> Result<bool> {
    let at = now();
    let Some(folder) = &s.folder else {
        let ids: Vec<i64> = {
            let mut st = store.conn().prepare(&format!(
                "SELECT m.id FROM messages m WHERE m.account_id = ?1 AND {KEY_SQL} = ?2 AND m.snoozed_until IS NOT NULL"
            ))?;
            st.query_map(params![s.account_id, s.key], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        for id in &ids {
            store.conn().execute("UPDATE messages SET snoozed_until = NULL WHERE id = ?1", [id])?;
        }
        bump(store, &ids, at)?;
        if unread && !ids.is_empty() {
            actions::set_flags(store, &ids, Some(false), None)?;
        }
        store.conn().execute("DELETE FROM snoozes WHERE id = ?1", [s.id])?;
        return Ok(true);
    };
    // Still waiting for the server: the move is taken back.
    if let Some(ob) = s.outbox_id
        && let Some(Op::Move { ids, .. }) = outbox::cancel(store, ob)?
    {
        store.set_hidden(&ids, false)?;
        bump(store, &ids, at)?;
        if unread {
            actions::set_flags(store, &ids, Some(false), None)?;
        }
        store.conn().execute("DELETE FROM snoozes WHERE id = ?1", [s.id])?;
        return Ok(true);
    }
    let ids: Vec<i64> = {
        let mut st = store.conn().prepare(
            "SELECT m.id FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id
             WHERE m.account_id = ?1 AND m.message_id = ?2 AND b.name = ?3 AND m.hidden = 0",
        )?;
        st.query_map(params![s.account_id, s.key, folder], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let target = match store.mailbox_by_name(s.account_id, &s.mailbox)? {
        Some(m) => Some(m),
        None => store.mailbox_by_role(s.account_id, Role::Inbox)?,
    };
    let Some(target) = target else { return Ok(false) };
    if ids.is_empty() {
        return Ok(false);
    }
    if unread {
        actions::set_flags(store, &ids, Some(false), None)?;
    }
    actions::move_now(store, &ids, target.id)?;
    store.conn().execute("UPDATE snoozes SET state = 'returned', returned_at = ?2 WHERE id = ?1", params![s.id, at])?;
    Ok(true)
}

/// The snoozed messages, the next to come back first.
pub fn snoozed(store: &Store) -> Result<Vec<MessageSummary>> {
    keyed_list(
        store,
        &format!(
            "FROM (SELECT account_id, message_key, until AS t, id AS g FROM snoozes WHERE state = 'waiting') x
             JOIN messages m ON m.account_id = x.account_id AND {KEY_SQL} = x.message_key"
        ),
        "t, g",
        &[],
    )
}

/// A message stored in an inbox: back from snoozing it sorts as if it arrived then. Called by
/// the store for every new row of an inbox.
pub(crate) fn arrived(store: &Store, account_id: i64, id: i64, message_id: Option<&str>) -> Result<()> {
    let Some(mid) = message_id else { return Ok(()) };
    let back: Option<(i64, Option<i64>)> = store
        .conn()
        .query_row(
            "SELECT id, returned_at FROM snoozes WHERE account_id = ?1 AND message_key = ?2 AND state = 'returned'
             ORDER BY id DESC LIMIT 1",
            params![account_id, mid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((sid, at)) = back {
        bump(store, &[id], at.unwrap_or_else(now))?;
        store.conn().execute("DELETE FROM snoozes WHERE id = ?1", [sid])?;
        return Ok(());
    }
    // Moved back into the inbox by another client while it waited: the snooze is over.
    store.conn().execute(
        "DELETE FROM snoozes WHERE account_id = ?1 AND message_key = ?2 AND state = 'waiting' AND folder IS NOT NULL
           AND (outbox_id IS NULL OR outbox_id NOT IN (SELECT id FROM outbox))",
        params![account_id, mid],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------- send later

/// A message waiting to be sent later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scheduled {
    pub id: i64,
    pub account_id: i64,
    pub due_at: i64,
    pub subject: String,
    pub recipients: String,
    pub followup_secs: Option<i64>,
}

/// Keeps a message to send at `at`; with `followup_secs` a reminder follows that long after it
/// went out unless an answer came. Returns the entry and the message's Message-ID.
pub fn schedule_send(
    store: &Store,
    d: &Draft,
    files: &[FileAttachment],
    at: i64,
    followup_secs: Option<i64>,
) -> Result<(i64, String)> {
    if at <= now() {
        return Err(Error::Invalid(tr!("Der Zeitpunkt liegt in der Vergangenheit.", "That time has passed.").into()));
    }
    let (op, mid) = actions::prepare_send(store, d, files, at)?;
    let recipients =
        d.to.iter()
            .chain(&d.cc)
            .chain(&d.bcc)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
    let draft = Draft { message_id: Some(mid.clone()), attachments: vec![], ..d.clone() };
    store.conn().execute(
        "INSERT INTO scheduled (account_id, due_at, op, draft, subject, recipients, message_id, followup_secs, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            d.account_id,
            at,
            serde_json::to_string(&op)?,
            serde_json::to_string(&draft)?,
            d.subject.trim(),
            recipients,
            mid,
            followup_secs,
            now()
        ],
    )?;
    Ok((store.conn().last_insert_rowid(), mid))
}

pub fn scheduled(store: &Store) -> Result<Vec<Scheduled>> {
    let mut st = store.conn().prepare(
        "SELECT id, account_id, due_at, subject, recipients, followup_secs FROM scheduled ORDER BY due_at, id",
    )?;
    let rows = st.query_map([], |r| {
        Ok(Scheduled {
            id: r.get(0)?,
            account_id: r.get(1)?,
            due_at: r.get(2)?,
            subject: r.get(3)?,
            recipients: r.get(4)?,
            followup_secs: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Takes a scheduled message back; returns its draft to open it again (without the files it
/// had: they were in the finished message).
pub fn unschedule(store: &Store, id: i64) -> Result<Draft> {
    let draft: Option<String> =
        store.conn().query_row("SELECT draft FROM scheduled WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    let Some(draft) = draft else {
        return Err(Error::NotFound(
            tr!("Die Nachricht ist schon unterwegs.", "The message is on its way already.").into(),
        ));
    };
    store.conn().execute("DELETE FROM scheduled WHERE id = ?1", [id])?;
    Ok(serde_json::from_str(&draft)?)
}

/// Hands one scheduled message to the queue; returns its account.
fn release(store: &Store, id: i64) -> Result<Option<i64>> {
    let row: Option<(i64, String, String, String, Option<i64>)> = store
        .conn()
        .query_row(
            "SELECT account_id, op, message_id, subject, followup_secs FROM scheduled WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((account, op, mid, subject, followup)) = row else { return Ok(None) };
    let op: Op = serde_json::from_str(&op)?;
    outbox::enqueue(store, account, &op)?;
    store.conn().execute("DELETE FROM scheduled WHERE id = ?1", [id])?;
    if let Some(secs) = followup {
        add_followup(store, account, &mid, now() + secs, &subject)?;
    }
    Ok(Some(account))
}

/// „Jetzt senden“ for a scheduled message; returns its account.
pub fn send_now(store: &Store, id: i64) -> Result<i64> {
    store.batch(|| release(store, id))?.ok_or_else(|| {
        Error::NotFound(tr!("Die Nachricht ist schon unterwegs.", "The message is on its way already.").into())
    })
}

// ---------------------------------------------------------------------------- follow-ups

/// A reminder for the message with this key, due at `due` unless it was answered.
pub fn add_followup(store: &Store, account_id: i64, key: &str, due: i64, subject: &str) -> Result<i64> {
    store.conn().execute(
        "UPDATE followups SET state = 'done' WHERE account_id = ?1 AND message_key = ?2 AND state <> 'done'",
        params![account_id, key],
    )?;
    store.conn().execute(
        "INSERT INTO followups (account_id, message_key, due_at, subject, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![account_id, key, due, subject, now()],
    )?;
    Ok(store.conn().last_insert_rowid())
}

/// „Erinnern, wenn keine Antwort“ on stored messages.
pub fn followup_messages(store: &Store, ids: &[i64], due: i64) -> Result<()> {
    if due <= now() {
        return Err(Error::Invalid(tr!("Der Zeitpunkt liegt in der Vergangenheit.", "That time has passed.").into()));
    }
    for r in rows(store, ids)? {
        add_followup(store, r.account_id, &key_of(r.message_id.as_deref(), r.id), due, &r.subject)?;
    }
    Ok(())
}

/// Takes reminders off messages (done, or not wanted any more).
pub fn dismiss_followups(store: &Store, ids: &[i64]) -> Result<()> {
    for r in rows(store, ids)? {
        store.conn().execute(
            "UPDATE followups SET state = 'done' WHERE account_id = ?1 AND message_key = ?2 AND state <> 'done'",
            params![r.account_id, key_of(r.message_id.as_deref(), r.id)],
        )?;
    }
    Ok(())
}

/// Drops the reminder of a message that was not sent after all (undo send).
pub fn forget_followup(store: &Store, message_id: &str) -> Result<()> {
    store.conn().execute("DELETE FROM followups WHERE message_key = ?1 AND state = 'waiting'", [message_id])?;
    Ok(())
}

/// Whether someone other than the user answered the message with this key.
fn answered(store: &Store, account_id: i64, key: &str) -> Result<bool> {
    Ok(store
        .conn()
        .query_row(
            "SELECT 1 FROM messages r JOIN accounts a ON a.id = r.account_id
             WHERE r.account_id = ?1 AND (r.in_reply_to = ?2 OR r.refs LIKE ?3)
               AND lower(COALESCE(r.from_email, '')) <> lower(a.email) LIMIT 1",
            params![account_id, key, format!("%\"{key}\"%")],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The reminders that came due without an answer, the latest first.
pub fn followups_due(store: &Store) -> Result<Vec<MessageSummary>> {
    keyed_list(
        store,
        &format!(
            "FROM (SELECT account_id, message_key, due_at AS t, id AS g FROM followups WHERE state = 'due') x
             JOIN messages m ON m.account_id = x.account_id AND {KEY_SQL} = x.message_key WHERE m.hidden = 0"
        ),
        "t DESC, g",
        &[],
    )
}

// ---------------------------------------------------------------------------- the clock

/// A reminder that came due, for a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub account_id: i64,
    pub subject: String,
}

/// What [`tick`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Tick {
    /// Accounts whose queue or lists changed (their workers are kicked, the UI reloads).
    pub accounts: Vec<i64>,
    /// Scheduled messages handed to the queue.
    pub sent: usize,
    /// Snoozed messages that came back.
    pub returned: usize,
    pub reminders: Vec<Reminder>,
}

impl Tick {
    pub fn changed(&self) -> bool {
        !self.accounts.is_empty() || !self.reminders.is_empty()
    }
}

/// Does what is due at `at`: scheduled messages go to the queue, snoozed ones come back,
/// reminders without an answer come due (and the ones answered meanwhile go). One transaction:
/// the app's clock and a phone's background check never hand the same message to the queue
/// twice.
pub fn tick(store: &Store, at: i64) -> Result<Tick> {
    store.batch(|| tick_in(store, at))
}

fn tick_in(store: &Store, at: i64) -> Result<Tick> {
    let mut t = Tick::default();
    let due: Vec<i64> = {
        let mut st = store.conn().prepare("SELECT id FROM scheduled WHERE due_at <= ?1 ORDER BY due_at, id")?;
        st.query_map([at], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    for id in due {
        if let Some(account) = release(store, id)? {
            t.sent += 1;
            t.accounts.push(account);
        }
    }
    for s in snoozes_where(store, "state = 'waiting' AND until <= ?1", [at])? {
        if wake(store, &s, true)? {
            t.returned += 1;
            t.accounts.push(s.account_id);
        }
    }
    let followups: Vec<(i64, i64, String, String, String)> = {
        let mut st = store.conn().prepare(
            "SELECT id, account_id, message_key, subject, state FROM followups
             WHERE state = 'due' OR (state = 'waiting' AND due_at <= ?1)",
        )?;
        st.query_map([at], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, account, key, subject, state) in followups {
        if answered(store, account, &key)? {
            store.conn().execute("UPDATE followups SET state = 'done' WHERE id = ?1", [id])?;
            if state == "due" {
                t.accounts.push(account);
            }
            continue;
        }
        if state == "due" {
            continue;
        }
        store.conn().execute("UPDATE followups SET state = 'due' WHERE id = ?1", [id])?;
        // A message in the inbox comes back to the top, unread.
        let inbox: Vec<i64> = {
            let mut st = store.conn().prepare(&format!(
                "SELECT m.id FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id
                 WHERE m.account_id = ?1 AND {KEY_SQL} = ?2 AND b.role = 'inbox' AND m.hidden = 0"
            ))?;
            st.query_map(params![account, key], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        if !inbox.is_empty() {
            bump(store, &inbox, at)?;
            actions::set_flags(store, &inbox, Some(false), None)?;
        }
        t.reminders.push(Reminder { account_id: account, subject });
        t.accounts.push(account);
    }
    store.conn().execute("DELETE FROM snoozes WHERE state = 'returned' AND returned_at < ?1", [at - 7 * 86400])?;
    store.conn().execute("DELETE FROM followups WHERE state = 'done' AND due_at < ?1", [at - 30 * 86400])?;
    t.accounts.sort_unstable();
    t.accounts.dedup();
    Ok(t)
}

/// How many messages each time tool holds, for the sidebar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub snoozed: u32,
    pub later: u32,
    pub followups: u32,
    pub scheduled: u32,
}

pub fn counts(store: &Store) -> Result<Counts> {
    Ok(Counts {
        snoozed: snoozed(store)?.len() as u32,
        later: later_list(store)?.len() as u32,
        followups: followups_due(store)?.len() as u32,
        scheduled: store.conn().query_row("SELECT COUNT(*) FROM scheduled", [], |r| r.get(0))?,
    })
}

/// When [`tick`] has something to do next (to wake up in time), if anything waits.
pub fn next_due(store: &Store) -> Result<Option<i64>> {
    Ok(store.conn().query_row(
        "SELECT MIN(t) FROM (
           SELECT MIN(due_at) AS t FROM scheduled
           UNION ALL SELECT MIN(until) FROM snoozes WHERE state = 'waiting'
           UNION ALL SELECT MIN(due_at) FROM followups WHERE state = 'waiting')",
        [],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::MailboxEntry;
    use crate::parse::parse;
    use crate::store::tests::{header, store_with_inbox};

    fn add(s: &Store, mailbox: i64, uid: u32, from: &str, subject: &str, extra: &str) -> i64 {
        let raw = format!(
            "From: {from}\r\nTo: maria@example.org\r\nSubject: {subject}\r\nMessage-ID: <{}@x>\r\n{extra}\r\nText\r\n",
            subject.replace(' ', ".")
        );
        s.insert_message(mailbox, &header(uid, raw.as_bytes()), &parse(raw.as_bytes()).unwrap()).unwrap()
    }

    fn inbox_subjects(s: &Store) -> Vec<String> {
        let q = crate::triage::InboxQuery {
            account: None,
            tab: crate::triage::Category::People,
            focus: false,
            threads: false,
            bundles: false,
        };
        crate::triage::inbox(s, &q, 50, 0).unwrap().1.into_iter().map(|m| m.subject).collect()
    }

    fn kinds(s: &Store) -> Vec<String> {
        outbox::entries(s).unwrap().into_iter().map(|e| e.kind).collect()
    }

    #[test]
    fn pins_and_read_later_survive_a_move() {
        let (s, account, inbox) = store_with_inbox();
        let a = add(&s, inbox, 1, "jonas@example.org", "Eins", "");
        let b = add(&s, inbox, 2, "jonas@example.org", "Zwei", "");
        assert_eq!(inbox_subjects(&s), ["Zwei", "Eins"]);
        set_pinned(&s, &[a], true).unwrap();
        assert_eq!(inbox_subjects(&s), ["Eins", "Zwei"], "pinned first");
        set_later(&s, &[b], true).unwrap();
        assert!(marks_of(&s, a).unwrap().pinned && marks_of(&s, b).unwrap().later);
        // The copy in the archive (same Message-ID) keeps the mark.
        let archive = s
            .upsert_mailbox(account, &MailboxEntry { name: "Archive".into(), delimiter: None, attributes: vec![] })
            .unwrap();
        let copy = add(&s, archive, 7, "jonas@example.org", "Zwei", "");
        s.remove_messages(&[b]).unwrap();
        assert_eq!(later_list(&s).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [copy]);
        set_later(&s, &[copy], false).unwrap();
        assert!(later_list(&s).unwrap().is_empty());
        set_pinned(&s, &[a], false).unwrap();
        assert_eq!(inbox_subjects(&s), ["Eins"]);
    }

    #[test]
    fn snoozed_messages_wait_in_a_folder_and_come_back_on_top() {
        let (s, account, inbox) = store_with_inbox();
        let old = add(&s, inbox, 1, "jonas@example.org", "Alt", "");
        add(&s, inbox, 2, "jonas@example.org", "Neu", "");
        s.mark_seen(old, true).unwrap();
        let t = now();
        assert!(snooze(&s, &[old], t - 1).is_err(), "not in the past");
        assert_eq!(snooze(&s, &[old], t + 3600).unwrap(), [account]);
        assert_eq!(inbox_subjects(&s), ["Neu"]);
        assert_eq!(kinds(&s), ["create_folder", "move"]);
        assert_eq!(snoozed(&s).unwrap()[0].due, Some(t + 3600));
        assert_eq!(marks_of(&s, old).unwrap().snoozed_until, Some(t + 3600));
        assert_eq!(next_due(&s).unwrap(), Some(t + 3600));
        // Due before the move ran: the move is taken back, the message is unread on top.
        let tick1 = tick(&s, t + 3600).unwrap();
        assert_eq!((tick1.returned, tick1.accounts.as_slice()), (1, &[account][..]));
        assert_eq!(inbox_subjects(&s), ["Alt", "Neu"]);
        assert!(!s.summaries(&[old]).unwrap()[0].seen);
        assert_eq!(kinds(&s), ["create_folder", "flags"]);
        assert!(snoozed(&s).unwrap().is_empty());

        // Snoozed again; this time the server has it in Snoozed.
        snooze(&s, &[old], t + 7200).unwrap();
        let mv = outbox::entries(&s).unwrap().into_iter().find(|e| e.kind == "move").unwrap();
        outbox::done(&s, mv.id).unwrap();
        s.remove_messages(&[old]).unwrap();
        let folder = s.mailbox_by_name(account, SNOOZE_FOLDER).unwrap().unwrap();
        let waiting = add(&s, folder.id, 1, "jonas@example.org", "Alt", "");
        assert_eq!(snoozed(&s).unwrap()[0].id, waiting);
        assert_eq!(tick(&s, t + 3600).unwrap().returned, 0, "not yet");
        assert_eq!(tick(&s, t + 7200).unwrap().returned, 1);
        let e = outbox::entries(&s).unwrap();
        assert_eq!(e.iter().rev().take(2).map(|e| e.kind.as_str()).collect::<Vec<_>>(), ["move", "flags"]);
        assert!(e.last().unwrap().next_try <= now(), "no undo window for the way back");
        // The server's copy arrives in the inbox: on top.
        add(&s, inbox, 3, "jonas@example.org", "Alt", "");
        assert_eq!(inbox_subjects(&s)[0], "Alt");
    }

    #[test]
    fn snoozing_without_a_message_id_stays_here() {
        let (s, _, inbox) = store_with_inbox();
        let raw = b"From: jonas@example.org\r\nSubject: Ohne\r\n\r\nx";
        let id = s.insert_message(inbox, &header(1, raw), &parse(raw).unwrap()).unwrap();
        let t = now();
        snooze(&s, &[id], t + 60).unwrap();
        assert!(inbox_subjects(&s).is_empty());
        assert!(kinds(&s).is_empty(), "nothing for the server");
        assert_eq!(snoozed(&s).unwrap()[0].id, id);
        unsnooze(&s, &[id]).unwrap();
        assert_eq!(inbox_subjects(&s), ["Ohne"]);
        assert!(snoozed(&s).unwrap().is_empty());
    }

    #[test]
    fn scheduled_messages_go_to_the_queue_when_due() {
        let (s, account, _) = store_with_inbox();
        let d = Draft {
            account_id: account,
            to: vec!["lea@example.org".into()],
            subject: "Später".into(),
            text: "x".into(),
            ..Draft::default()
        };
        let t = now();
        assert!(schedule_send(&s, &d, &[], t - 5, None).is_err());
        let (id, mid) = schedule_send(&s, &d, &[], t + 600, Some(86400)).unwrap();
        assert!(kinds(&s).is_empty(), "not queued yet");
        let list = scheduled(&s).unwrap();
        assert_eq!((list[0].id, list[0].recipients.as_str(), list[0].due_at), (id, "lea@example.org", t + 600));
        assert_eq!(tick(&s, t + 599).unwrap().sent, 0);
        let done = tick(&s, t + 600).unwrap();
        assert_eq!((done.sent, done.accounts.as_slice()), (1, &[account][..]));
        assert_eq!(kinds(&s), ["send"]);
        assert!(scheduled(&s).unwrap().is_empty());
        // The reminder it asked for.
        let f: (String, i64) = s
            .conn()
            .query_row("SELECT message_key, due_at FROM followups", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(f.0, mid);
        assert!(f.1 >= t + 86400);
        // Taken back: the draft returns.
        let (id2, _) = schedule_send(&s, &d, &[], t + 600, None).unwrap();
        assert_eq!(unschedule(&s, id2).unwrap().subject, "Später");
        assert!(unschedule(&s, id2).is_err());
        let (id3, _) = schedule_send(&s, &d, &[], t + 600, None).unwrap();
        assert_eq!(send_now(&s, id3).unwrap(), account);
    }

    #[test]
    fn reminders_come_due_without_an_answer() {
        let (s, account, inbox) = store_with_inbox();
        let sent = s
            .upsert_mailbox(
                account,
                &MailboxEntry { name: "Sent".into(), delimiter: None, attributes: vec!["\\Sent".into()] },
            )
            .unwrap();
        let mine = add(&s, sent, 1, "maria@example.org", "Angebot", "");
        let theirs = add(&s, inbox, 1, "lea@example.org", "Frage", "");
        let t = now();
        followup_messages(&s, &[mine, theirs], t + 100).unwrap();
        assert_eq!(marks_of(&s, mine).unwrap().followup_due, Some(t + 100));
        // The user's own answer does not count; Jonas answers the offer.
        add(&s, inbox, 2, "maria@example.org", "Re Frage", "In-Reply-To: <Frage@x>\r\n");
        add(&s, inbox, 3, "jonas@example.org", "Re Angebot", "In-Reply-To: <x@y>\r\nReferences: <x@y> <Angebot@x>\r\n");
        let r = tick(&s, t + 100).unwrap();
        assert_eq!(r.reminders, [Reminder { account_id: account, subject: "Frage".into() }]);
        assert_eq!(followups_due(&s).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [theirs]);
        assert_eq!(inbox_subjects(&s)[0], "Frage", "back on top");
        assert!(tick(&s, t + 200).unwrap().reminders.is_empty(), "told once");
        dismiss_followups(&s, &[theirs]).unwrap();
        assert!(followups_due(&s).unwrap().is_empty());
    }
}
