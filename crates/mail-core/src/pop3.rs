//! POP3 accounts (RFC 1939, `STLS` RFC 2595, `UIDL`): the server has an inbox and nothing else,
//! so the app keeps the account's mail itself. New messages are downloaded whole into
//! `pop3_messages` (by their UIDL, once); the folders (Inbox, Sent, Drafts, Archive, Trash and
//! the user's own) live in the app. [`Pop3Session`] shows them to the sync engine as an
//! [`ImapClient`], so the worker, the store, search, rules and every action work unchanged.
//!
//! On the server, messages stay (`leave_on_server`), optionally until `delete_after_days`
//! after their download, and go when the user deleted them here; without „leave on server“
//! they are removed once downloaded. Sending is SMTP, as for IMAP accounts.

use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;

use crate::account::{Account, Credential, Security, Server, Transport};
use crate::error::{Error, Result};
use crate::imap::{
    BodyPart, Capabilities, FetchedHeader, FlagUpdate, Flags, IdleOutcome, ImapClient, MailboxEntry, QresyncParams,
    Search, SelectInfo, Selected, UidRange,
};
use crate::network::{self, MailStream, Problem};
use crate::store::{Store, now};
use crate::{smtp, tr};

/// How often the server is asked for new mail while the app runs.
pub const POLL: Duration = Duration::from_secs(120);
/// The UIDVALIDITY of every folder: the app numbers the messages itself and never reuses a
/// number.
const UID_VALIDITY: u32 = 1;

/// Providers that still offer POP3: name, address domains, POP3 server, SMTP server.
/// A provider: name, address domains, POP3 server (host, port), SMTP server.
pub type Preset = (&'static str, &'static [&'static str], (&'static str, u16), (&'static str, u16, Security));

pub const PRESETS: &[Preset] = &[
    (
        "GMX",
        &["gmx.de", "gmx.net", "gmx.at", "gmx.ch"],
        ("pop.gmx.net", 995),
        ("mail.gmx.net", 587, Security::StartTls),
    ),
    ("WEB.DE", &["web.de"], ("pop3.web.de", 995), ("smtp.web.de", 587, Security::StartTls)),
    (
        "T-Online",
        &["t-online.de", "magenta.de"],
        ("securepop.t-online.de", 995),
        ("securesmtp.t-online.de", 465, Security::Tls),
    ),
    ("Yahoo", &["yahoo.com", "yahoo.de"], ("pop.mail.yahoo.com", 995), ("smtp.mail.yahoo.com", 465, Security::Tls)),
    ("AOL", &["aol.com", "aol.de"], ("pop.aol.com", 995), ("smtp.aol.com", 465, Security::Tls)),
    ("Gmail", &["gmail.com", "googlemail.com"], ("pop.gmail.com", 995), ("smtp.gmail.com", 465, Security::Tls)),
    ("mailbox.org", &["mailbox.org"], ("pop3.mailbox.org", 995), ("smtp.mailbox.org", 465, Security::Tls)),
    ("Posteo", &["posteo.de", "posteo.net"], ("posteo.de", 995), ("posteo.de", 465, Security::Tls)),
    ("IONOS", &["ionos.de", "online.de"], ("pop.ionos.de", 995), ("smtp.ionos.de", 465, Security::Tls)),
    ("STRATO", &["strato.de"], ("pop3.strato.de", 995), ("smtp.strato.de", 465, Security::Tls)),
    ("freenet", &["freenet.de"], ("mx.freenet.de", 995), ("mx.freenet.de", 587, Security::StartTls)),
];

/// The POP3 settings of an address: a preset where the provider has one, else a guess
/// (`pop.<domain>` on 995, `smtp.<domain>` on 465).
pub fn preset(email: &str) -> Option<Transport> {
    let domain = crate::account::domain_of(email)?;
    let (pop, smtp) = PRESETS
        .iter()
        .find(|(_, domains, _, _)| domains.contains(&domain.as_str()))
        .map(|(_, _, p, s)| (Server::new(p.0, p.1, Security::Tls), Server::new(s.0, s.1, s.2)))
        .unwrap_or_else(|| {
            (
                Server::new(&format!("pop.{domain}"), 995, Security::Tls),
                Server::new(&format!("smtp.{domain}"), 465, Security::Tls),
            )
        });
    Some(Transport::Pop3 {
        pop3: pop,
        smtp,
        username: email.trim().to_string(),
        auth: crate::account::Auth::Password,
        leave_on_server: true,
        delete_after_days: None,
    })
}

// ---------------------------------------------------------------------------- the protocol

/// A signed-in POP3 connection.
pub struct Client {
    io: BufReader<MailStream>,
}

fn protocol(detail: impl Into<String>) -> Error {
    Problem::Protocol { detail: detail.into() }.into()
}

impl Client {
    /// Connects and signs in with USER and PASS.
    pub async fn connect(server: &Server, user: &str, password: &str) -> Result<Client> {
        let t = network::connect_pop3(server).await?;
        let mut c = Client { io: BufReader::new(t.stream) };
        if !t.greeted {
            let g = c.line().await?;
            if !g.starts_with("+OK") {
                return Err(protocol(g));
            }
        }
        let r = c.command(&format!("USER {user}")).await?;
        if !r.starts_with("+OK") {
            return Err(Problem::Auth { detail: r }.into());
        }
        let r = c.command(&format!("PASS {password}")).await?;
        if !r.starts_with("+OK") {
            return Err(Problem::Auth { detail: r.trim_start_matches("-ERR").trim().to_string() }.into());
        }
        Ok(c)
    }

    async fn line(&mut self) -> Result<String> {
        let mut s = String::new();
        let n = tokio::time::timeout(network::IO_TIMEOUT, self.io.read_line(&mut s))
            .await
            .map_err(|_| Error::from(Problem::Timeout { host: String::new(), port: 0 }))?
            .map_err(|e| Error::from(network::io_problem(&e)))?;
        if n == 0 {
            return Err(protocol(tr!("Verbindung geschlossen", "connection closed")));
        }
        Ok(s.trim_end_matches(['\r', '\n']).to_string())
    }

    async fn command(&mut self, cmd: &str) -> Result<String> {
        let s = self.io.get_mut();
        s.write_all(format!("{cmd}\r\n").as_bytes()).await.map_err(|e| Error::from(network::io_problem(&e)))?;
        s.flush().await.map_err(|e| Error::from(network::io_problem(&e)))?;
        self.line().await
    }

    /// A multi-line answer up to the line with the single dot, dot-stuffing undone.
    async fn multi(&mut self, cmd: &str) -> Result<Vec<u8>> {
        let first = self.command(cmd).await?;
        if !first.starts_with("+OK") {
            return Err(protocol(first));
        }
        let mut out = vec![];
        loop {
            let mut buf = vec![];
            let n = tokio::time::timeout(network::IO_TIMEOUT, self.io.read_until(b'\n', &mut buf))
                .await
                .map_err(|_| Error::from(Problem::Timeout { host: String::new(), port: 0 }))?
                .map_err(|e| Error::from(network::io_problem(&e)))?;
            if n == 0 {
                return Err(protocol(tr!("Verbindung geschlossen", "connection closed")));
            }
            if buf == b".\r\n" || buf == b".\n" {
                return Ok(out);
            }
            let line = if buf.starts_with(b"..") { &buf[1..] } else { &buf[..] };
            out.extend_from_slice(line);
        }
    }

    /// Message numbers and their unique ids.
    pub async fn uidl(&mut self) -> Result<Vec<(u32, String)>> {
        let raw = self.multi("UIDL").await?;
        Ok(String::from_utf8_lossy(&raw)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                Some((it.next()?.parse().ok()?, it.next()?.to_string()))
            })
            .collect())
    }

    pub async fn retr(&mut self, n: u32) -> Result<Vec<u8>> {
        self.multi(&format!("RETR {n}")).await
    }

    pub async fn dele(&mut self, n: u32) -> Result<()> {
        let r = self.command(&format!("DELE {n}")).await?;
        if !r.starts_with("+OK") {
            return Err(protocol(r));
        }
        Ok(())
    }

    /// Ends the session; the server removes what was marked with DELE only now.
    pub async fn quit(mut self) -> Result<()> {
        let r = self.command("QUIT").await?;
        if !r.starts_with("+OK") {
            return Err(protocol(r));
        }
        Ok(())
    }

    /// The number of messages (`STAT`).
    pub async fn stat(&mut self) -> Result<u32> {
        let r = self.command("STAT").await?;
        r.split_whitespace().nth(1).and_then(|n| n.parse().ok()).ok_or_else(|| protocol(r))
    }
}

