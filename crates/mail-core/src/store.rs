//! The local store: one SQLite database per profile (WAL, foreign keys, FTS5).
//!
//! Migrations are numbered files in `migrations/`, applied in order and tracked through
//! `PRAGMA user_version`; a database newer than the binary is refused rather than modified.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::account::{Account, AccountDraft, Provider, validate};
use crate::error::{Error, Result};
use crate::imap::{FetchedHeader, Flags, MailboxEntry, Role};
use crate::parse::{Address, ParsedMessage};
use crate::search::fold;
use crate::trf;

/// The migrations in order; `MIGRATIONS[i]` brings the schema from version `i` to `i + 1`.
pub const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

pub struct Store {
    conn: Connection,
}

/// A folder with what the sync engine needs to know about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mailbox {
    pub id: i64,
    pub account_id: i64,
    pub name: String,
    pub delimiter: Option<String>,
    pub role: Option<Role>,
    pub selectable: bool,
    pub sync: SyncState,
    pub unread: u32,
    pub total: u32,
}

/// The last known server state of a folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub highest_modseq: Option<u64>,
}

/// A row of the message list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSummary {
    pub id: i64,
    pub mailbox_id: i64,
    pub uid: u32,
    pub thread_id: i64,
    pub subject: String,
    pub from: Option<Address>,
    pub date: i64,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub has_attachments: bool,
}

/// A message as the reading pane shows it (the HTML not yet sanitized).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDetail {
    pub summary: MessageSummary,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub message_id: Option<String>,
    pub has_body: bool,
    pub text: Option<String>,
    pub html: Option<String>,
    pub remote_allowed: bool,
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn provider_str(p: Provider) -> &'static str {
    match p {
        Provider::Imap => "imap",
        Provider::Microsoft => "microsoft",
        Provider::Google => "google",
        Provider::Jmap => "jmap",
    }
}

fn provider_of(s: &str) -> Provider {
    match s {
        "microsoft" => Provider::Microsoft,
        "google" => Provider::Google,
        "jmap" => Provider::Jmap,
        _ => Provider::Imap,
    }
}

const SUMMARY_COLS: &str = "id, mailbox_id, uid, thread_id, subject, from_name, from_email, COALESCE(date, internal_date), \
     snippet, seen, flagged, answered, has_attachments";

