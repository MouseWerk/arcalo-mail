//! An IMAP server in memory for tests: folders with UIDs, flags, MODSEQ, UIDVALIDITY and
//! vanished UIDs, with the capabilities a test switches on or off. It answers like a server
//! that follows RFC 3501, RFC 4315 (UIDPLUS), RFC 6851 (MOVE) and RFC 7162; `log` records the
//! commands for assertions, `fail` makes the next command of a kind fail.
//!
//! [`FakeImap`] is one connection to its own server (the sync tests); [`SharedFake`] gives
//! several connections to one server, as the account worker opens them, and wakes IDLE when
//! another connection or the test changes something. The wire server of the e2e tests
//! (`crates/mail-testserver`) serves the same state over TCP.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use super::*;
use crate::error::Error;

#[derive(Debug, Clone)]
pub struct FakeMessage {
    pub raw: Vec<u8>,
    pub flags: Flags,
    pub modseq: u64,
    pub internal_date: i64,
    /// Gmail's `X-GM-MSGID`: the same message in every label folder and in All Mail.
    pub gm_msgid: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct FakeMailbox {
    pub attributes: Vec<String>,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub highest_modseq: u64,
    pub messages: BTreeMap<u32, FakeMessage>,
    /// Expunged UIDs with the modseq of the expunge (for `VANISHED (EARLIER)`).
    pub vanished: Vec<(u64, u32)>,
}

impl FakeMailbox {
    pub fn new(attributes: &[&str]) -> Self {
        FakeMailbox {
            attributes: attributes.iter().map(|s| s.to_string()).collect(),
            uid_validity: 1,
            uid_next: 1,
            highest_modseq: 1,
            messages: BTreeMap::new(),
            vanished: vec![],
        }
    }

    pub fn info(&self) -> SelectInfo {
        SelectInfo {
            uid_validity: self.uid_validity,
            uid_next: self.uid_next,
            exists: self.messages.len() as u32,
            highest_modseq: Some(self.highest_modseq),
        }
    }

    fn add(&mut self, msg: FakeMessage) -> u32 {
        self.highest_modseq += 1;
        let uid = self.uid_next;
        self.uid_next += 1;
        self.messages.insert(uid, FakeMessage { modseq: self.highest_modseq, ..msg });
        uid
    }