/// Signs in to the POP3 server and counts the messages (the setup's check).
pub async fn check(server: &Server, user: &str, password: &str) -> Result<u32> {
    let mut c = Client::connect(server, user, password).await?;
    let n = c.stat().await?;
    let _ = c.quit().await;
    Ok(n)
}

// ---------------------------------------------------------------------------- the local folders

/// The folders a POP3 account has from the start, with their roles.
const FOLDERS: &[(&str, &str)] =
    &[("INBOX", ""), ("Sent", "\\Sent"), ("Drafts", "\\Drafts"), ("Archive", "\\Archive"), ("Trash", "\\Trash")];

fn ensure_folders(store: &Store, account: i64) -> Result<()> {
    for (name, _) in FOLDERS {
        store
            .conn()
            .execute("INSERT OR IGNORE INTO pop3_folders (account_id, name) VALUES (?1, ?2)", params![account, name])?;
    }
    Ok(())
}

fn next_uid(store: &Store, account: i64, mailbox: &str) -> Result<u32> {
    let uid: Option<u32> = store
        .conn()
        .query_row(
            "SELECT uid_next FROM pop3_folders WHERE account_id = ?1 AND name = ?2",
            params![account, mailbox],
            |r| r.get(0),
        )
        .optional()?;
    let Some(uid) = uid else {
        return Err(Error::Imap(format!("NO no such folder {mailbox}")));
    };
    store.conn().execute(
        "UPDATE pop3_folders SET uid_next = uid_next + 1, modseq = modseq + 1 WHERE account_id = ?1 AND name = ?2",
        params![account, mailbox],
    )?;
    Ok(uid)
}

fn header_of(raw: &[u8]) -> Vec<u8> {
    match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => raw[..i + 4].to_vec(),
        None => match raw.windows(2).position(|w| w == b"\n\n") {
            Some(i) => raw[..i + 2].to_vec(),
            None => raw.to_vec(),
        },
    }
}

