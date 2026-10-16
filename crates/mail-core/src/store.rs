//! The local store: one SQLite database per profile (WAL, foreign keys, FTS5).
//!
//! Migrations are numbered files in `migrations/`, applied in order and tracked through
//! `PRAGMA user_version`; a database newer than the binary is refused rather than modified.
//! Several connections may use one file (the app's and one per account worker); WAL and the
//! busy timeout keep them apart.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::account::{Account, AccountDraft, Provider, Transport, validate};
use crate::error::{Error, Result};
use crate::imap::{FetchedHeader, Flags, MailboxEntry, Role};
use crate::parse::{Address, AttachmentInfo, ParsedMessage};
use crate::search::fold;
use crate::trf;

/// The migrations in order; `MIGRATIONS[i]` brings the schema from version `i` to `i + 1`.
pub const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_mail_actions.sql"),
    include_str!("../migrations/0003_sync_sources.sql"),
    include_str!("../migrations/0004_inbox_tools.sql"),
];
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

pub struct Store {
    conn: Connection,
    /// Replies without reply headers join a conversation by their subject (the setting
    /// `subject_threads`; the account worker sets it before each round).
    subject_threads: std::cell::Cell<bool>,
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

/// A row of the message list. In the conversation view it stands for its thread in the folder:
/// the newest message, with the number of messages and whether any is unread.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Messages of the thread in this folder (1 outside the conversation view).
    pub thread_count: u32,
    #[serde(default)]
    pub account_id: i64,
    /// The inbox tab the message is in (`triage::Category`); set in the sorted inbox only.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    /// A bundle of the newsletters tab: how many messages of this sender it holds (0: a plain row).
    #[serde(default)]
    pub bundle: u32,
    /// In the lists of the time tools: when it comes back, is due, or was put there (`later`).
    #[serde(default)]
    pub due: Option<i64>,
}

/// A message as the reading pane shows it (the HTML not yet sanitized).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDetail {
    pub summary: MessageSummary,
    pub account_id: i64,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub size: u32,
    pub draft: bool,
    pub has_body: bool,
    pub text: Option<String>,
    pub html: Option<String>,
    pub remote_allowed: bool,
    pub attachments: Vec<Attachment>,
    pub list_id: Option<String>,
    /// `List-Unsubscribe` as sent; [`crate::unsubscribe`] reads it.
    pub unsubscribe: Option<String>,
    pub one_click: bool,
}

/// An attachment as stored: from the kept message (`part_index`) or, for a large message, its
/// IMAP part (`section`), fetched when opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: i64,
    pub message_id: i64,
    pub part_index: u32,
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: u64,
    pub content_id: Option<String>,
    pub inline: bool,
    pub section: Option<String>,
}

/// The body of a large message, fetched part by part.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartialBody {
    pub text: Option<String>,
    pub html: Option<String>,
    pub snippet: String,
    /// Attachments with their sections.
    pub attachments: Vec<(AttachmentInfo, String)>,
    /// `cid:` pictures: Content-ID, type, bytes.
    pub inline: Vec<(String, String, Vec<u8>)>,
}

/// An address the compose sheet suggests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub email: String,
    pub name: Option<String>,
}

pub fn now() -> i64 {
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

pub(crate) const SUMMARY_COLS: &str = "id, mailbox_id, uid, thread_id, subject, from_name, from_email, COALESCE(date, internal_date), \
     snippet, seen, flagged, answered, has_attachments, account_id";

pub(crate) fn summary(r: &Row<'_>) -> rusqlite::Result<MessageSummary> {
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
        thread_count: 1,
        account_id: r.get(13)?,
        ..MessageSummary::default()
    })
}

const ATTACHMENT_COLS: &str = "id, message_id, part_index, filename, mime_type, size, content_id, inline, section";

