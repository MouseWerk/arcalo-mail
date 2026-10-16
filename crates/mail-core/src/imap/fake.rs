//! An IMAP server in memory for tests: folders with UIDs, flags, MODSEQ, UIDVALIDITY and
//! vanished UIDs, with the capabilities a test switches on or off. It answers like a server
//! that follows RFC 3501 and RFC 7162; `log` records the commands for assertions.

use std::collections::BTreeMap;
use std::time::Duration;

use super::*;
use crate::error::Error;

#[derive(Debug, Clone)]
pub struct FakeMessage {
    pub raw: Vec<u8>,
    pub flags: Flags,
    pub modseq: u64,
    pub internal_date: i64,
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
    fn new(attributes: &[&str]) -> Self {
        FakeMailbox {
            attributes: attributes.iter().map(|s| s.to_string()).collect(),
            uid_validity: 1,
            uid_next: 1,
            highest_modseq: 1,
            messages: BTreeMap::new(),
            vanished: vec![],
        }
    }

    fn info(&self) -> SelectInfo {
        SelectInfo {
            uid_validity: self.uid_validity,
            uid_next: self.uid_next,
            exists: self.messages.len() as u32,
            highest_modseq: Some(self.highest_modseq),
        }
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
        })
    }
}

impl FakeImap {
    /// A server with INBOX, Sent and Trash.
    pub fn new(caps: Capabilities) -> Self {
        let mut mailboxes = BTreeMap::new();
        mailboxes.insert("INBOX".to_string(), FakeMailbox::new(&[]));
        mailboxes.insert("Sent".to_string(), FakeMailbox::new(&["\\Sent"]));
        mailboxes.insert("Trash".to_string(), FakeMailbox::new(&["\\Trash"]));
        FakeImap { caps, mailboxes, selected: None, log: vec![], changed: false }
    }

    fn mailbox(&mut self, name: &str) -> &mut FakeMailbox {
        self.mailboxes.get_mut(name).unwrap_or_else(|| panic!("no folder {name}"))
    }

    fn selected_mailbox(&mut self) -> Result<&mut FakeMailbox> {
        let name = self.selected.clone().ok_or_else(|| Error::Imap("no folder selected".into()))?;
        Ok(self.mailbox(&name))
    }

    /// Delivers a message; returns its UID.
    pub fn deliver(&mut self, mailbox: &str, raw: &[u8], internal_date: i64) -> u32 {
        self.changed = true;
        let m = self.mailbox(mailbox);
        m.highest_modseq += 1;
        let uid = m.uid_next;
        m.uid_next += 1;
        let msg = FakeMessage { raw: raw.to_vec(), flags: Flags::default(), modseq: m.highest_modseq, internal_date };
        m.messages.insert(uid, msg);
        uid
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
        let m = self.mailbox(mailbox);
        m.highest_modseq += 1;
        if m.messages.remove(&uid).is_some() {
            let modseq = m.highest_modseq;
            m.vanished.push((modseq, uid));
        }
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
}

impl ImapClient for FakeImap {
    async fn capabilities(&mut self) -> Result<Capabilities> {
        self.log.push("CAPABILITY".into());
        Ok(self.caps)
    }

    async fn list(&mut self) -> Result<Vec<MailboxEntry>> {
        self.log.push("LIST".into());
        Ok(self
            .mailboxes
            .iter()
            .map(|(name, m)| MailboxEntry {
                name: name.clone(),
                delimiter: Some('/'),
                attributes: m.attributes.clone(),
            })
            .collect())
    }

    async fn select(&mut self, mailbox: &str, qresync: Option<QresyncParams>) -> Result<Selected> {
        let caps = self.caps;
        let m = self.mailboxes.get(mailbox).ok_or_else(|| Error::Imap(format!("NO no such folder {mailbox}")))?;
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

    async fn uids(&mut self) -> Result<Vec<u32>> {
        self.log.push("UID SEARCH ALL".into());
        Ok(self.selected_mailbox()?.messages.keys().copied().collect())
    }

    async fn fetch_headers(&mut self, range: UidRange) -> Result<Vec<FetchedHeader>> {
        self.log.push(format!("UID FETCH {}:{} (HEADER)", range.from, range.to.map_or("*".into(), |t| t.to_string())));
        let condstore = self.caps.condstore;
        let m = self.selected_mailbox()?;
        Ok(m.messages
            .iter()
            .filter(|(uid, _)| range.contains(**uid))
            .map(|(uid, msg)| {
                let end = msg.raw.windows(4).position(|w| w == b"\r\n\r\n").map_or(msg.raw.len(), |p| p + 4);
                FetchedHeader {
                    uid: *uid,
                    flags: msg.flags.clone(),
                    modseq: condstore.then_some(msg.modseq),
                    internal_date: msg.internal_date,
                    size: msg.raw.len() as u32,
                    header: msg.raw[..end].to_vec(),
                }
            })
            .collect())
    }

    async fn fetch_flags(&mut self, range: UidRange, changed_since: Option<u64>) -> Result<Vec<FlagUpdate>> {
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

    async fn fetch_message(&mut self, uid: u32) -> Result<Vec<u8>> {
        self.log.push(format!("UID FETCH {uid} (BODY.PEEK[])"));
        let m = self.selected_mailbox()?;
        m.messages.get(&uid).map(|msg| msg.raw.clone()).ok_or_else(|| Error::Imap(format!("NO no message {uid}")))
    }

    async fn idle(&mut self, _timeout: Duration) -> Result<IdleOutcome> {
        if !self.caps.idle {
            return Err(Error::Imap("BAD IDLE not supported".into()));
        }
        self.log.push("IDLE".into());
        Ok(if std::mem::take(&mut self.changed) { IdleOutcome::Changed } else { IdleOutcome::Timeout })
    }
}