/// Stores a downloaded or appended message in a folder; returns its UID.
fn insert(
    store: &Store,
    account: i64,
    mailbox: &str,
    uidl: Option<&str>,
    raw: &[u8],
    flags: &Flags,
    at: i64,
) -> Result<u32> {
    let uid = next_uid(store, account, mailbox)?;
    store.conn().execute(
        "INSERT INTO pop3_messages (account_id, mailbox, uid, uidl, raw, seen, answered, flagged, draft, keywords, internal_date)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            account,
            mailbox,
            uid,
            uidl,
            raw,
            flags.seen,
            flags.answered,
            flags.flagged,
            flags.draft,
            serde_json::to_string(&flags.keywords)?,
            at
        ],
    )?;
    Ok(uid)
}

/// What a visit to the server did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Visit {
    pub downloaded: u32,
    pub removed: u32,
}

/// How the account keeps mail on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub leave_on_server: bool,
    pub delete_after_days: Option<u32>,
}

/// Downloads new messages into the inbox and removes from the server what should go: what was
/// deleted here, what is older than the days to keep, or everything downloaded when nothing
/// stays on the server. One session; DELE takes effect at QUIT.
pub async fn visit(
    store: &Mutex<Store>,
    account: i64,
    server: &Server,
    user: &str,
    password: &str,
    policy: Policy,
) -> Result<Visit> {
    let lock = || store.lock().unwrap_or_else(|e| e.into_inner());
    ensure_folders(&lock(), account)?;
    let mut c = Client::connect(server, user, password).await?;
    let list = c.uidl().await?;
    let mut v = Visit::default();
    let at = now();
    let mut removed: Vec<&str> = vec![];
    for (n, uidl) in &list {
        let known: Option<(i64, bool)> = lock()
            .conn()
            .query_row(
                "SELECT first_seen, deleted FROM pop3_seen WHERE account_id = ?1 AND uidl = ?2",
                params![account, uidl],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let remove = match known {
            None => {
                let raw = c.retr(*n).await?;
                let date = crate::parse::parse(&raw).ok().and_then(|p| p.date).unwrap_or(at);
                {
                    let s = lock();
                    s.batch(|| {
                        insert(&s, account, "INBOX", Some(uidl), &raw, &Flags::default(), date.min(at))?;
                        s.conn().execute(
                            "INSERT INTO pop3_seen (account_id, uidl, first_seen) VALUES (?1, ?2, ?3)",
                            params![account, uidl, at],
                        )?;
                        Ok(())
                    })?;
                }
                v.downloaded += 1;
                !policy.leave_on_server
            }
            Some((first, deleted)) => {
                deleted
                    || !policy.leave_on_server
                    || policy.delete_after_days.is_some_and(|d| at - first >= d as i64 * 86400)
            }
        };
        if remove {
            c.dele(*n).await?;
            removed.push(uidl);
            v.removed += 1;
        }
    }
    c.quit().await?;
    // What left the server (now, or by another program) is forgotten: it cannot come again.
    let s = lock();
    let known: Vec<String> = {
        let mut st = s.conn().prepare("SELECT uidl FROM pop3_seen WHERE account_id = ?1")?;
        st.query_map([account], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    for u in known {
        if removed.contains(&u.as_str()) || !list.iter().any(|(_, x)| *x == u) {
            s.conn().execute("DELETE FROM pop3_seen WHERE account_id = ?1 AND uidl = ?2", params![account, u])?;
        }
    }
    Ok(v)
}

// ---------------------------------------------------------------------------- as an IMAP client

/// A stored message: UID, UIDL, the message, its flags and when it arrived.
type Row = (u32, Option<String>, Vec<u8>, Flags, i64);

/// A POP3 account's folders as the sync engine sees them.
pub struct Pop3Session {
    store: Mutex<Store>,
    account: i64,
    server: Server,
    user: String,
    password: String,
    policy: Policy,
    selected: Option<String>,
    poll: Duration,
}

impl Pop3Session {
    /// Opens the session and visits the server once (a refused sign-in shows at once).
    pub async fn open(
        store: Store,
        account: i64,
        server: Server,
        user: String,
        password: String,
        policy: Policy,
        poll: Duration,
    ) -> Result<Self> {
        let s = Pop3Session { store: Mutex::new(store), account, server, user, password, policy, selected: None, poll };
        s.refresh().await?;
        Ok(s)
    }

    async fn refresh(&self) -> Result<Visit> {
        visit(&self.store, self.account, &self.server, &self.user, &self.password, self.policy).await
    }

    fn with<T>(&self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        f(&self.store.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn mailbox(&self) -> Result<String> {
        self.selected.clone().ok_or_else(|| Error::Imap("BAD no folder selected".into()))
    }

    fn rows(&self, mailbox: &str, uids: &[u32]) -> Result<Vec<Row>> {
        self.with(|s| {
            let mut st = s.conn().prepare(
                "SELECT uid, uidl, raw, seen, answered, flagged, draft, keywords, internal_date FROM pop3_messages
                 WHERE account_id = ?1 AND mailbox = ?2 ORDER BY uid",
            )?;
            let rows = st.query_map(params![self.account, mailbox], |r| {
                let kw: String = r.get(7)?;
                Ok((
                    r.get::<_, u32>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    Flags {
                        seen: r.get(3)?,
                        answered: r.get(4)?,
                        flagged: r.get(5)?,
                        draft: r.get(6)?,
                        deleted: false,
                        keywords: serde_json::from_str(&kw).unwrap_or_default(),
                    },
                    r.get::<_, i64>(8)?,
                ))
            })?;
            let all: Vec<_> = rows.collect::<rusqlite::Result<_>>()?;
            Ok(if uids.is_empty() { all } else { all.into_iter().filter(|r| uids.contains(&r.0)).collect() })
        })
    }

    fn in_range(&self, range: UidRange) -> Result<Vec<Row>> {
        Ok(self.rows(&self.mailbox()?, &[])?.into_iter().filter(|r| range.contains(r.0)).collect())
    }

    fn copy_into(&self, uids: &[u32], to: &str, remove: bool) -> Result<Vec<(u32, u32)>> {
        let from = self.mailbox()?;
        let rows = self.rows(&from, uids)?;
        self.with(|s| {
            s.batch(|| {
                let mut out = vec![];
                for (uid, uidl, raw, flags, at) in &rows {
                    let new = insert(s, self.account, to, uidl.as_deref(), raw, flags, *at)?;
                    out.push((*uid, new));
                    if remove {
                        s.conn().execute(
                            "DELETE FROM pop3_messages WHERE account_id = ?1 AND mailbox = ?2 AND uid = ?3",
                            params![self.account, from, uid],
                        )?;
                    }
                }
                s.conn().execute(
                    "UPDATE pop3_folders SET modseq = modseq + 1 WHERE account_id = ?1 AND name = ?2",
                    params![self.account, from],
                )?;
                Ok(out)
            })
        })
    }
}

fn message_id_of(raw: &[u8]) -> Option<String> {
    crate::parse::parse(&header_of(raw)).ok().and_then(|p| p.message_id)
}

impl ImapClient for Pop3Session {
    async fn capabilities(&mut self) -> Result<Capabilities> {
        Ok(Capabilities { uidplus: true, moves: true, special_use: true, ..Default::default() })
    }

    async fn list(&mut self) -> Result<Vec<MailboxEntry>> {
        self.with(|s| {
            ensure_folders(s, self.account)?;
            let mut st = s.conn().prepare("SELECT name FROM pop3_folders WHERE account_id = ?1 ORDER BY name")?;
            let names: Vec<String> = st.query_map([self.account], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            Ok(names
                .into_iter()
                .map(|name| {
                    let role = FOLDERS.iter().find(|(n, _)| *n == name).map(|(_, r)| *r).filter(|r| !r.is_empty());
                    MailboxEntry {
                        attributes: role.map(|r| vec![r.to_string()]).unwrap_or_default(),
                        delimiter: Some('/'),
                        name,
                    }
                })
                .collect())
        })
    }

    async fn select(&mut self, mailbox: &str, _: Option<QresyncParams>) -> Result<Selected> {
        let (uid_next, exists): (Option<u32>, u32) = self.with(|s| {
            let next = s
                .conn()
                .query_row(
                    "SELECT uid_next FROM pop3_folders WHERE account_id = ?1 AND name = ?2",
                    params![self.account, mailbox],
                    |r| r.get(0),
                )
                .optional()?;
            let n = s.conn().query_row(
                "SELECT COUNT(*) FROM pop3_messages WHERE account_id = ?1 AND mailbox = ?2",
                params![self.account, mailbox],
                |r| r.get(0),
            )?;
            Ok((next, n))
        })?;
        let Some(uid_next) = uid_next else { return Err(Error::Imap(format!("NO no such folder {mailbox}"))) };
        self.selected = Some(mailbox.to_string());
        Ok(Selected {
            info: SelectInfo { uid_validity: UID_VALIDITY, uid_next, exists, highest_modseq: None },
            vanished: vec![],
            changed: vec![],
        })
    }

    async fn uids(&mut self) -> Result<Vec<u32>> {
        Ok(self.rows(&self.mailbox()?, &[])?.into_iter().map(|r| r.0).collect())
    }

    async fn fetch_headers(&mut self, range: UidRange) -> Result<Vec<FetchedHeader>> {
        Ok(self
            .in_range(range)?
            .into_iter()
            .map(|(uid, _, raw, flags, at)| FetchedHeader {
                uid,
                flags,
                modseq: None,
                internal_date: at,
                size: raw.len() as u32,
                header: header_of(&raw),
            })
            .collect())
    }

    async fn fetch_flags(&mut self, range: UidRange, _: Option<u64>) -> Result<Vec<FlagUpdate>> {
        Ok(self
            .in_range(range)?
            .into_iter()
            .map(|(uid, _, _, flags, _)| FlagUpdate { uid, flags, modseq: None })
            .collect())
    }

    async fn fetch_message(&mut self, uid: u32) -> Result<Vec<u8>> {
        self.rows(&self.mailbox()?, &[uid])?
            .into_iter()
            .next()
            .map(|r| r.2)
            .ok_or_else(|| Error::Imap(format!("NO no message {uid}")))
    }

    async fn fetch_structure(&mut self, uid: u32) -> Result<Vec<BodyPart>> {
        Ok(crate::parse::structure(&self.fetch_message(uid).await?))
    }

    async fn fetch_section(&mut self, uid: u32, section: &str) -> Result<Vec<u8>> {
        let raw = self.fetch_message(uid).await?;
        crate::parse::section(&raw, section).ok_or_else(|| Error::Imap(format!("NO no section {section}")))
    }

    async fn idle(&mut self, timeout: Duration, wake: &Notify) -> Result<IdleOutcome> {
        let wait = timeout.min(self.poll);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = wake.notified() => return Ok(IdleOutcome::Interrupted),
        }
        let v = self.refresh().await?;
        Ok(if v.downloaded > 0 { IdleOutcome::Changed } else { IdleOutcome::Timeout })
    }

    async fn store(&mut self, uids: &[u32], flags: &[&str], add: bool) -> Result<()> {
        let mailbox = self.mailbox()?;
        let rows = self.rows(&mailbox, uids)?;
        self.with(|s| {
            for (uid, _, _, mut f, _) in rows {
                for flag in flags {
                    match flag.to_ascii_lowercase().as_str() {
                        "\\seen" => f.seen = add,
                        "\\answered" => f.answered = add,
                        "\\flagged" => f.flagged = add,
                        "\\draft" => f.draft = add,
                        "\\deleted" => {}
                        _ => {
                            f.keywords.retain(|k| !k.eq_ignore_ascii_case(flag));
                            if add {
                                f.keywords.push(flag.to_string());
                            }
                        }
                    }
                }
                s.conn().execute(
                    "UPDATE pop3_messages SET seen = ?4, answered = ?5, flagged = ?6, draft = ?7, keywords = ?8
                     WHERE account_id = ?1 AND mailbox = ?2 AND uid = ?3",
                    params![
                        self.account,
                        mailbox,
                        uid,
                        f.seen,
                        f.answered,
                        f.flagged,
                        f.draft,
                        serde_json::to_string(&f.keywords)?
                    ],
                )?;
            }
            Ok(())
        })
    }

    async fn copy(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        self.copy_into(uids, to, false)
    }

    async fn move_to(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        self.copy_into(uids, to, true)
    }

    async fn expunge(&mut self, uids: &[u32]) -> Result<()> {
        let mailbox = self.mailbox()?;
        let rows = self.rows(&mailbox, uids)?;
        self.with(|s| {
            s.batch(|| {
                for (uid, uidl, ..) in &rows {
                    s.conn().execute(
                        "DELETE FROM pop3_messages WHERE account_id = ?1 AND mailbox = ?2 AND uid = ?3",
                        params![self.account, mailbox, uid],
                    )?;
                    // Deleted here: off the server at the next visit (unless a copy is kept).
                    if let Some(u) = uidl {
                        let copies: u32 = s.conn().query_row(
                            "SELECT COUNT(*) FROM pop3_messages WHERE account_id = ?1 AND uidl = ?2",
                            params![self.account, u],
                            |r| r.get(0),
                        )?;
                        if copies == 0 {
                            s.conn().execute(
                                "UPDATE pop3_seen SET deleted = 1 WHERE account_id = ?1 AND uidl = ?2",
                                params![self.account, u],
                            )?;
                        }
                    }
                }
                Ok(())
            })
        })
    }

    async fn append(&mut self, mailbox: &str, flags: &[&str], raw: &[u8]) -> Result<Option<u32>> {
        let f = Flags {
            seen: flags.iter().any(|x| x.eq_ignore_ascii_case("\\Seen")),
            draft: flags.iter().any(|x| x.eq_ignore_ascii_case("\\Draft")),
            ..Default::default()
        };
        self.with(|s| insert(s, self.account, mailbox, None, raw, &f, now()).map(Some))
    }

    async fn search(&mut self, query: &Search) -> Result<Vec<u32>> {
        let rows = self.rows(&self.mailbox()?, &[])?;
        Ok(match query {
            Search::MessageId(mid) => rows
                .into_iter()
                .filter(|r| message_id_of(&r.2).as_deref() == Some(mid.trim_matches(['<', '>'])))
                .map(|r| r.0)
                .collect(),
            Search::Text(t) => {
                let t = t.to_lowercase();
                rows.into_iter()
                    .filter(|r| String::from_utf8_lossy(&r.2).to_lowercase().contains(&t))
                    .map(|r| r.0)
                    .collect()
            }
        })
    }

    async fn create(&mut self, mailbox: &str) -> Result<()> {
        self.with(|s| {
            s.conn().execute(
                "INSERT OR IGNORE INTO pop3_folders (account_id, name) VALUES (?1, ?2)",
                params![self.account, mailbox],
            )?;
            Ok(())
        })
    }

    async fn delete(&mut self, mailbox: &str) -> Result<()> {
        if FOLDERS.iter().any(|(n, _)| *n == mailbox) {
            return Err(Error::Imap("NO this folder stays".into()));
        }
        self.with(|s| {
            s.conn().execute(
                "DELETE FROM pop3_messages WHERE account_id = ?1 AND mailbox = ?2",
                params![self.account, mailbox],
            )?;
            s.conn().execute(
                "DELETE FROM pop3_folders WHERE account_id = ?1 AND name = ?2",
                params![self.account, mailbox],
            )?;
            Ok(())
        })
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.with(|s| {
            s.conn().execute(
                "UPDATE pop3_folders SET name = ?3 WHERE account_id = ?1 AND name = ?2",
                params![self.account, from, to],
            )?;
            s.conn().execute(
                "UPDATE pop3_messages SET mailbox = ?3 WHERE account_id = ?1 AND mailbox = ?2",
                params![self.account, from, to],
            )?;
            Ok(())
        })
    }

    async fn logout(&mut self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------- the connector

/// The servers of a POP3 account, for the account worker.
pub struct Pop3Connector {
    account_id: i64,
    pop3: Server,
    smtp: Server,
    username: String,
    policy: Policy,
    password: Mutex<String>,
    db: std::path::PathBuf,
    pub poll: Duration,
}

impl Pop3Connector {
    pub fn new(account: &Account, password: String, db: std::path::PathBuf) -> Result<Self> {
        let Transport::Pop3 { pop3, smtp, username, leave_on_server, delete_after_days, .. } = &account.transport
        else {
            return Err(Error::Invalid(tr!("Kein POP3-Konto.", "Not a POP3 account.").into()));
        };
        Ok(Pop3Connector {
            account_id: account.id,
            pop3: pop3.clone(),
            smtp: smtp.clone(),
            username: username.clone(),
            policy: Policy { leave_on_server: *leave_on_server, delete_after_days: *delete_after_days },
            password: Mutex::new(password),
            db,
            poll: POLL,
        })
    }

    fn password(&self) -> String {
        self.password.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl crate::engine::Connector for Pop3Connector {
    type Imap = Pop3Session;

    async fn imap(&self) -> Result<Pop3Session> {
        let store = Store::open(&self.db)?;
        Pop3Session::open(
            store,
            self.account_id,
            self.pop3.clone(),
            self.username.clone(),
            self.password(),
            self.policy,
            self.poll,
        )
        .await
    }

    async fn send(&self, from: &str, to: &[String], raw: &[u8]) -> Result<()> {
        smtp::send_with(&self.smtp, &self.username, &Credential::Password(self.password()), from, to, raw).await
    }

    fn files_sent(&self) -> bool {
        false
    }

    fn set_password(&self, password: String) {
        *self.password.lock().unwrap_or_else(|e| e.into_inner()) = password;
    }
}

#[cfg(any(test, feature = "fake"))]
pub mod fake {
    //! A POP3 server's state machine, without sockets (the core's tests and the test server):
    //! a maildrop, USER/PASS, STAT, LIST, UIDL, RETR, TOP, DELE, RSET, NOOP and QUIT; the
    //! deletions take effect at QUIT.

    #[derive(Debug, Clone)]
    pub struct FakePop3 {
        pub user: String,
        pub password: String,
        /// (UIDL, message, marked for deletion)
        pub messages: Vec<(String, Vec<u8>, bool)>,
        state: State,
        /// Every command, for the tests.
        pub log: Vec<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum State {
        User,
        Pass(String),
        Transaction,
    }

    fn stuffed(raw: &[u8]) -> String {
        let text = String::from_utf8_lossy(raw).replace("\r\n", "\n");
        let mut out = String::new();
        for line in text.trim_end_matches('\n').split('\n') {
            if line.starts_with('.') {
                out.push('.');
            }
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str(".\r\n");
        out
    }

    impl FakePop3 {
        pub fn new(user: &str, password: &str) -> Self {
            FakePop3 { user: user.into(), password: password.into(), messages: vec![], state: State::User, log: vec![] }
        }

        pub fn greeting(&self) -> String {
            "+OK Arcalo Mail test server ready\r\n".into()
        }

        /// Answers one command; the answer and whether the session ends. At QUIT, the messages
        /// marked with DELE go.
        pub fn handle(&mut self, line: &str) -> (String, bool) {
            let mut parts = line.trim().splitn(2, ' ');
            let cmd = parts.next().unwrap_or_default().to_ascii_uppercase();
            let arg = parts.next().unwrap_or_default().trim().to_string();
            self.log.push(if cmd == "PASS" { "PASS ***".into() } else { line.trim().to_string() });
            let n = arg
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| *n >= 1 && *n <= self.messages.len());
            let live = |m: &(String, Vec<u8>, bool)| !m.2;
            match (&self.state, cmd.as_str()) {
                (_, "QUIT") => {
                    if self.state == State::Transaction {
                        self.messages.retain(|m| !m.2);
                    }
                    self.state = State::User;
                    ("+OK bye\r\n".into(), true)
                }
                (State::User, "USER") => {
                    self.state = State::Pass(arg);
                    ("+OK\r\n".into(), false)
                }
                (State::Pass(u), "PASS") => {
                    if u.eq_ignore_ascii_case(&self.user) && arg == self.password {
                        self.state = State::Transaction;
                        ("+OK signed in\r\n".into(), false)
                    } else {
                        self.state = State::User;
                        ("-ERR [AUTH] wrong user name or password\r\n".into(), false)
                    }
                }
                (State::Transaction, "STAT") => {
                    let list: Vec<_> = self.messages.iter().filter(|m| live(m)).collect();
                    (format!("+OK {} {}\r\n", list.len(), list.iter().map(|m| m.1.len()).sum::<usize>()), false)
                }
                (State::Transaction, "UIDL") => {
                    let mut out = "+OK\r\n".to_string();
                    for (i, m) in self.messages.iter().enumerate().filter(|(_, m)| live(m)) {
                        out.push_str(&format!("{} {}\r\n", i + 1, m.0));
                    }
                    out.push_str(".\r\n");
                    (out, false)
                }
                (State::Transaction, "LIST") => {
                    let mut out = "+OK\r\n".to_string();
                    for (i, m) in self.messages.iter().enumerate().filter(|(_, m)| live(m)) {
                        out.push_str(&format!("{} {}\r\n", i + 1, m.1.len()));
                    }
                    out.push_str(".\r\n");
                    (out, false)
                }
                (State::Transaction, "RETR") => match n.filter(|n| live(&self.messages[n - 1])) {
                    Some(n) => (format!("+OK message follows\r\n{}", stuffed(&self.messages[n - 1].1)), false),
                    None => ("-ERR no such message\r\n".into(), false),
                },
                (State::Transaction, "TOP") => match n.filter(|n| live(&self.messages[n - 1])) {
                    Some(n) => {
                        let raw = &self.messages[n - 1].1;
                        let end = raw.windows(4).position(|w| w == b"\r\n\r\n").map_or(raw.len(), |i| i + 4);
                        (format!("+OK\r\n{}", stuffed(&raw[..end])), false)
                    }
                    None => ("-ERR no such message\r\n".into(), false),
                },
                (State::Transaction, "DELE") => match n.filter(|n| live(&self.messages[n - 1])) {
                    Some(n) => {
                        self.messages[n - 1].2 = true;
                        ("+OK marked\r\n".into(), false)
                    }
                    None => ("-ERR no such message\r\n".into(), false),
                },
                (State::Transaction, "RSET") => {
                    self.messages.iter_mut().for_each(|m| m.2 = false);
                    ("+OK\r\n".into(), false)
                }
                (State::Transaction, "NOOP") => ("+OK\r\n".into(), false),
                (_, "CAPA") => ("+OK\r\nUSER\r\nUIDL\r\nTOP\r\n.\r\n".into(), false),
                _ => ("-ERR unknown command\r\n".into(), false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::fake::FakePop3;
    use super::*;
    use crate::engine::Connector;

    async fn serve(fake: Arc<Mutex<FakePop3>>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else { return };
                let fake = fake.clone();
                tokio::spawn(async move {
                    let mut io = BufReader::new(s);
                    let g = fake.lock().unwrap().greeting();
                    let _ = io.get_mut().write_all(g.as_bytes()).await;
                    loop {
                        let mut line = String::new();
                        if io.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let (answer, bye) = fake.lock().unwrap().handle(&line);
                        let _ = io.get_mut().write_all(answer.as_bytes()).await;
                        if bye {
                            return;
                        }
                    }
                });
            }
        });
        port
    }

    fn mail(id: &str, subject: &str) -> Vec<u8> {
        format!("From: Jonas <jonas@example.org>\r\nTo: maria@example.org\r\nSubject: {subject}\r\nMessage-ID: <{id}>\r\nDate: Fri, 16 Oct 2026 09:30:00 +0000\r\n\r\nHallo\r\n.Punkt am Anfang\r\n").into_bytes()
    }

    fn setup(policy: Policy) -> (Arc<Mutex<FakePop3>>, std::path::PathBuf, Account) {
        let fake = Arc::new(Mutex::new(FakePop3::new("maria@example.org", "geheim")));
        fake.lock().unwrap().messages.push(("u1".into(), mail("m1@x", "Eins"), false));
        fake.lock().unwrap().messages.push(("u2".into(), mail("m2@x", "Zwei"), false));
        let dir = std::env::temp_dir().join(format!("mail-core-pop3-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("mail.db");
        let store = Store::open(&db).unwrap();
        let mut transport = preset("maria@example.org").unwrap();
        if let Transport::Pop3 { leave_on_server, delete_after_days, pop3, .. } = &mut transport {
            *leave_on_server = policy.leave_on_server;
            *delete_after_days = policy.delete_after_days;
            *pop3 = Server::new("127.0.0.1", 1, Security::Plain);
        }
        let id = store
            .add_account(&crate::account::AccountDraft {
                provider: crate::account::Provider::Imap,
                name: String::new(),
                email: "maria@example.org".into(),
                sender_name: String::new(),
                transport,
            })
            .unwrap();
        let account = store.account(id).unwrap();
        (fake, db, account)
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed) * 1000
            + std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_micros() as u64
    }

    #[tokio::test]
    async fn downloads_once_and_leaves_mail_on_the_server() {
        let (fake, db, account) = setup(Policy { leave_on_server: true, delete_after_days: None });
        let port = serve(fake.clone()).await;
        let server = Server::new("127.0.0.1", port, Security::Plain);
        let mut s = Pop3Session::open(
            Store::open(&db).unwrap(),
            account.id,
            server.clone(),
            "maria@example.org".into(),
            "geheim".into(),
            Policy { leave_on_server: true, delete_after_days: None },
            POLL,
        )
        .await
        .unwrap();
        // The sync engine over it.
        let store = Store::open(&db).unwrap();
        crate::sync::sync_folders(&mut s, &store, account.id).await.unwrap();
        let inbox = store.mailbox_by_role(account.id, crate::imap::Role::Inbox).unwrap().unwrap();
        assert!(store.mailbox_by_role(account.id, crate::imap::Role::Sent).unwrap().is_some());
        let r = crate::sync::sync_mailbox(&mut s, &store, Capabilities::default(), inbox.id).await.unwrap();
        assert_eq!(r.added, 2);
        let list = store.messages(inbox.id, 10, 0).unwrap();
        assert_eq!(list.len(), 2);
        // Dot-stuffing undone.
        let raw = s.fetch_message(list[0].uid).await.unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("\r\n.Punkt am Anfang"));
        assert_eq!(fake.lock().unwrap().messages.len(), 2, "left on the server");
        // A second visit downloads nothing again; a new message comes.
        fake.lock().unwrap().messages.push(("u3".into(), mail("m3@x", "Drei"), false));
        let v = s.refresh().await.unwrap();
        assert_eq!((v.downloaded, v.removed), (1, 0));
        // Deleted here (to the trash, then emptied): off the server at the next visit.
        s.select("INBOX", None).await.unwrap();
        let moved = s.move_to(&[1], "Trash").await.unwrap();
        s.select("Trash", None).await.unwrap();
        s.expunge(&[moved[0].1]).await.unwrap();
        let v = s.refresh().await.unwrap();
        assert_eq!(v.removed, 1);
        let left: Vec<String> = fake.lock().unwrap().messages.iter().map(|m| m.0.clone()).collect();
        assert_eq!(left, ["u2", "u3"]);
        // Flags and an appended copy for Sent stay here.
        s.select("INBOX", None).await.unwrap();
        s.store(&[2], &["\\Seen", "$Rechnung"], true).await.unwrap();
        let f = s.fetch_flags(UidRange { from: 1, to: None }, None).await.unwrap();
        assert!(f.iter().any(|x| x.uid == 2 && x.flags.seen && x.flags.keywords == ["$Rechnung"]));
        assert_eq!(s.append("Sent", &["\\Seen"], &mail("s1@x", "Gesendet")).await.unwrap(), Some(1));
        s.select("Sent", None).await.unwrap();
        assert_eq!(s.search(&Search::MessageId("s1@x".into())).await.unwrap(), [1]);
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    #[tokio::test]
    async fn removes_after_download_or_after_days() {
        let (fake, db, account) = setup(Policy { leave_on_server: false, delete_after_days: None });
        let port = serve(fake.clone()).await;
        let server = Server::new("127.0.0.1", port, Security::Plain);
        let store = Mutex::new(Store::open(&db).unwrap());
        let v = visit(
            &store,
            account.id,
            &server,
            "maria@example.org",
            "geheim",
            Policy { leave_on_server: false, delete_after_days: None },
        )
        .await
        .unwrap();
        assert_eq!((v.downloaded, v.removed), (2, 2));
        assert!(fake.lock().unwrap().messages.is_empty(), "removed at QUIT");
        // Kept for N days: the first visit leaves them, a visit after N days removes them.
        fake.lock().unwrap().messages.push(("u9".into(), mail("m9@x", "Neun"), false));
        let keep = Policy { leave_on_server: true, delete_after_days: Some(3) };
        assert_eq!(visit(&store, account.id, &server, "maria@example.org", "geheim", keep).await.unwrap().removed, 0);
        store.lock().unwrap().conn().execute("UPDATE pop3_seen SET first_seen = first_seen - 4 * 86400", []).unwrap();
        assert_eq!(visit(&store, account.id, &server, "maria@example.org", "geheim", keep).await.unwrap().removed, 1);
        assert!(fake.lock().unwrap().messages.is_empty());
        let local: u32 =
            store.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM pop3_messages", [], |r| r.get(0)).unwrap();
        assert_eq!(local, 3, "the app keeps them");
        // A wrong password.
        let e = check(&server, "maria@example.org", "falsch").await.unwrap_err();
        assert!(matches!(e, Error::Connection(Problem::Auth { .. })), "{e:?}");
        assert_eq!(check(&server, "maria@example.org", "geheim").await.unwrap(), 0);
        // The connector of the worker.
        let c = Pop3Connector::new(
            &Account {
                transport: Transport::Pop3 {
                    pop3: server.clone(),
                    smtp: Server::new("127.0.0.1", 1, Security::Plain),
                    username: "maria@example.org".into(),
                    auth: crate::account::Auth::Password,
                    leave_on_server: true,
                    delete_after_days: None,
                },
                ..account.clone()
            },
            "geheim".into(),
            db.clone(),
        )
        .unwrap();
        assert!(!c.files_sent());
        let mut session = c.imap().await.unwrap();
        assert_eq!(session.list().await.unwrap().len(), 5);
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    #[test]
    fn presets() {
        let Some(Transport::Pop3 { pop3, smtp, .. }) = preset("maria@gmx.de") else { panic!() };
        assert_eq!((pop3.host.as_str(), pop3.port, smtp.port), ("pop.gmx.net", 995, 587));
        let Some(Transport::Pop3 { pop3, .. }) = preset("a@example.org") else { panic!() };
        assert_eq!(pop3.host, "pop.example.org");
        let draft = crate::account::AccountDraft {
            provider: crate::account::Provider::Imap,
            name: String::new(),
            email: "a@web.de".into(),
            sender_name: String::new(),
            transport: preset("a@web.de").unwrap(),
        };
        crate::account::validate(&draft).unwrap();
    }
}