fn summary(r: &Row<'_>) -> rusqlite::Result<MessageSummary> {
    let email: Option<String> = r.get(6)?;
    Ok(MessageSummary {
        id: r.get(0)?,
        mailbox_id: r.get(1)?,
        uid: r.get(2)?,
        thread_id: r.get(3)?,
        subject: r.get(4)?,
        from: email.map(|email| Address { name: r.get(5).ok().flatten(), email }),
        date: r.get(7)?,
        snippet: r.get(8)?,
        seen: r.get(9)?,
        flagged: r.get(10)?,
        answered: r.get(11)?,
        has_attachments: r.get(12)?,
    })
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn migrate(&mut self) -> Result<()> {
        let have = self.schema_version()?;
        if have > SCHEMA_VERSION {
            return Err(Error::Newer(trf!(
                "Die Datenbank stammt von einer neueren Version von Arcalo Mail (Schema {have}, diese kennt {SCHEMA_VERSION}). Aktualisiere die App.",
                "The database comes from a newer version of Arcalo Mail (schema {have}, this one knows {SCHEMA_VERSION}). Update the app."
            )));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(have as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as u32 + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    // ------------------------------------------------------------------ accounts

    pub fn add_account(&self, draft: &AccountDraft) -> Result<i64> {
        validate(draft)?;
        self.conn.execute(
            "INSERT INTO accounts (provider, name, email, sender_name, transport, sort, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, (SELECT COALESCE(MAX(sort), 0) + 1 FROM accounts), ?6)",
            params![
                provider_str(draft.provider),
                draft.name.trim(),
                draft.email.trim(),
                draft.sender_name.trim(),
                serde_json::to_string(&draft.transport)?,
                now()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut st = self
            .conn
            .prepare("SELECT id, provider, name, email, sender_name, transport FROM accounts ORDER BY sort, id")?;
        let rows = st.query_map([], |r| {
            Ok((r.get(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, String>(5)?))
        })?;
        let mut out = vec![];
        for row in rows {
            let (id, provider, name, email, sender_name, transport) = row?;
            out.push(Account {
                id,
                provider: provider_of(&provider),
                name,
                email,
                sender_name,
                transport: serde_json::from_str(&transport)?,
            });
        }
        Ok(out)
    }

    /// Removes an account with its folders, messages and queue (the caller deletes its secrets).
    pub fn remove_account(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE account_id = ?1)", [id])?;
        self.conn.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        Ok(())
    }

    // ------------------------------------------------------------------ folders

    /// Adds a folder of a `LIST` answer or updates its attributes; returns its id.
    pub fn upsert_mailbox(&self, account_id: i64, e: &MailboxEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO mailboxes (account_id, name, delimiter, role, selectable) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (account_id, name) DO UPDATE SET delimiter = excluded.delimiter, role = excluded.role,
               selectable = excluded.selectable",
            params![account_id, e.name, e.delimiter.map(String::from), e.role().map(Role::as_str), e.selectable()],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM mailboxes WHERE account_id = ?1 AND name = ?2",
            params![account_id, e.name],
            |r| r.get(0),
        )?)
    }

    /// Removes folders that are no longer on the server.
    pub fn retain_mailboxes(&self, account_id: i64, names: &[String]) -> Result<()> {
        let existing = self.mailboxes(account_id)?;
        for m in existing.iter().filter(|m| !names.contains(&m.name)) {
            self.clear_mailbox(m.id)?;
            self.conn.execute("DELETE FROM mailboxes WHERE id = ?1", [m.id])?;
        }
        Ok(())
    }

    fn mailbox_row(r: &Row<'_>) -> rusqlite::Result<Mailbox> {
        let role: Option<String> = r.get(4)?;
        Ok(Mailbox {
            id: r.get(0)?,
            account_id: r.get(1)?,
            name: r.get(2)?,
            delimiter: r.get(3)?,
            role: role.as_deref().and_then(Role::parse),
            selectable: r.get(5)?,
            sync: SyncState {
                uid_validity: r.get(6)?,
                uid_next: r.get(7)?,
                highest_modseq: r.get::<_, Option<i64>>(8)?.map(|v| v as u64),
            },
            unread: r.get(9)?,
            total: r.get(10)?,
        })
    }

    const MAILBOX_COLS: &str = "b.id, b.account_id, b.name, b.delimiter, b.role, b.selectable, b.uid_validity, b.uid_next, \
         b.highest_modseq, (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id = b.id AND m.seen = 0), \
         (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id = b.id)";

    /// The folders of an account: the inbox and the other special folders first, then by name.
    pub fn mailboxes(&self, account_id: i64) -> Result<Vec<Mailbox>> {
        let sql = format!(
            "SELECT {} FROM mailboxes b WHERE b.account_id = ?1 ORDER BY
               CASE b.role WHEN 'inbox' THEN 0 WHEN 'drafts' THEN 1 WHEN 'sent' THEN 2 WHEN 'archive' THEN 3
                 WHEN 'junk' THEN 4 WHEN 'trash' THEN 5 ELSE 9 END, b.name COLLATE NOCASE",
            Self::MAILBOX_COLS
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([account_id], Self::mailbox_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn mailbox(&self, id: i64) -> Result<Mailbox> {
        let sql = format!("SELECT {} FROM mailboxes b WHERE b.id = ?1", Self::MAILBOX_COLS);
        self.conn
            .query_row(&sql, [id], Self::mailbox_row)
            .optional()?
            .ok_or_else(|| Error::NotFound(trf!("Ordner {id} fehlt.", "Folder {id} is missing.")))
    }

    pub fn set_sync_state(&self, mailbox_id: i64, s: SyncState) -> Result<()> {
        self.conn.execute(
            "UPDATE mailboxes SET uid_validity = ?2, uid_next = ?3, highest_modseq = ?4, last_sync = ?5 WHERE id = ?1",
            params![mailbox_id, s.uid_validity, s.uid_next, s.highest_modseq.map(|v| v as i64), now()],
        )?;
        Ok(())
    }

    /// Drops every message of a folder (its UIDVALIDITY changed: the old UIDs mean nothing).
    pub fn clear_mailbox(&self, mailbox_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE mailbox_id = ?1)",
            [mailbox_id],
        )?;
        self.conn.execute("DELETE FROM messages WHERE mailbox_id = ?1", [mailbox_id])?;
        Ok(())
    }

    // ------------------------------------------------------------------ messages

    /// Stores a message from its header (the body follows on demand); returns its id. A UID
    /// already stored keeps its row and gets the new flags.
    pub fn insert_message(&self, mailbox_id: i64, h: &FetchedHeader, p: &ParsedMessage) -> Result<i64> {
        let account_id: i64 =
            self.conn.query_row("SELECT account_id FROM mailboxes WHERE id = ?1", [mailbox_id], |r| r.get(0))?;
        if let Some(id) = self
            .conn
            .query_row("SELECT id FROM messages WHERE mailbox_id = ?1 AND uid = ?2", params![mailbox_id, h.uid], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        {
            self.set_flags(mailbox_id, h.uid, &h.flags, h.modseq)?;
            return Ok(id);
        }
        let has_attachments = p.attachments.iter().any(|a| !a.inline)
            || h.header.windows(15).any(|w| w.eq_ignore_ascii_case(b"multipart/mixed"));
        self.conn.execute(
            "INSERT INTO messages (account_id, mailbox_id, uid, modseq, message_id, in_reply_to, refs, subject,
               from_name, from_email, to_json, cc_json, date, internal_date, size, seen, answered, flagged, draft,
               keywords, has_attachments, snippet)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                account_id,
                mailbox_id,
                h.uid,
                h.modseq.map(|v| v as i64),
                p.message_id,
                p.in_reply_to,
                serde_json::to_string(&p.references)?,
                p.subject,
                p.from.as_ref().and_then(|f| f.name.clone()),
                p.from.as_ref().map(|f| f.email.clone()),
                serde_json::to_string(&p.to)?,
                serde_json::to_string(&p.cc)?,
                p.date,
                h.internal_date,
                h.size,
                h.flags.seen,
                h.flags.answered,
                h.flags.flagged,
                h.flags.draft,
                serde_json::to_string(&h.flags.keywords)?,
                has_attachments,
                p.snippet,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        crate::thread::assign(self, account_id, id, p)?;
        self.index_message(id, p)?;
        Ok(id)
    }

    /// Writes the search entry of a message (again, after its body arrived).
    pub(crate) fn index_message(&self, id: i64, p: &ParsedMessage) -> Result<()> {
        let people = p
            .from
            .iter()
            .chain(&p.to)
            .chain(&p.cc)
            .map(|a| format!("{} {}", a.name.as_deref().unwrap_or_default(), a.email))
            .collect::<Vec<_>>()
            .join(" ");
        let body = p.text.as_deref().unwrap_or(&p.snippet);
        self.conn.execute("DELETE FROM messages_fts WHERE rowid = ?1", [id])?;
        self.conn.execute(
            "INSERT INTO messages_fts (rowid, subject, people, body) VALUES (?1, ?2, ?3, ?4)",
            params![id, fold(&p.subject), fold(&people), fold(body)],
        )?;
        Ok(())
    }

    /// Stores the body of a message (parsed from the whole message) and its attachments.
    pub fn set_body(&self, id: i64, p: &ParsedMessage) -> Result<()> {
        self.conn.execute(
            "INSERT INTO message_bodies (message_id, text, html) VALUES (?1, ?2, ?3)
             ON CONFLICT (message_id) DO UPDATE SET text = excluded.text, html = excluded.html",
            params![id, p.text, p.html],
        )?;
        self.conn.execute("DELETE FROM attachments WHERE message_id = ?1", [id])?;
        for a in &p.attachments {
            self.conn.execute(
                "INSERT INTO attachments (message_id, part_index, filename, mime_type, size, content_id, inline)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, a.index, a.filename, a.mime_type, a.size as i64, a.content_id, a.inline],
            )?;
        }
        self.conn.execute(
            "UPDATE messages SET has_body = 1, snippet = ?2, has_attachments = ?3 WHERE id = ?1",
            params![id, p.snippet, p.attachments.iter().any(|a| !a.inline)],
        )?;
        self.index_message(id, p)
    }

    pub fn set_flags(&self, mailbox_id: i64, uid: u32, f: &Flags, modseq: Option<u64>) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET seen = ?3, answered = ?4, flagged = ?5, draft = ?6, keywords = ?7,
               modseq = COALESCE(?8, modseq) WHERE mailbox_id = ?1 AND uid = ?2",
            params![
                mailbox_id,
                uid,
                f.seen,
                f.answered,
                f.flagged,
                f.draft,
                serde_json::to_string(&f.keywords)?,
                modseq.map(|v| v as i64)
            ],
        )?;
        Ok(())
    }

    /// Removes messages that are gone from the server.
    pub fn remove_uids(&self, mailbox_id: i64, uids: &[u32]) -> Result<usize> {
        let mut n = 0;
        for uid in uids {
            self.conn.execute(
                "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE mailbox_id = ?1 AND uid = ?2)",
                params![mailbox_id, uid],
            )?;
            n += self
                .conn
                .execute("DELETE FROM messages WHERE mailbox_id = ?1 AND uid = ?2", params![mailbox_id, uid])?;
        }
        Ok(n)
    }

    /// The UIDs stored for a folder, ascending.
    pub fn uids(&self, mailbox_id: i64) -> Result<Vec<u32>> {
        let mut st = self.conn.prepare("SELECT uid FROM messages WHERE mailbox_id = ?1 ORDER BY uid")?;
        let rows = st.query_map([mailbox_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The message list of a folder, newest first.
    pub fn messages(&self, mailbox_id: i64, limit: u32, offset: u32) -> Result<Vec<MessageSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages WHERE mailbox_id = ?1
             ORDER BY COALESCE(date, internal_date) DESC, id DESC LIMIT ?2 OFFSET ?3"
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(params![mailbox_id, limit, offset], summary)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The messages of a thread across folders, oldest first.
    pub fn thread(&self, thread_id: i64) -> Result<Vec<MessageSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages WHERE thread_id = ?1 ORDER BY COALESCE(date, internal_date), id"
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([thread_id], summary)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn summaries(&self, ids: &[i64]) -> Result<Vec<MessageSummary>> {
        let sql = format!("SELECT {SUMMARY_COLS} FROM messages WHERE id = ?1");
        let mut st = self.conn.prepare(&sql)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(s) = st.query_row([id], summary).optional()? {
                out.push(s);
            }
        }
        Ok(out)
    }

    pub fn message(&self, id: i64) -> Result<MessageDetail> {
        let sql = format!(
            "SELECT {SUMMARY_COLS}, to_json, cc_json, message_id, has_body,
               (SELECT text FROM message_bodies WHERE message_id = messages.id),
               (SELECT html FROM message_bodies WHERE message_id = messages.id),
               COALESCE((SELECT remote_allowed FROM message_bodies WHERE message_id = messages.id), 0)
             FROM messages WHERE id = ?1"
        );
        let row = self
            .conn
            .query_row(&sql, [id], |r| {
                Ok((
                    summary(r)?,
                    r.get::<_, String>(13)?,
                    r.get::<_, String>(14)?,
                    r.get(15)?,
                    r.get(16)?,
                    r.get(17)?,
                    r.get(18)?,
                    r.get(19)?,
                ))
            })
            .optional()?
            .ok_or_else(|| Error::NotFound(trf!("Nachricht {id} fehlt.", "Message {id} is missing.")))?;
        let (summary, to, cc, message_id, has_body, text, html, remote_allowed) = row;
        Ok(MessageDetail {
            summary,
            to: serde_json::from_str(&to)?,
            cc: serde_json::from_str(&cc)?,
            message_id,
            has_body,
            text,
            html,
            remote_allowed,
        })
    }

    /// Marks a message read or unread locally (the change is queued for the server, `outbox`).
    pub fn mark_seen(&self, id: i64, seen: bool) -> Result<()> {
        self.conn.execute("UPDATE messages SET seen = ?2 WHERE id = ?1", params![id, seen])?;
        Ok(())
    }

    /// Allows remote content for one message.
    pub fn allow_remote(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE message_bodies SET remote_allowed = 1 WHERE message_id = ?1", [id])?;
        Ok(())
    }

    // ------------------------------------------------------------------ settings

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::account::preset;
    use crate::parse::{parse, tests::sample};

    pub fn store_with_inbox() -> (Store, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        let account = s
            .add_account(&AccountDraft {
                provider: Provider::Imap,
                name: "Arbeit".into(),
                email: "maria@example.org".into(),
                sender_name: "Maria Muster".into(),
                transport: preset(Provider::Imap, "maria@example.org").unwrap(),
            })
            .unwrap();
        let inbox = s
            .upsert_mailbox(account, &MailboxEntry { name: "INBOX".into(), delimiter: Some('/'), attributes: vec![] })
            .unwrap();
        (s, account, inbox)
    }

    pub fn header(uid: u32, raw: &[u8]) -> FetchedHeader {
        FetchedHeader {
            uid,
            flags: Flags::default(),
            modseq: Some(uid as u64),
            internal_date: 1_760_000_000 + uid as i64,
            size: raw.len() as u32,
            header: raw.to_vec(),
        }
    }

    #[test]
    fn migrates_and_refuses_newer() {
        let dir = std::env::temp_dir().join(format!("mail-core-test-{}-{}", std::process::id(), now()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mail.db");
        {
            let s = Store::open(&path).unwrap();
            assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
            s.conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();
        }
        assert!(matches!(Store::open(&path), Err(Error::Newer(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn accounts_round_trip() {
        let (s, id, _) = store_with_inbox();
        let a = s.accounts().unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].id, id);
        assert_eq!(a[0].name, "Arbeit");
        assert_eq!(a[0].transport, preset(Provider::Imap, "maria@example.org").unwrap());
        s.remove_account(id).unwrap();
        assert!(s.accounts().unwrap().is_empty());
        assert!(s.mailboxes(id).unwrap().is_empty(), "folders go with the account");
    }

    #[test]
    fn messages_flags_and_removal() {
        let (s, _, inbox) = store_with_inbox();
        for uid in 1..=3 {
            let raw = sample(&format!("m{uid}@x"), &format!("Nachricht {uid}"), None, "Text");
            s.insert_message(inbox, &header(uid, &raw), &parse(&raw).unwrap()).unwrap();
        }
        let list = s.messages(inbox, 50, 0).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(s.mailbox(inbox).unwrap().unread, 3);
        s.set_flags(inbox, 2, &Flags { seen: true, ..Flags::default() }, Some(9)).unwrap();
        assert_eq!(s.mailbox(inbox).unwrap().unread, 2);
        assert_eq!(s.remove_uids(inbox, &[1, 7]).unwrap(), 1);
        assert_eq!(s.uids(inbox).unwrap(), vec![2, 3]);
        let mb = s.mailboxes(1).unwrap();
        assert_eq!(mb[0].role, Some(Role::Inbox));
    }

    #[test]
    fn body_and_detail() {
        let (s, _, inbox) = store_with_inbox();
        let raw = sample("d@x", "Bericht", None, "Der ganze Text");
        let id = s.insert_message(inbox, &header(1, &raw), &parse(&raw).unwrap()).unwrap();
        assert!(!s.message(id).unwrap().has_body);
        s.set_body(id, &parse(&raw).unwrap()).unwrap();
        let d = s.message(id).unwrap();
        assert!(d.has_body);
        assert_eq!(d.text.as_deref().map(str::trim), Some("Der ganze Text"));
        assert_eq!(d.to.len(), 2);
        assert!(!d.remote_allowed);
        s.allow_remote(id).unwrap();
        assert!(s.message(id).unwrap().remote_allowed);
        assert!(matches!(s.message(999), Err(Error::NotFound(_))));
    }
}