    fn remove(&mut self, uid: u32) -> Option<FakeMessage> {
        self.highest_modseq += 1;
        let m = self.messages.remove(&uid);
        if m.is_some() {
            self.vanished.push((self.highest_modseq, uid));
        }
        m
    }
}

#[derive(Debug, Clone)]
pub struct FakeImap {
    pub caps: Capabilities,
    pub mailboxes: BTreeMap<String, FakeMailbox>,
    pub selected: Option<String>,
    pub log: Vec<String>,
    /// Set by every change; the next `idle` reports it and clears it.
    pub changed: bool,
    /// Commands that fail once with `NO`, by their first word (`MOVE`, `APPEND`, …).
    pub fail: Vec<String>,
    /// Folders the server lists but refuses to open (`NO [NOPERM]`: a shared folder without
    /// the read right).
    pub refuse_select: Vec<String>,
    /// How long each command of a [`SharedFake`] connection takes (a slow server, a phone's
    /// network): the connection waits that long before it answers.
    pub latency: Duration,
    /// Commands of a [`SharedFake`] connection that never get an answer, once each, by the
    /// method's name (`fetch_message`, …).
    pub stall: Vec<String>,
    /// The last `X-GM-MSGID` given out ([`FakeImap::deliver_gmail`]).
    pub gm_last: u64,
}

impl Default for FakeImap {
    fn default() -> Self {
        Self::new(Capabilities {
            idle: true,
            condstore: true,
            qresync: true,
            uidplus: true,
            moves: true,
            special_use: true,
            gmail: false,
        })
    }
}

fn no(what: &str) -> Error {
    Error::Imap(format!("NO {what}"))
}

/// Where the body of a raw message starts.
fn header_end(raw: &[u8]) -> usize {
    raw.windows(4).position(|w| w == b"\r\n\r\n").map_or(raw.len(), |p| p + 4)
}

/// Unix seconds now (internal dates of appended messages).
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl FakeImap {
    /// A server with INBOX, Sent and Trash.
    pub fn new(caps: Capabilities) -> Self {
        let mut mailboxes = BTreeMap::new();
        mailboxes.insert("INBOX".to_string(), FakeMailbox::new(&[]));
        mailboxes.insert("Sent".to_string(), FakeMailbox::new(&["\\Sent"]));
        mailboxes.insert("Trash".to_string(), FakeMailbox::new(&["\\Trash"]));
        FakeImap {
            caps,
            mailboxes,
            selected: None,
            log: vec![],
            changed: false,
            fail: vec![],
            refuse_select: vec![],
            latency: Duration::ZERO,
            stall: vec![],
            gm_last: 0,
        }
    }

    /// A server as Gmail's IMAP with the account in German: `X-GM-EXT-1`, CONDSTORE but no
    /// QRESYNC, the special folders below „[Gmail]“ with their German names and SPECIAL-USE
    /// attributes, and labels as folders (one with brackets in its name, and a label „Entwürfe“
    /// another client made next to the real drafts).
    pub fn gmail() -> Self {
        let caps = Capabilities {
            idle: true,
            condstore: true,
            qresync: false,
            uidplus: true,
            moves: true,
            special_use: true,
            gmail: true,
        };
        let mut s = Self::new(caps);
        s.mailboxes.clear();
        for (name, attrs) in [
            ("INBOX", &[][..]),
            ("[Gmail]", &["\\HasChildren", "\\Noselect"][..]),
            ("[Gmail]/Alle Nachrichten", &["\\All"][..]),
            ("[Gmail]/Entwürfe", &["\\Drafts"][..]),
            ("[Gmail]/Gesendet", &["\\Sent"][..]),
            ("[Gmail]/Markiert", &["\\Flagged"][..]),
            ("[Gmail]/Papierkorb", &["\\Trash"][..]),
            ("[Gmail]/Spam", &["\\Junk"][..]),
            ("[Gmail]/Wichtig", &["\\Important"][..]),
            ("[Notion]", &[][..]),
            ("Account Confirmations", &[][..]),
            ("Entwürfe", &[][..]),
        ] {
            s.mailboxes.insert(name.to_string(), FakeMailbox::new(attrs));
        }
        s
    }

    /// Gmail's All Mail (the folder with `\All`).
    pub fn all_mail(&self) -> Option<String> {
        self.mailboxes.iter().find(|(_, m)| m.attributes.iter().any(|a| a == "\\All")).map(|(n, _)| n.clone())
    }

    /// Delivers a message as Gmail keeps it: once in All Mail and once in each of `labels`
    /// (`INBOX` among them for the inbox), all with the same `X-GM-MSGID`; returns that id.
    pub fn deliver_gmail(&mut self, raw: &[u8], internal_date: i64, labels: &[&str]) -> u64 {
        self.changed = true;
        self.gm_last += 1;
        let id = 0x1000_0000_0000 + self.gm_last;
        let msg =
            FakeMessage { raw: raw.to_vec(), flags: Flags::default(), modseq: 0, internal_date, gm_msgid: Some(id) };
        let all = self.all_mail();
        for name in labels.iter().map(|l| l.to_string()).chain(all) {
            self.mailbox(&name).add(msg.clone());
        }
        id
    }

    /// Adds the other usual folders (Drafts, Archive, Junk).
    pub fn with_all_folders(mut self) -> Self {
        for (name, attr) in [("Drafts", "\\Drafts"), ("Archive", "\\Archive"), ("Junk", "\\Junk")] {
            self.mailboxes.insert(name.to_string(), FakeMailbox::new(&[attr]));
        }
        self
    }

    fn mailbox(&mut self, name: &str) -> &mut FakeMailbox {
        self.mailboxes.get_mut(name).unwrap_or_else(|| panic!("no folder {name}"))
    }

    fn existing(&mut self, name: &str) -> Result<&mut FakeMailbox> {
        self.mailboxes.get_mut(name).ok_or_else(|| no(&format!("[TRYCREATE] no folder {name}")))
    }

    fn selected_mailbox(&mut self) -> Result<&mut FakeMailbox> {
        let name = self.selected.clone().ok_or_else(|| Error::Imap("BAD no folder selected".into()))?;
        self.existing(&name)
    }

    /// Fails when a test asked the next command of this kind to fail.
    fn check(&mut self, kind: &str) -> Result<()> {
        if let Some(i) = self.fail.iter().position(|f| f.eq_ignore_ascii_case(kind)) {
            self.fail.remove(i);
            return Err(no(&format!("{kind} refused (test)")));
        }
        Ok(())
    }

    /// Delivers a message; returns its UID.
    pub fn deliver(&mut self, mailbox: &str, raw: &[u8], internal_date: i64) -> u32 {
        self.changed = true;
        let msg = FakeMessage { raw: raw.to_vec(), flags: Flags::default(), modseq: 0, internal_date, gm_msgid: None };
        self.mailbox(mailbox).add(msg)
    }

    /// Changes the flags of a message, as another client would.
    pub fn set_flags(&mut self, mailbox: &str, uid: u32, flags: Flags) {
        self.changed = true;
        let m = self.mailbox(mailbox);
        m.highest_modseq += 1;
        let modseq = m.highest_modseq;
        let msg = m.messages.get_mut(&uid).expect("no such message");
        msg.flags = flags;
        msg.modseq = modseq;
    }

    /// Removes a message, as another client would.
    pub fn expunge(&mut self, mailbox: &str, uid: u32) {
        self.changed = true;
        self.mailbox(mailbox).remove(uid);
    }

    /// The server renumbers the folder (a restore, a migration): new UIDVALIDITY, new UIDs.
    pub fn renumber(&mut self, mailbox: &str) {
        self.changed = true;
        let m = self.mailbox(mailbox);
        let old = std::mem::take(&mut m.messages);
        m.uid_validity += 1;
        m.vanished.clear();
        m.uid_next = 1;
        for (_, mut msg) in old {
            m.highest_modseq += 1;
            msg.modseq = m.highest_modseq;
            m.messages.insert(m.uid_next, msg);
            m.uid_next += 1;
        }
    }

    /// The subjects of a folder in UID order (for assertions).
    pub fn subjects(&self, mailbox: &str) -> Vec<String> {
        self.mailboxes
            .get(mailbox)
            .map(|m| {
                m.messages
                    .values()
                    .map(|msg| crate::parse::parse(&msg.raw).map(|p| p.subject).unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------ the commands

    pub fn do_list(&mut self) -> Vec<MailboxEntry> {
        self.log.push("LIST".into());
        self.mailboxes
            .iter()
            .map(|(name, m)| MailboxEntry {
                name: name.clone(),
                delimiter: Some('/'),
                attributes: m.attributes.clone(),
            })
            .collect()
    }

    pub fn do_select(&mut self, mailbox: &str, qresync: Option<QresyncParams>) -> Result<Selected> {
        let caps = self.caps;
        if self.refuse_select.iter().any(|r| r == mailbox) {
            self.log.push(format!("SELECT {mailbox} refused"));
            return Err(no(&format!("[NOPERM] no read right on {mailbox}")));
        }
        let m = self.mailboxes.get(mailbox).ok_or_else(|| no(&format!("no such folder {mailbox}")))?;
        let info = m.info();
        let mut out = Selected { info, vanished: vec![], changed: vec![] };
        match qresync {
            Some(q) => {
                if !caps.qresync {
                    return Err(Error::Imap("BAD QRESYNC not enabled".into()));
                }
                self.log.push(format!("SELECT {mailbox} (QRESYNC ({} {}))", q.uid_validity, q.modseq));
                // A different UIDVALIDITY: the server sends no changes (RFC 7162, 3.2.5.2).
                if q.uid_validity == m.uid_validity {
                    out.vanished = m.vanished.iter().filter(|(ms, _)| *ms > q.modseq).map(|(_, uid)| *uid).collect();
                    out.changed = m
                        .messages
                        .iter()
                        .filter(|(_, msg)| msg.modseq > q.modseq)
                        .map(|(uid, msg)| FlagUpdate { uid: *uid, flags: msg.flags.clone(), modseq: Some(msg.modseq) })
                        .collect();
                }
            }
            None => self.log.push(format!("SELECT {mailbox}")),
        }
        if !caps.condstore {
            out.info.highest_modseq = None;
        }
        self.selected = Some(mailbox.to_string());
        Ok(out)
    }

    pub fn do_uids(&mut self) -> Result<Vec<u32>> {
        self.log.push("UID SEARCH ALL".into());
        Ok(self.selected_mailbox()?.messages.keys().copied().collect())
    }

    pub fn do_fetch_headers(&mut self, range: UidRange) -> Result<Vec<FetchedHeader>> {
        self.log.push(format!("UID FETCH {}:{} (HEADER)", range.from, range.to.map_or("*".into(), |t| t.to_string())));
        let condstore = self.caps.condstore;
        let gmail = self.caps.gmail;
        let m = self.selected_mailbox()?;
        Ok(m.messages
            .iter()
            .filter(|(uid, _)| range.contains(**uid))
            .map(|(uid, msg)| FetchedHeader {
                uid: *uid,
                flags: msg.flags.clone(),
                modseq: condstore.then_some(msg.modseq),
                internal_date: msg.internal_date,
                size: msg.raw.len() as u32,
                header: msg.raw[..header_end(&msg.raw)].to_vec(),
                gm_msgid: if gmail { msg.gm_msgid } else { None },
            })
            .collect())
    }

    /// The first [`PREVIEW_BYTES`] of each body (`BODY.PEEK[TEXT]<0.2048>`).
    pub fn do_fetch_previews(&mut self, range: UidRange) -> Result<Vec<(u32, Vec<u8>)>> {
        self.log.push(format!(
            "UID FETCH {}:{} (BODY.PEEK[TEXT]<0.{PREVIEW_BYTES}>)",
            range.from,
            range.to.map_or("*".into(), |t| t.to_string())
        ));
        let m = self.selected_mailbox()?;
        Ok(m.messages
            .iter()
            .filter(|(uid, _)| range.contains(**uid))
            .map(|(uid, msg)| {
                let body = &msg.raw[header_end(&msg.raw)..];
                (*uid, body[..body.len().min(PREVIEW_BYTES)].to_vec())
            })
            .collect())
    }

    pub fn do_fetch_flags(&mut self, range: UidRange, changed_since: Option<u64>) -> Result<Vec<FlagUpdate>> {
        if changed_since.is_some() && !self.caps.condstore {
            return Err(Error::Imap("BAD CHANGEDSINCE without CONDSTORE".into()));
        }
        self.log.push(format!("UID FETCH {}:* (FLAGS) CHANGEDSINCE {changed_since:?}", range.from));
        let condstore = self.caps.condstore;
        let m = self.selected_mailbox()?;
        Ok(m.messages
            .iter()
            .filter(|(uid, msg)| range.contains(**uid) && changed_since.is_none_or(|c| msg.modseq > c))
            .map(|(uid, msg)| FlagUpdate {
                uid: *uid,
                flags: msg.flags.clone(),
                modseq: condstore.then_some(msg.modseq),
            })
            .collect())
    }

    fn raw_of(&mut self, uid: u32) -> Result<Vec<u8>> {
        let m = self.selected_mailbox()?;
        m.messages.get(&uid).map(|msg| msg.raw.clone()).ok_or_else(|| no(&format!("no message {uid}")))
    }

    pub fn do_fetch_message(&mut self, uid: u32) -> Result<Vec<u8>> {
        self.log.push(format!("UID FETCH {uid} (BODY.PEEK[])"));
        self.raw_of(uid)
    }

    pub fn do_fetch_structure(&mut self, uid: u32) -> Result<Vec<BodyPart>> {
        self.log.push(format!("UID FETCH {uid} (BODYSTRUCTURE)"));
        Ok(crate::parse::structure(&self.raw_of(uid)?))
    }

    pub fn do_fetch_section(&mut self, uid: u32, section: &str) -> Result<Vec<u8>> {
        self.log.push(format!("UID FETCH {uid} (BODY.PEEK[{section}])"));
        crate::parse::section(&self.raw_of(uid)?, section).ok_or_else(|| no("no such part"))
    }

    pub fn do_store(&mut self, uids: &[u32], flags: &[&str], add: bool) -> Result<()> {
        self.check("STORE")?;
        self.log.push(format!(
            "UID STORE {} {}FLAGS ({})",
            uid_set(uids),
            if add { "+" } else { "-" },
            flags.join(" ")
        ));
        self.changed = true;
        let m = self.selected_mailbox()?;
        for uid in uids {
            m.highest_modseq += 1;
            let modseq = m.highest_modseq;
            let Some(msg) = m.messages.get_mut(uid) else { continue };
            for f in flags {
                let slot = match f.to_ascii_lowercase().as_str() {
                    "\\seen" => &mut msg.flags.seen,
                    "\\flagged" => &mut msg.flags.flagged,
                    "\\answered" => &mut msg.flags.answered,
                    "\\deleted" => &mut msg.flags.deleted,
                    "\\draft" => &mut msg.flags.draft,
                    _ => {
                        msg.flags.keywords.retain(|k| !k.eq_ignore_ascii_case(f));
                        if add {
                            msg.flags.keywords.push(f.to_string());
                        }
                        continue;
                    }
                };
                *slot = add;
            }
            msg.modseq = modseq;
        }
        Ok(())
    }

    pub fn do_copy(&mut self, uids: &[u32], to: &str, remove: bool) -> Result<Vec<(u32, u32)>> {
        self.check(if remove { "MOVE" } else { "COPY" })?;
        self.log.push(format!("UID {} {} {to}", if remove { "MOVE" } else { "COPY" }, uid_set(uids)));
        self.existing(to)?;
        self.changed = true;
        let from = self.selected.clone().ok_or_else(|| Error::Imap("BAD no folder selected".into()))?;
        let mut moved = vec![];
        for uid in uids {
            let src = self.existing(&from)?;
            let msg = if remove { src.remove(*uid) } else { src.messages.get(uid).cloned() };
            if let Some(msg) = msg {
                moved.push((*uid, msg));
            }
        }
        let target = self.existing(to)?;
        let map: Vec<(u32, u32)> = moved.into_iter().map(|(uid, msg)| (uid, target.add(msg))).collect();
        Ok(if self.caps.uidplus { map } else { vec![] })
    }

    pub fn do_expunge(&mut self, uids: &[u32]) -> Result<()> {
        self.check("EXPUNGE")?;
        self.log.push(format!("UID EXPUNGE {}", uid_set(uids)));
        self.changed = true;
        let m = self.selected_mailbox()?;
        for uid in uids {
            m.remove(*uid);
        }
        Ok(())
    }

    pub fn do_append(&mut self, mailbox: &str, flags: &[&str], raw: &[u8]) -> Result<Option<u32>> {
        self.check("APPEND")?;
        self.log.push(format!("APPEND {mailbox} ({})", flags.join(" ")));
        self.changed = true;
        let uidplus = self.caps.uidplus;
        let m = self.existing(mailbox)?;
        let f = Flags {
            seen: flags.iter().any(|f| f.eq_ignore_ascii_case(SEEN)),
            draft: flags.iter().any(|f| f.eq_ignore_ascii_case(DRAFT)),
            flagged: flags.iter().any(|f| f.eq_ignore_ascii_case(FLAGGED)),
            ..Flags::default()
        };
        let uid = m.add(FakeMessage { raw: raw.to_vec(), flags: f, modseq: 0, internal_date: now(), gm_msgid: None });
        Ok(uidplus.then_some(uid))
    }

    pub fn do_search(&mut self, query: &Search) -> Result<Vec<u32>> {
        self.log.push(format!("UID SEARCH {query:?}"));
        let m = self.selected_mailbox()?;
        Ok(m.messages
            .iter()
            .filter(|(_, msg)| match query {
                Search::Text(t) => {
                    let p = crate::parse::parse(&msg.raw).unwrap_or_default();
                    let hay =
                        format!("{} {} {}", p.subject, p.text.unwrap_or_default(), String::from_utf8_lossy(&msg.raw))
                            .to_lowercase();
                    hay.contains(&t.to_lowercase())
                }
                Search::MessageId(id) => crate::parse::parse(&msg.raw)
                    .ok()
                    .and_then(|p| p.message_id)
                    .is_some_and(|m| m == id.trim_matches(['<', '>'])),
            })
            .map(|(uid, _)| *uid)
            .collect())
    }

    pub fn do_create(&mut self, mailbox: &str) -> Result<()> {
        self.check("CREATE")?;
        self.log.push(format!("CREATE {mailbox}"));
        if self.mailboxes.contains_key(mailbox) {
            return Err(no("[ALREADYEXISTS] folder exists"));
        }
        self.mailboxes.insert(mailbox.to_string(), FakeMailbox::new(&[]));
        Ok(())
    }

    pub fn do_delete(&mut self, mailbox: &str) -> Result<()> {
        self.check("DELETE")?;
        self.log.push(format!("DELETE {mailbox}"));
        self.mailboxes.remove(mailbox).map(drop).ok_or_else(|| no("[NONEXISTENT] no such folder"))
    }

    pub fn do_rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.check("RENAME")?;
        self.log.push(format!("RENAME {from} {to}"));
        if self.mailboxes.contains_key(to) {
            return Err(no("[ALREADYEXISTS] folder exists"));
        }
        let m = self.mailboxes.remove(from).ok_or_else(|| no("[NONEXISTENT] no such folder"))?;
        self.mailboxes.insert(to.to_string(), FakeMailbox { uid_validity: m.uid_validity + 1, ..m });
        Ok(())
    }
}

impl ImapClient for FakeImap {
    async fn capabilities(&mut self) -> Result<Capabilities> {
        self.log.push("CAPABILITY".into());
        Ok(self.caps)
    }

    async fn list(&mut self) -> Result<Vec<MailboxEntry>> {
        Ok(self.do_list())
    }

    async fn select(&mut self, mailbox: &str, qresync: Option<QresyncParams>) -> Result<Selected> {
        self.do_select(mailbox, qresync)
    }

    async fn uids(&mut self) -> Result<Vec<u32>> {
        self.do_uids()
    }

    async fn fetch_headers(&mut self, range: UidRange) -> Result<Vec<FetchedHeader>> {
        self.do_fetch_headers(range)
    }

    async fn fetch_previews(&mut self, range: UidRange) -> Result<Vec<(u32, Vec<u8>)>> {
        self.do_fetch_previews(range)
    }

    async fn fetch_flags(&mut self, range: UidRange, changed_since: Option<u64>) -> Result<Vec<FlagUpdate>> {
        self.do_fetch_flags(range, changed_since)
    }

    async fn fetch_message(&mut self, uid: u32) -> Result<Vec<u8>> {
        self.do_fetch_message(uid)
    }

    async fn fetch_structure(&mut self, uid: u32) -> Result<Vec<BodyPart>> {
        self.do_fetch_structure(uid)
    }

    async fn fetch_section(&mut self, uid: u32, section: &str) -> Result<Vec<u8>> {
        self.do_fetch_section(uid, section)
    }

    async fn idle(&mut self, _timeout: Duration, _wake: &Notify) -> Result<IdleOutcome> {
        if !self.caps.idle {
            return Err(Error::Imap("BAD IDLE not supported".into()));
        }
        self.log.push("IDLE".into());
        Ok(if std::mem::take(&mut self.changed) { IdleOutcome::Changed } else { IdleOutcome::Timeout })
    }

    async fn store(&mut self, uids: &[u32], flags: &[&str], add: bool) -> Result<()> {
        self.do_store(uids, flags, add)
    }

    async fn copy(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        self.do_copy(uids, to, false)
    }

    async fn move_to(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        self.do_copy(uids, to, true)
    }

    async fn expunge(&mut self, uids: &[u32]) -> Result<()> {
        self.do_expunge(uids)
    }

    async fn append(&mut self, mailbox: &str, flags: &[&str], raw: &[u8]) -> Result<Option<u32>> {
        self.do_append(mailbox, flags, raw)
    }

    async fn search(&mut self, query: &Search) -> Result<Vec<u32>> {
        self.do_search(query)
    }

    async fn create(&mut self, mailbox: &str) -> Result<()> {
        self.do_create(mailbox)
    }

    async fn delete(&mut self, mailbox: &str) -> Result<()> {
        self.do_delete(mailbox)
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.do_rename(from, to)
    }

    async fn logout(&mut self) -> Result<()> {
        self.log.push("LOGOUT".into());
        Ok(())
    }
}

// ---------------------------------------------------------------------------- shared

/// One fake server for several connections (the worker's sync, IDLE and on-demand ones). Each
/// clone from [`SharedFake::connection`] has its own selected folder.
#[derive(Debug, Clone)]
pub struct SharedFake {
    server: Arc<Mutex<FakeImap>>,
    notify: Arc<Notify>,
    selected: Option<String>,
}

impl SharedFake {
    pub fn new(server: FakeImap) -> Self {
        SharedFake { server: Arc::new(Mutex::new(server)), notify: Arc::new(Notify::new()), selected: None }
    }

    /// Another connection to the same server.
    pub fn connection(&self) -> SharedFake {
        SharedFake { selected: None, ..self.clone() }
    }

    /// Changes the server as a test or another client would; a connection in IDLE notices.
    pub fn with<R>(&self, f: impl FnOnce(&mut FakeImap) -> R) -> R {
        let r = f(&mut self.server.lock().unwrap_or_else(|e| e.into_inner()));
        self.notify.notify_waiters();
        r
    }

    /// The server's [`FakeImap::latency`]: a real wait, so other tasks run meanwhile. A
    /// command in [`FakeImap::stall`] never answers (once): a connection that died unnoticed.
    async fn wait(&self, command: &str) {
        let (latency, stall) = {
            let mut s = self.server.lock().unwrap_or_else(|e| e.into_inner());
            let stall = s.stall.iter().position(|c| c == command).map(|i| s.stall.remove(i)).is_some();
            (s.latency, stall)
        };
        if stall {
            std::future::pending::<()>().await;
        }
        if !latency.is_zero() {
            tokio::time::sleep(latency).await;
        }
    }

    /// Runs a command of this connection (its selected folder in place).
    fn run<R>(&mut self, f: impl FnOnce(&mut FakeImap) -> R) -> R {
        let mut s = self.server.lock().unwrap_or_else(|e| e.into_inner());
        s.selected = self.selected.take();
        let r = f(&mut s);
        self.selected = s.selected.take();
        let changed = s.changed;
        drop(s);
        if changed {
            self.notify.notify_waiters();
        }
        r
    }
}

impl ImapClient for SharedFake {
    async fn capabilities(&mut self) -> Result<Capabilities> {
        self.wait("capabilities").await;
        Ok(self.run(|s| s.caps))
    }

    async fn list(&mut self) -> Result<Vec<MailboxEntry>> {
        self.wait("list").await;
        Ok(self.run(|s| s.do_list()))
    }

    async fn select(&mut self, mailbox: &str, qresync: Option<QresyncParams>) -> Result<Selected> {
        self.wait("select").await;
        self.run(|s| s.do_select(mailbox, qresync))
    }

    async fn uids(&mut self) -> Result<Vec<u32>> {
        self.wait("uids").await;
        self.run(|s| s.do_uids())
    }

    async fn fetch_headers(&mut self, range: UidRange) -> Result<Vec<FetchedHeader>> {
        self.wait("fetch_headers").await;
        self.run(|s| s.do_fetch_headers(range))
    }

    async fn fetch_previews(&mut self, range: UidRange) -> Result<Vec<(u32, Vec<u8>)>> {
        self.wait("fetch_previews").await;
        self.run(|s| s.do_fetch_previews(range))
    }

    async fn fetch_flags(&mut self, range: UidRange, changed_since: Option<u64>) -> Result<Vec<FlagUpdate>> {
        self.wait("fetch_flags").await;
        self.run(|s| s.do_fetch_flags(range, changed_since))
    }

    async fn fetch_message(&mut self, uid: u32) -> Result<Vec<u8>> {
        self.wait("fetch_message").await;
        self.run(|s| s.do_fetch_message(uid))
    }

    async fn fetch_structure(&mut self, uid: u32) -> Result<Vec<BodyPart>> {
        self.wait("fetch_structure").await;
        self.run(|s| s.do_fetch_structure(uid))
    }

    async fn fetch_section(&mut self, uid: u32, section: &str) -> Result<Vec<u8>> {
        self.wait("fetch_section").await;
        self.run(|s| s.do_fetch_section(uid, section))
    }

    async fn idle(&mut self, timeout: Duration, wake: &Notify) -> Result<IdleOutcome> {
        let notified = self.notify.clone();
        let changed = notified.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if self.run(|s| std::mem::take(&mut s.changed)) {
            return Ok(IdleOutcome::Changed);
        }
        Ok(tokio::select! {
            _ = changed => {
                self.run(|s| s.changed = false);
                IdleOutcome::Changed
            }
            _ = wake.notified() => IdleOutcome::Interrupted,
            _ = tokio::time::sleep(timeout) => IdleOutcome::Timeout,
        })
    }

    async fn store(&mut self, uids: &[u32], flags: &[&str], add: bool) -> Result<()> {
        self.wait("store").await;
        self.run(|s| s.do_store(uids, flags, add))
    }

    async fn copy(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        self.wait("copy").await;
        self.run(|s| s.do_copy(uids, to, false))
    }

    async fn move_to(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        self.wait("move_to").await;
        self.run(|s| s.do_copy(uids, to, true))
    }

    async fn expunge(&mut self, uids: &[u32]) -> Result<()> {
        self.wait("expunge").await;
        self.run(|s| s.do_expunge(uids))
    }

    async fn append(&mut self, mailbox: &str, flags: &[&str], raw: &[u8]) -> Result<Option<u32>> {
        self.wait("append").await;
        self.run(|s| s.do_append(mailbox, flags, raw))
    }

    async fn search(&mut self, query: &Search) -> Result<Vec<u32>> {
        self.wait("search").await;
        self.run(|s| s.do_search(query))
    }

    async fn create(&mut self, mailbox: &str) -> Result<()> {
        self.wait("create").await;
        self.run(|s| s.do_create(mailbox))
    }

    async fn delete(&mut self, mailbox: &str) -> Result<()> {
        self.wait("delete").await;
        self.run(|s| s.do_delete(mailbox))
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.wait("rename").await;
        self.run(|s| s.do_rename(from, to))
    }

    async fn logout(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::tests::sample;

    #[tokio::test]
    async fn moves_copies_appends_and_folders() {
        let mut s = FakeImap::default().with_all_folders();
        let a = s.deliver("INBOX", &sample("a@x", "Eins", None, "x"), 1);
        let b = s.deliver("INBOX", &sample("b@x", "Zwei", None, "x"), 2);
        s.select("INBOX", None).await.unwrap();
        let map = s.move_to(&[a], "Archive").await.unwrap();
        assert_eq!(map, vec![(a, 1)], "COPYUID with UIDPLUS");
        assert_eq!(s.subjects("Archive"), ["Eins"]);
        assert_eq!(s.subjects("INBOX"), ["Zwei"]);
        s.store(&[b], &[SEEN, FLAGGED], true).await.unwrap();
        assert!(s.mailboxes["INBOX"].messages[&b].flags.seen && s.mailboxes["INBOX"].messages[&b].flags.flagged);
        s.store(&[b], &[SEEN], false).await.unwrap();
        assert!(!s.mailboxes["INBOX"].messages[&b].flags.seen);
        assert_eq!(s.append("Drafts", &[DRAFT, SEEN], &sample("d@x", "Entwurf", None, "x")).await.unwrap(), Some(1));
        s.select("Drafts", None).await.unwrap();
        assert_eq!(s.search(&Search::MessageId("<d@x>".into())).await.unwrap(), [1]);
        assert_eq!(s.search(&Search::Text("entwurf".into())).await.unwrap(), [1]);
        ImapClient::expunge(&mut s, &[1]).await.unwrap();
        assert!(s.subjects("Drafts").is_empty());
        s.create("Projekte").await.unwrap();
        assert!(s.create("Projekte").await.is_err());
        s.rename("Projekte", "Kunden").await.unwrap();
        s.delete("Kunden").await.unwrap();
        assert!(!s.mailboxes.contains_key("Kunden"));
        s.fail.push("MOVE".into());
        s.select("INBOX", None).await.unwrap();
        assert!(s.move_to(&[b], "Archive").await.is_err(), "a failure the test asked for");
        assert!(s.move_to(&[b], "Archive").await.is_ok(), "only once");
    }

    #[tokio::test]
    async fn shared_connections_wake_idle() {
        let shared = SharedFake::new(FakeImap::default());
        let mut idle = shared.connection();
        idle.select("INBOX", None).await.unwrap();
        let wake = Notify::new();
        assert_eq!(idle.idle(Duration::from_millis(10), &wake).await.unwrap(), IdleOutcome::Timeout);
        let s2 = shared.clone();
        let waiting = tokio::spawn(async move {
            let wake = Notify::new();
            idle.idle(Duration::from_secs(5), &wake).await.unwrap()
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        s2.with(|s| s.deliver("INBOX", &sample("n@x", "Neu", None, "x"), 3));
        assert_eq!(waiting.await.unwrap(), IdleOutcome::Changed);
        let mut other = shared.connection();
        other.select("INBOX", None).await.unwrap();
        assert_eq!(other.uids().await.unwrap(), [1]);
    }
}
