//! The IMAP client as the sync engine sees it: a trait ([`ImapClient`]) with the few operations
//! the engine needs, and plain data types for the answers.
//!
//! The engine ([`crate::sync`]) is written against the trait only. The real client (async-imap
//! over a TLS stream from [`crate::network`], milestone 1) and the in-memory server for tests
//! ([`fake::FakeImap`]) implement it. Every message is addressed by UID; sequence numbers never
//! leave the client.

#[cfg(any(test, feature = "fake"))]
pub mod fake;

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// The extensions the engine uses when the server offers them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// RFC 2177: the server tells about new mail while the client waits.
    pub idle: bool,
    /// RFC 7162: per-message MODSEQ, `CHANGEDSINCE`.
    pub condstore: bool,
    /// RFC 7162: `SELECT … (QRESYNC …)` answers with the changed and the vanished UIDs.
    pub qresync: bool,
    /// RFC 4315: `APPENDUID`, `UID EXPUNGE`.
    pub uidplus: bool,
    /// RFC 6851.
    pub moves: bool,
    /// RFC 6154: `\Sent`, `\Trash` … on folders.
    pub special_use: bool,
}

impl Capabilities {
    /// From the atoms of a `CAPABILITY` answer (case does not matter).
    pub fn parse<'a>(atoms: impl IntoIterator<Item = &'a str>) -> Self {
        let mut c = Capabilities::default();
        for a in atoms {
            match a.to_ascii_uppercase().as_str() {
                "IDLE" => c.idle = true,
                "CONDSTORE" => c.condstore = true,
                // QRESYNC implies CONDSTORE (RFC 7162, 3.2.3).
                "QRESYNC" => (c.qresync, c.condstore) = (true, true),
                "UIDPLUS" => c.uidplus = true,
                "MOVE" => c.moves = true,
                "SPECIAL-USE" => c.special_use = true,
                _ => {}
            }
        }
        c
    }
}

/// What a folder is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    All,
    Flagged,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Inbox => "inbox",
            Role::Sent => "sent",
            Role::Drafts => "drafts",
            Role::Trash => "trash",
            Role::Junk => "junk",
            Role::Archive => "archive",
            Role::All => "all",
            Role::Flagged => "flagged",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        [Role::Inbox, Role::Sent, Role::Drafts, Role::Trash, Role::Junk, Role::Archive, Role::All, Role::Flagged]
            .into_iter()
            .find(|r| r.as_str() == s)
    }

    /// From the SPECIAL-USE attributes, else from common names of servers without them.
    pub fn detect(name: &str, attributes: &[String]) -> Option<Role> {
        if name.eq_ignore_ascii_case("INBOX") {
            return Some(Role::Inbox);
        }
        for a in attributes {
            let r = match a.to_ascii_lowercase().as_str() {
                "\\sent" => Role::Sent,
                "\\drafts" => Role::Drafts,
                "\\trash" => Role::Trash,
                "\\junk" => Role::Junk,
                "\\archive" => Role::Archive,
                "\\all" => Role::All,
                "\\flagged" => Role::Flagged,
                _ => continue,
            };
            return Some(r);
        }
        let leaf = name.rsplit(['/', '.']).next().unwrap_or(name).to_lowercase();
        Some(match leaf.as_str() {
            "sent" | "sent items" | "sent messages" | "gesendet" | "gesendete elemente" | "gesendete objekte" => {
                Role::Sent
            }
            "drafts" | "entwürfe" => Role::Drafts,
            "trash" | "deleted items" | "deleted messages" | "papierkorb" | "gelöschte elemente" => Role::Trash,
            "junk" | "spam" | "junk e-mail" | "junk-e-mail" => Role::Junk,
            "archive" | "archiv" => Role::Archive,
            _ => return None,
        })
    }
}

/// One folder of a `LIST` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxEntry {
    pub name: String,
    pub delimiter: Option<char>,
    pub attributes: Vec<String>,
}

impl MailboxEntry {
    pub fn selectable(&self) -> bool {
        !self.attributes.iter().any(|a| a.eq_ignore_ascii_case("\\Noselect") || a.eq_ignore_ascii_case("\\NonExistent"))
    }

    pub fn role(&self) -> Option<Role> {
        Role::detect(&self.name, &self.attributes)
    }
}

/// The state of a folder as `SELECT` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectInfo {
    pub uid_validity: u32,
    pub uid_next: u32,
    pub exists: u32,
    /// `HIGHESTMODSEQ`, with CONDSTORE.
    pub highest_modseq: Option<u64>,
}

/// What the client knows from the last sync, for `SELECT … (QRESYNC (uidvalidity modseq))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QresyncParams {
    pub uid_validity: u32,
    pub modseq: u64,
}