fn attachment(r: &Row<'_>) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: r.get(0)?,
        message_id: r.get(1)?,
        part_index: r.get(2)?,
        filename: r.get(3)?,
        mime_type: r.get(4)?,
        size: r.get::<_, i64>(5)? as u64,
        content_id: r.get(6)?,
        inline: r.get(7)?,
        section: r.get(8)?,
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
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        let mut store = Store { conn, subject_threads: std::cell::Cell::new(true) };
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

    pub fn set_subject_threads(&self, on: bool) {
        self.subject_threads.set(on);
    }

    pub fn subject_threads(&self) -> bool {
        self.subject_threads.get()
    }

    /// Runs `f` in one transaction (a header batch of a sync: one write instead of hundreds).
    pub fn batch<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match f() {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
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
        let mut st = self.conn.prepare(
            "SELECT id, provider, name, email, sender_name, transport, signature FROM accounts ORDER BY sort, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, String>(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = vec![];
        for row in rows {
            let (id, provider, name, email, sender_name, transport, signature) = row?;
            out.push(Account {
                id,
                provider: provider_of(&provider),
                name,
                email,
                sender_name,
                transport: serde_json::from_str(&transport)?,
                signature,
            });
        }
        Ok(out)
    }

    pub fn account(&self, id: i64) -> Result<Account> {
        self.accounts()?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::NotFound(trf!("Konto {id} fehlt.", "Account {id} is missing.")))
    }

    /// Changes what the user sees of an account (its name, the sender name, the signature).
    pub fn update_account(&self, id: i64, name: &str, sender_name: &str, signature: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET name = ?2, sender_name = ?3, signature = ?4 WHERE id = ?1",
            params![id, name.trim(), sender_name.trim(), signature.trim_end()],
        )?;
        Ok(())
    }

    /// New server settings for an account (checked as a new account would be).
    pub fn update_transport(&self, id: i64, transport: &Transport) -> Result<()> {
        let a = self.account(id)?;
        validate(&AccountDraft {
            provider: a.provider,
            name: a.name,
            email: a.email,
            sender_name: a.sender_name,
            transport: transport.clone(),
        })?;
        self.conn.execute(
            "UPDATE accounts SET transport = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(transport)?],
        )?;
        Ok(())
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
            self.remove_mailbox(m.id)?;
        }
        Ok(())
    }

    /// Removes a folder and its messages here (deleted on the server, or by the user).
    pub fn remove_mailbox(&self, id: i64) -> Result<()> {
        self.clear_mailbox(id)?;
        self.conn.execute("DELETE FROM mailboxes WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Renames a folder here (and its subfolders); its messages stay until the next sync of the
    /// new name.
    pub fn rename_mailbox(&self, account_id: i64, from: &str, to: &str) -> Result<()> {
        let boxes = self.mailboxes(account_id)?;
        for m in boxes {
            let delim = m.delimiter.clone().unwrap_or_else(|| "/".into());
            let new = if m.name == from {
                to.to_string()
            } else if let Some(rest) = m.name.strip_prefix(&format!("{from}{delim}")) {
                format!("{to}{delim}{rest}")
            } else {
                continue;
            };
            self.conn.execute("UPDATE mailboxes SET name = ?2 WHERE id = ?1", params![m.id, new])?;
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
         b.highest_modseq, (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id = b.id AND m.seen = 0 AND m.hidden = 0), \
         (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id = b.id AND m.hidden = 0)";

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

    pub fn mailbox_by_name(&self, account_id: i64, name: &str) -> Result<Option<Mailbox>> {
        Ok(self.mailboxes(account_id)?.into_iter().find(|m| m.name == name))
    }

    /// The folder of a role (the account's Sent, Trash, …), if the server has one.
    pub fn mailbox_by_role(&self, account_id: i64, role: Role) -> Result<Option<Mailbox>> {
        Ok(self.mailboxes(account_id)?.into_iter().find(|m| m.role == Some(role)))
    }

    /// Unread messages in the inboxes of all accounts (the badge of the app).
    pub fn unread_in_inboxes(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id
             WHERE b.role = 'inbox' AND m.seen = 0 AND m.hidden = 0",
            [],
            |r| r.get(0),
        )?)
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
    /// already stored keeps its row and gets the new flags. A copy of a message already here
    /// (moved to the archive, a reply in Sent) takes over its body.
    pub fn insert_message(&self, mailbox_id: i64, h: &FetchedHeader, p: &ParsedMessage) -> Result<i64> {
        let (account_id, role): (i64, Option<String>) =
            self.conn.query_row("SELECT account_id, role FROM mailboxes WHERE id = ?1", [mailbox_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
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
               keywords, has_attachments, snippet, reply_to_json, base_subject, category, list_id, unsubscribe, one_click)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
               ?25, ?26, ?27, ?28)",
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
                serde_json::to_string(&p.reply_to)?,
                crate::thread::base_subject(&p.subject).0,
                crate::triage::classify(p).as_str(),
                p.list.list_id,
                p.list.unsubscribe,
                p.list.one_click,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        crate::thread::assign(self, account_id, id, p)?;
        if role.as_deref() == Some("inbox") {
            crate::later::arrived(self, account_id, id, p.message_id.as_deref())?;
        }
        self.index_message(id, p)?;
        self.adopt_body(account_id, id, p.message_id.as_deref())?;
        // The address book: who wrote, and whom the user wrote to.
        let at = p.date.unwrap_or(h.internal_date);
        if role.as_deref() == Some("sent") {
            for a in p.to.iter().chain(&p.cc) {
                self.touch_contact(a, true, at)?;
            }
        } else if role.as_deref() != Some("junk")
            && let Some(f) = &p.from
        {
            self.touch_contact(f, false, at)?;
        }
        Ok(id)
    }

    /// Copies the stored body of another copy of the same message (same Message-ID).
    fn adopt_body(&self, account_id: i64, id: i64, message_id: Option<&str>) -> Result<()> {
        let Some(mid) = message_id else { return Ok(()) };
        let Some(source): Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM messages WHERE account_id = ?1 AND message_id = ?2 AND id <> ?3 AND has_body = 1 LIMIT 1",
                params![account_id, mid, id],
                |r| r.get(0),
            )
            .optional()?
        else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO message_bodies (message_id, text, html, remote_allowed, raw)
             SELECT ?2, text, html, remote_allowed, raw FROM message_bodies WHERE message_id = ?1",
            params![source, id],
        )?;
        self.conn.execute(
            "INSERT INTO attachments (message_id, part_index, filename, mime_type, size, content_id, inline, section, data)
             SELECT ?2, part_index, filename, mime_type, size, content_id, inline, section, data
             FROM attachments WHERE message_id = ?1",
            params![source, id],
        )?;
        self.conn.execute(
            "UPDATE messages SET has_body = 1, snippet = (SELECT snippet FROM messages WHERE id = ?1),
               has_attachments = (SELECT has_attachments FROM messages WHERE id = ?1) WHERE id = ?2",
            params![source, id],
        )?;
        Ok(())
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

    /// Stores the body of a message (parsed from the whole message) and its attachments; `raw`
    /// keeps the whole message for attachments offline.
    pub fn set_body(&self, id: i64, p: &ParsedMessage) -> Result<()> {
        self.set_body_raw(id, p, None)
    }

    pub fn set_body_raw(&self, id: i64, p: &ParsedMessage, raw: Option<&[u8]>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO message_bodies (message_id, text, html, raw) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (message_id) DO UPDATE SET text = excluded.text, html = excluded.html, raw = excluded.raw",
            params![id, p.text, p.html, raw],
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
            params![id, p.snippet, p.attachments.iter().any(|a| !a.inline || a.content_id.is_none())],
        )?;
        self.index_message(id, p)
    }

    /// Stores the text of a large message, its attachments by section and the pictures its
    /// HTML shows.
    pub fn set_body_parts(&self, id: i64, header: &ParsedMessage, b: &PartialBody) -> Result<()> {
        self.conn.execute(
            "INSERT INTO message_bodies (message_id, text, html, raw) VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT (message_id) DO UPDATE SET text = excluded.text, html = excluded.html, raw = NULL",
            params![id, b.text, b.html],
        )?;
        self.conn.execute("DELETE FROM attachments WHERE message_id = ?1", [id])?;
        for (a, section) in &b.attachments {
            self.conn.execute(
                "INSERT INTO attachments (message_id, part_index, filename, mime_type, size, content_id, inline, section)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id, a.index, a.filename, a.mime_type, a.size as i64, a.content_id, a.inline, section],
            )?;
        }
        for (i, (cid, mime, data)) in b.inline.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO attachments (message_id, part_index, mime_type, size, content_id, inline, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![id, 10_000 + i as i64, mime, data.len() as i64, cid, data],
            )?;
        }
        self.conn.execute(
            "UPDATE messages SET has_body = 1, snippet = ?2, has_attachments = ?3 WHERE id = ?1",
            params![id, b.snippet, !b.attachments.is_empty()],
        )?;
        let p = ParsedMessage { text: b.text.clone(), snippet: b.snippet.clone(), ..header.clone() };
        self.index_message(id, &p)
    }

    /// The whole message, when it was kept.
    pub fn raw_message(&self, id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT raw FROM message_bodies WHERE message_id = ?1", [id], |r| r.get(0))
            .optional()?
            .flatten())
    }

    pub fn attachments(&self, message_id: i64) -> Result<Vec<Attachment>> {
        let sql = format!("SELECT {ATTACHMENT_COLS} FROM attachments WHERE message_id = ?1 ORDER BY part_index");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([message_id], attachment)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn attachment(&self, id: i64) -> Result<Attachment> {
        let sql = format!("SELECT {ATTACHMENT_COLS} FROM attachments WHERE id = ?1");
        self.conn
            .query_row(&sql, [id], attachment)
            .optional()?
            .ok_or_else(|| Error::NotFound(trf!("Anhang {id} fehlt.", "Attachment {id} is missing.")))
    }

    /// The `cid:` pictures of a message: from the kept message, or fetched with a large one.
    pub fn inline_images(&self, message_id: i64) -> Result<Vec<(String, String, Vec<u8>)>> {
        if let Some(raw) = self.raw_message(message_id)? {
            return Ok(crate::parse::inline_images(&raw));
        }
        let mut st = self.conn.prepare(
            "SELECT content_id, mime_type, data FROM attachments WHERE message_id = ?1 AND data IS NOT NULL
               AND content_id IS NOT NULL",
        )?;
        let rows = st.query_map([message_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Notes where an attachment was saved.
    pub fn set_attachment_path(&self, id: i64, path: &str) -> Result<()> {
        self.conn.execute("UPDATE attachments SET local_path = ?2 WHERE id = ?1", params![id, path])?;
        Ok(())
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

    /// Removes messages by id (moved away on the server, deleted for good).
    pub fn remove_messages(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            self.conn.execute("DELETE FROM messages_fts WHERE rowid = ?1", [id])?;
            self.conn.execute("DELETE FROM messages WHERE id = ?1", [id])?;
        }
        Ok(())
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
            "SELECT {SUMMARY_COLS} FROM messages WHERE mailbox_id = ?1 AND hidden = 0
             ORDER BY COALESCE(date, internal_date) DESC, id DESC LIMIT ?2 OFFSET ?3"
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(params![mailbox_id, limit, offset], summary)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The conversation view of a folder: one row per thread (its newest message), newest
    /// thread first; unread when any message of it is.
    pub fn threads(&self, mailbox_id: i64, limit: u32, offset: u32) -> Result<Vec<MessageSummary>> {
        let sql =
            "WITH ranked AS (
               SELECT *, ROW_NUMBER() OVER (PARTITION BY thread_id ORDER BY COALESCE(date, internal_date) DESC, id DESC) AS rn,
                 COUNT(*) OVER (PARTITION BY thread_id) AS n,
                 MIN(seen) OVER (PARTITION BY thread_id) AS all_seen,
                 MAX(flagged) OVER (PARTITION BY thread_id) AS any_flag,
                 MAX(has_attachments) OVER (PARTITION BY thread_id) AS any_att
               FROM messages WHERE mailbox_id = ?1 AND hidden = 0)
             SELECT id, mailbox_id, uid, thread_id, subject, from_name, from_email, COALESCE(date, internal_date),
               snippet, all_seen, any_flag, answered, any_att, account_id, n
             FROM ranked WHERE rn = 1 ORDER BY COALESCE(date, internal_date) DESC, id DESC LIMIT ?2 OFFSET ?3";
        let mut st = self.conn.prepare(sql)?;
        let rows = st.query_map(params![mailbox_id, limit, offset], |r| {
            let mut s = summary(r)?;
            s.thread_count = r.get(14)?;
            Ok(s)
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How many rows the list of a folder has (threads in the conversation view).
    pub fn list_len(&self, mailbox_id: i64, threads: bool) -> Result<u32> {
        let sql = if threads {
            "SELECT COUNT(DISTINCT thread_id) FROM messages WHERE mailbox_id = ?1 AND hidden = 0"
        } else {
            "SELECT COUNT(*) FROM messages WHERE mailbox_id = ?1 AND hidden = 0"
        };
        Ok(self.conn.query_row(sql, [mailbox_id], |r| r.get(0))?)
    }

    /// The messages of a thread across folders, oldest first (not the ones in the trash or spam,
    /// unless the thread lives there).
    pub fn thread(&self, thread_id: i64) -> Result<Vec<MessageSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages WHERE thread_id = ?1 AND hidden = 0
             ORDER BY COALESCE(date, internal_date), id"
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([thread_id], summary)?;
        let all: Vec<MessageSummary> = rows.collect::<rusqlite::Result<_>>()?;
        // One copy per Message-ID (the same message in Inbox and Archive): the first.
        let mut seen = std::collections::HashSet::new();
        let mut out = vec![];
        for m in all {
            let key: Option<String> =
                self.conn.query_row("SELECT message_id FROM messages WHERE id = ?1", [m.id], |r| r.get(0))?;
            if key.is_none_or(|k| seen.insert(k)) {
                out.push(m);
            }
        }
        Ok(out)
    }

    pub fn summaries(&self, ids: &[i64]) -> Result<Vec<MessageSummary>> {
        let sql = format!("SELECT {SUMMARY_COLS} FROM messages WHERE id = ?1 AND hidden = 0");
        let mut st = self.conn.prepare(&sql)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(s) = st.query_row([id], summary).optional()? {
                out.push(s);
            }
        }
        Ok(out)
    }

    /// The newest messages of a folder without a body, up to `max_size` bytes (prefetching).
    pub fn without_body(&self, mailbox_id: i64, limit: u32, max_size: u32) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare(
            "SELECT id FROM messages WHERE mailbox_id = ?1 AND has_body = 0 AND size <= ?2
             ORDER BY internal_date DESC, id DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![mailbox_id, max_size, limit], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Messages without a body received since `since` (Unix seconds), newest first.
    pub fn without_body_since(&self, mailbox_id: i64, since: i64, limit: u32, max_size: u32) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare(
            "SELECT id FROM messages WHERE mailbox_id = ?1 AND has_body = 0 AND size <= ?2 AND internal_date >= ?3
             ORDER BY internal_date DESC, id DESC LIMIT ?4",
        )?;
        let rows = st.query_map(params![mailbox_id, max_size, since, limit], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Ids of the stored messages of a folder with these UIDs.
    pub fn ids_for_uids(&self, mailbox_id: i64, uids: &[u32]) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare("SELECT id FROM messages WHERE mailbox_id = ?1 AND uid = ?2")?;
        let mut out = vec![];
        for uid in uids {
            if let Some(id) = st.query_row(params![mailbox_id, uid], |r| r.get(0)).optional()? {
                out.push(id);
            }
        }
        Ok(out)
    }

    pub fn message(&self, id: i64) -> Result<MessageDetail> {
        let sql = format!(
            "SELECT {SUMMARY_COLS}, to_json, cc_json, message_id, has_body,
               (SELECT text FROM message_bodies WHERE message_id = messages.id),
               (SELECT html FROM message_bodies WHERE message_id = messages.id),
               COALESCE((SELECT remote_allowed FROM message_bodies WHERE message_id = messages.id), 0),
               account_id, reply_to_json, in_reply_to, refs, size, draft, list_id, unsubscribe, one_click
             FROM messages WHERE id = ?1"
        );
        let row = self
            .conn
            .query_row(&sql, [id], |r| {
                Ok((
                    summary(r)?,
                    (r.get::<_, String>(14)?, r.get::<_, String>(15)?, r.get(16)?, r.get(17)?),
                    (r.get(18)?, r.get(19)?, r.get(20)?),
                    (r.get(21)?, r.get::<_, String>(22)?, r.get(23)?, r.get::<_, String>(24)?, r.get(25)?, r.get(26)?),
                    (r.get(27)?, r.get(28)?, r.get(29)?),
                ))
            })
            .optional()?
            .ok_or_else(|| Error::NotFound(trf!("Nachricht {id} fehlt.", "Message {id} is missing.")))?;
        let (summary, (to, cc, message_id, has_body), (text, html, remote_allowed), rest, list) = row;
        let (account_id, reply_to, in_reply_to, refs, size, draft) = rest;
        let (list_id, unsubscribe, one_click) = list;
        Ok(MessageDetail {
            summary,
            account_id,
            to: serde_json::from_str(&to)?,
            cc: serde_json::from_str(&cc)?,
            reply_to: serde_json::from_str(&reply_to).unwrap_or_default(),
            message_id,
            in_reply_to,
            references: serde_json::from_str(&refs).unwrap_or_default(),
            size,
            draft,
            has_body,
            text,
            html,
            remote_allowed,
            attachments: self.attachments(id)?,
            list_id,
            unsubscribe,
            one_click,
        })
    }

    /// Marks a message read or unread locally (the change is queued for the server, `outbox`).
    pub fn mark_seen(&self, id: i64, seen: bool) -> Result<()> {
        self.conn.execute("UPDATE messages SET seen = ?2 WHERE id = ?1", params![id, seen])?;
        Ok(())
    }

    pub fn mark_flagged(&self, id: i64, flagged: bool) -> Result<()> {
        self.conn.execute("UPDATE messages SET flagged = ?2 WHERE id = ?1", params![id, flagged])?;
        Ok(())
    }

    pub fn mark_answered(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE messages SET answered = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Hides messages while their move or deletion waits for the server (or shows them again
    /// on undo).
    pub fn set_hidden(&self, ids: &[i64], hidden: bool) -> Result<()> {
        for id in ids {
            self.conn.execute("UPDATE messages SET hidden = ?2 WHERE id = ?1", params![id, hidden])?;
        }
        Ok(())
    }

    /// Allows remote content for one message.
    pub fn allow_remote(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE message_bodies SET remote_allowed = 1 WHERE message_id = ?1", [id])?;
        Ok(())
    }

    // ------------------------------------------------------------------ senders and contacts

    /// Remote images of this sender always load.
    pub fn allow_sender(&self, email: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO remote_senders (email, added_at) VALUES (?1, ?2) ON CONFLICT (email) DO NOTHING",
            params![email.trim(), now()],
        )?;
        Ok(())
    }

    pub fn forbid_sender(&self, email: &str) -> Result<()> {
        self.conn.execute("DELETE FROM remote_senders WHERE email = ?1", [email.trim()])?;
        Ok(())
    }

    pub fn sender_allowed(&self, email: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM remote_senders WHERE email = ?1", [email.trim()], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn allowed_senders(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT email FROM remote_senders ORDER BY email COLLATE NOCASE")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Counts an address for the autocomplete (`sent`: the user wrote to it).
    pub fn touch_contact(&self, a: &Address, sent: bool, at: i64) -> Result<()> {
        if a.email.trim().is_empty() || !a.email.contains('@') {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO contacts (email, name, received, sent, last_used) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (email) DO UPDATE SET name = COALESCE(excluded.name, contacts.name),
               received = contacts.received + excluded.received, sent = contacts.sent + excluded.sent,
               last_used = MAX(contacts.last_used, excluded.last_used)",
            params![a.email.trim(), a.name, !sent as i64, sent as i64, at],
        )?;
        Ok(())
    }

    /// Addresses starting with `prefix` (in the address or in a word of the name), the ones
    /// written to most and most recently first.
    pub fn suggest_contacts(&self, prefix: &str, limit: u32) -> Result<Vec<Contact>> {
        let p = prefix.trim().to_lowercase();
        if p.is_empty() {
            return Ok(vec![]);
        }
        let like = format!("{}%", p.replace(['%', '_'], ""));
        let word = format!("% {}", like);
        let mut st = self.conn.prepare(
            "SELECT email, name FROM contacts
             WHERE lower(email) LIKE ?1 OR lower(name) LIKE ?1 OR lower(name) LIKE ?2
             ORDER BY sent * 4 + received DESC, last_used DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![like, word, limit], |r| Ok(Contact { email: r.get(0)?, name: r.get(1)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
    fn version_1_databases_are_migrated_with_their_mail() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO accounts (provider, email, transport, created_at) VALUES ('imap', 'a@example.org', '{}', 0);
             INSERT INTO mailboxes (account_id, name, role) VALUES (1, 'INBOX', 'inbox');
             INSERT INTO messages (account_id, mailbox_id, uid, from_name, from_email, internal_date)
               VALUES (1, 1, 1, 'Jonas', 'jonas@example.org', 5);",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(s.suggest_contacts("jon", 5).unwrap()[0].email, "jonas@example.org", "senders become contacts");
        assert_eq!(s.mailbox(1).unwrap().total, 1);
    }

    #[test]
    fn accounts_round_trip() {
        let (s, id, _) = store_with_inbox();
        let a = s.accounts().unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].id, id);
        assert_eq!(a[0].name, "Arbeit");
        assert_eq!(a[0].transport, preset(Provider::Imap, "maria@example.org").unwrap());
        s.update_account(id, "Büro", "Maria M.", "Viele Grüße\nMaria\n").unwrap();
        let a = s.account(id).unwrap();
        assert_eq!((a.name.as_str(), a.signature.as_str()), ("Büro", "Viele Grüße\nMaria"));
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
    fn hidden_messages_leave_lists_and_counts() {
        let (s, _, inbox) = store_with_inbox();
        let raw = sample("h@x", "Weg", None, "Text");
        let id = s.insert_message(inbox, &header(1, &raw), &parse(&raw).unwrap()).unwrap();
        s.set_hidden(&[id], true).unwrap();
        assert!(s.messages(inbox, 10, 0).unwrap().is_empty());
        assert_eq!((s.mailbox(inbox).unwrap().unread, s.unread_in_inboxes().unwrap()), (0, 0));
        assert_eq!(s.list_len(inbox, false).unwrap(), 0);
        s.set_hidden(&[id], false).unwrap();
        assert_eq!(s.unread_in_inboxes().unwrap(), 1);
    }

    #[test]
    fn threads_in_the_list() {
        let (s, _, inbox) = store_with_inbox();
        let add = |uid: u32, id: &str, reply: Option<&str>| {
            let raw = sample(id, "Angebot", reply, "x");
            let mut h = header(uid, &raw);
            h.flags.seen = uid != 2;
            s.insert_message(inbox, &h, &parse(&raw).unwrap()).unwrap()
        };
        add(1, "a@x", None);
        add(2, "b@x", Some("a@x"));
        add(3, "c@x", None);
        let threads = s.threads(inbox, 10, 0).unwrap();
        assert_eq!(threads.len(), 2);
        let t = threads.iter().find(|t| t.thread_count == 2).unwrap();
        assert!(!t.seen, "unread when one message is");
        assert_eq!(s.list_len(inbox, true).unwrap(), 2);
        assert_eq!(s.list_len(inbox, false).unwrap(), 3);
    }

    #[test]
    fn body_and_detail() {
        let (s, account, inbox) = store_with_inbox();
        let raw = sample("d@x", "Bericht", None, "Der ganze Text");
        let id = s.insert_message(inbox, &header(1, &raw), &parse(&raw).unwrap()).unwrap();
        assert!(!s.message(id).unwrap().has_body);
        s.set_body_raw(id, &parse(&raw).unwrap(), Some(&raw)).unwrap();
        let d = s.message(id).unwrap();
        assert!(d.has_body);
        assert_eq!(d.account_id, account);
        assert_eq!(d.text.as_deref().map(str::trim), Some("Der ganze Text"));
        assert_eq!(d.to.len(), 2);
        assert!(!d.remote_allowed);
        assert_eq!(s.raw_message(id).unwrap().as_deref(), Some(&raw[..]));
        s.allow_remote(id).unwrap();
        assert!(s.message(id).unwrap().remote_allowed);
        assert!(matches!(s.message(999), Err(Error::NotFound(_))));

        // A copy in another folder takes over the body.
        let archive = s
            .upsert_mailbox(account, &MailboxEntry { name: "Archiv".into(), delimiter: None, attributes: vec![] })
            .unwrap();
        let copy = s.insert_message(archive, &header(7, &raw), &parse(&raw).unwrap()).unwrap();
        assert!(s.message(copy).unwrap().has_body);
        assert_eq!(s.thread(s.message(copy).unwrap().summary.thread_id).unwrap().len(), 1, "one copy per message");
    }

    #[test]
    fn contacts_and_senders() {
        let (s, account, inbox) = store_with_inbox();
        let raw = sample("c@x", "Hallo", None, "x");
        s.insert_message(inbox, &header(1, &raw), &parse(&raw).unwrap()).unwrap();
        let sent = s
            .upsert_mailbox(
                account,
                &MailboxEntry { name: "Sent".into(), delimiter: None, attributes: vec!["\\Sent".into()] },
            )
            .unwrap();
        s.insert_message(sent, &header(1, &raw), &parse(&raw).unwrap()).unwrap();
        let hits = s.suggest_contacts("jo", 5).unwrap();
        assert_eq!(hits[0].email, "jonas@example.org", "written to: first");
        assert_eq!(s.suggest_contacts("must", 5).unwrap()[0].email, "maria@example.org", "a word of the name");
        assert!(s.suggest_contacts("", 5).unwrap().is_empty());
        assert!(!s.sender_allowed("news@example.org").unwrap());
        s.allow_sender("News@Example.org").unwrap();
        assert!(s.sender_allowed("news@example.org").unwrap());
        s.forbid_sender("news@example.org").unwrap();
        assert!(s.allowed_senders().unwrap().is_empty());
    }

    #[test]
    fn folders_rename_with_their_children() {
        let (s, account, _) = store_with_inbox();
        for name in ["Projekte", "Projekte/Website", "Projektarchiv"] {
            s.upsert_mailbox(account, &MailboxEntry { name: name.into(), delimiter: Some('/'), attributes: vec![] })
                .unwrap();
        }
        s.rename_mailbox(account, "Projekte", "Kunden").unwrap();
        let names: Vec<String> = s.mailboxes(account).unwrap().into_iter().map(|m| m.name).collect();
        assert!(
            names.contains(&"Kunden/Website".to_string()) && names.contains(&"Projektarchiv".to_string()),
            "{names:?}"
        );
    }
}
