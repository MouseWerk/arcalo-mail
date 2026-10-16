//! The IMAP client as the sync engine sees it: a trait ([`ImapClient`]) with the few operations
//! the engine needs, and plain data types for the answers.
//!
//! The engine ([`crate::sync`]) is written against the trait only. The real client
//! ([`client::ImapSession`]: our commands, imap-proto's parser, a stream from
//! [`crate::network`]) and the in-memory server for tests ([`fake::FakeImap`]) implement it.
//! Every message is addressed by UID; sequence numbers never leave the client. Folder names are
//! UTF-8 here and in the store; the client encodes them for the wire ([`utf7`]).

pub mod client;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
pub mod utf7;

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

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
    /// The app woke the connection for work of its own (a queued change, a sync request).
    Interrupted,
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
    /// The parts of a message (`BODYSTRUCTURE`), leaves only, for large messages whose
    /// attachments are fetched on demand.
    fn fetch_structure(&mut self, uid: u32) -> impl Future<Output = Result<Vec<BodyPart>>> + Send;
    /// One part with its MIME header (`BODY.PEEK[1.2.MIME]` and `BODY.PEEK[1.2]`), which
    /// parses as a message of its own.
    fn fetch_section(&mut self, uid: u32, section: &str) -> impl Future<Output = Result<Vec<u8>>> + Send;
    /// Waits in IDLE on the selected folder until the server reports a change, `timeout` runs
    /// out or `wake` is notified (the app has something to do on this connection). A server
    /// without IDLE only waits.
    fn idle(&mut self, timeout: Duration, wake: &Notify) -> impl Future<Output = Result<IdleOutcome>> + Send;
    /// `UID STORE uids ±FLAGS.SILENT (flags)` in the selected folder.
    fn store(&mut self, uids: &[u32], flags: &[&str], add: bool) -> impl Future<Output = Result<()>> + Send;
    /// `UID COPY`; the new UIDs when the server says them (UIDPLUS `COPYUID`).
    fn copy(&mut self, uids: &[u32], to: &str) -> impl Future<Output = Result<Vec<(u32, u32)>>> + Send;
    /// `UID MOVE`, or COPY, `\Deleted` and EXPUNGE on servers without MOVE.
    fn move_to(&mut self, uids: &[u32], to: &str) -> impl Future<Output = Result<Vec<(u32, u32)>>> + Send;
    /// Removes messages for good: `\Deleted`, then `UID EXPUNGE` (or EXPUNGE without UIDPLUS).
    fn expunge(&mut self, uids: &[u32]) -> impl Future<Output = Result<()>> + Send;
    /// `APPEND`; the new UID when the server says it (UIDPLUS `APPENDUID`).
    fn append(&mut self, mailbox: &str, flags: &[&str], raw: &[u8])
    -> impl Future<Output = Result<Option<u32>>> + Send;
    /// `UID SEARCH` in the selected folder.
    fn search(&mut self, query: &Search) -> impl Future<Output = Result<Vec<u32>>> + Send;
    fn create(&mut self, mailbox: &str) -> impl Future<Output = Result<()>> + Send;
    fn delete(&mut self, mailbox: &str) -> impl Future<Output = Result<()>> + Send;
    fn rename(&mut self, from: &str, to: &str) -> impl Future<Output = Result<()>> + Send;
    fn logout(&mut self) -> impl Future<Output = Result<()>> + Send;
}

/// What `UID SEARCH` looks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Search {
    /// Words anywhere in the message (`TEXT`), the server's own matching.
    Text(String),
    /// A message by its `Message-ID` header (drafts replaced on save).
    MessageId(String),
}

/// One leaf of a message's MIME tree, with the IMAP section that fetches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyPart {
    /// `1`, `2.1`, … (RFC 3501 part specifier).
    pub section: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size: u64,
    pub content_id: Option<String>,
    /// `Content-Disposition: inline` (or none) as opposed to `attachment`.
    pub inline: bool,
}

impl BodyPart {
    /// A part shown as an attachment (not the text, not an image the HTML shows).
    pub fn is_attachment(&self) -> bool {
        let text = self.mime_type == "text/plain" || self.mime_type == "text/html";
        !(text && self.filename.is_none() && self.inline) && !(self.inline && self.content_id.is_some())
    }
}

/// `1:3,5,7:9` for a list of UIDs.
pub fn uid_set(uids: &[u32]) -> String {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        out.push(if start == end { start.to_string() } else { format!("{start}:{end}") });
        i += 1;
    }
    out.join(",")
}

/// The IMAP name of a system flag the app sets.
pub const SEEN: &str = "\\Seen";
pub const FLAGGED: &str = "\\Flagged";
pub const ANSWERED: &str = "\\Answered";
pub const DELETED: &str = "\\Deleted";
pub const DRAFT: &str = "\\Draft";
/// The keyword spam filters and other clients read (RFC 5788 registry).
pub const JUNK: &str = "$Junk";

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