/// The system flags the app shows, and the other keywords as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flags {
    pub seen: bool,
    pub answered: bool,
    pub flagged: bool,
    pub deleted: bool,
    pub draft: bool,
    pub keywords: Vec<String>,
}

/// New flags of a message (`FETCH (FLAGS MODSEQ)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagUpdate {
    pub uid: u32,
    pub flags: Flags,
    pub modseq: Option<u64>,
}

/// The answer to `SELECT`: with QRESYNC also what changed and vanished since the given modseq.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub info: SelectInfo,
    pub vanished: Vec<u32>,
    pub changed: Vec<FlagUpdate>,
}

/// The header part of a message: what the list and the threads need, without the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedHeader {
    pub uid: u32,
    pub flags: Flags,
    pub modseq: Option<u64>,
    /// `INTERNALDATE`, Unix seconds.
    pub internal_date: i64,
    pub size: u32,
    /// `BODY.PEEK[HEADER]`.
    pub header: Vec<u8>,
}

/// A UID range `from:to`; `to: None` is `from:*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UidRange {
    pub from: u32,
    pub to: Option<u32>,
}

impl UidRange {
    pub fn contains(&self, uid: u32) -> bool {
        uid >= self.from && self.to.is_none_or(|to| uid <= to)
    }
}

/// How an `IDLE` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    /// The server reported a change (EXISTS, EXPUNGE, FETCH).
    Changed,
    /// The time ran out (IDLE is renewed before the server's 30 minutes, RFC 2177).
    Timeout,
}

/// The IMAP operations the sync engine uses. One value is one authenticated connection.
pub trait ImapClient: Send {
    fn capabilities(&mut self) -> impl Future<Output = Result<Capabilities>> + Send;
    fn list(&mut self) -> impl Future<Output = Result<Vec<MailboxEntry>>> + Send;
    /// `SELECT`, with `(QRESYNC …)` when `qresync` is given (the caller checks the capability).
    fn select(
        &mut self,
        mailbox: &str,
        qresync: Option<QresyncParams>,
    ) -> impl Future<Output = Result<Selected>> + Send;
    /// `UID SEARCH ALL` of the selected folder.
    fn uids(&mut self) -> impl Future<Output = Result<Vec<u32>>> + Send;
    fn fetch_headers(&mut self, range: UidRange) -> impl Future<Output = Result<Vec<FetchedHeader>>> + Send;
    /// `UID FETCH range (FLAGS)`, with `(CHANGEDSINCE modseq)` when given.
    fn fetch_flags(
        &mut self,
        range: UidRange,
        changed_since: Option<u64>,
    ) -> impl Future<Output = Result<Vec<FlagUpdate>>> + Send;
    /// The whole message (`BODY.PEEK[]`).
    fn fetch_message(&mut self, uid: u32) -> impl Future<Output = Result<Vec<u8>>> + Send;
    fn idle(&mut self, timeout: Duration) -> impl Future<Output = Result<IdleOutcome>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities() {
        let c = Capabilities::parse(["IMAP4rev1", "idle", "QRESYNC", "MOVE", "AUTH=XOAUTH2"]);
        assert!(c.idle && c.qresync && c.condstore && c.moves);
        assert!(!c.uidplus && !c.special_use);
        assert_eq!(Capabilities::parse([]), Capabilities::default());
    }

    #[test]
    fn roles() {
        let e = |name: &str, attrs: &[&str]| MailboxEntry {
            name: name.into(),
            delimiter: Some('/'),
            attributes: attrs.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(e("INBOX", &[]).role(), Some(Role::Inbox));
        assert_eq!(e("Whatever", &["\\HasNoChildren", "\\Sent"]).role(), Some(Role::Sent));
        assert_eq!(e("[Gmail]/All Mail", &["\\All"]).role(), Some(Role::All));
        assert_eq!(e("INBOX.Gesendete Elemente", &[]).role(), Some(Role::Sent));
        assert_eq!(e("Papierkorb", &[]).role(), Some(Role::Trash));
        assert_eq!(e("Projekte", &[]).role(), None);
        assert!(!e("[Gmail]", &["\\Noselect"]).selectable());
        for r in
            [Role::Inbox, Role::Sent, Role::Drafts, Role::Trash, Role::Junk, Role::Archive, Role::All, Role::Flagged]
        {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
    }

    #[test]
    fn uid_ranges() {
        assert!(UidRange { from: 5, to: None }.contains(9000));
        assert!(!UidRange { from: 5, to: Some(7) }.contains(8));
        assert!(UidRange { from: 5, to: Some(7) }.contains(5));
    }
}
