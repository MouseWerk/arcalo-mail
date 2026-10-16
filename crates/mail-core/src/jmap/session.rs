//! A JMAP account seen through [`ImapClient`]: folders by name, messages by UID, changes by
//! MODSEQ, so `sync.rs` and the account worker run it unchanged.
//!
//! The mapping lives in the store (`jmap_state`, `jmap_mailboxes`, `jmap_emails`): each
//! message in each folder has a UID, counted up per folder; a message that leaves a folder
//! keeps its row as „gone“ with the MODSEQ of its leaving (QRESYNC's vanished UIDs), and one
//! that comes back gets a new UID, as an IMAP server would give it. [`JmapSession::refresh`]
//! brings the mapping in line with the server through `Email/changes` (or, the first time and
//! when the server cannot say what changed, `Email/query` and `Email/get` over everything);
//! `select` then answers QRESYNC from it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use base64::Engine;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::{JmapClient, cannot_calculate_changes, parse_events};
use crate::error::{Error, Result};
use crate::imap::{
    BodyPart, Capabilities, FetchedHeader, FlagUpdate, Flags, IdleOutcome, ImapClient, MailboxEntry, QresyncParams,
    Search, SelectInfo, Selected, UidRange,
};
use crate::store::Store;

/// One folder as JMAP names it.
#[derive(Debug, Clone)]
struct JMailbox {
    id: String,
    /// The path with `/` between parent and child, as the store names folders.
    name: String,
    role: Option<String>,
}

/// A row of `jmap_emails`.
#[derive(Debug, Clone)]
struct Entry {
    email_id: String,
    uid: u32,
    modseq: u64,
    keywords: Vec<String>,
    gone: bool,
}

pub struct JmapSession {
    client: JmapClient,
    /// Its own connection to the store, behind a lock so the session's futures stay `Send`
    /// (never held across an `await`).
    store: std::sync::Mutex<Store>,
    account: i64,
    mailboxes: Vec<JMailbox>,
    selected: Option<String>,
    poll: Duration,
    /// Per folder, the MODSEQ the last sync of it (a select with QRESYNC) saw.
    synced: HashMap<String, u64>,
}

/// The JMAP keywords of IMAP flags (RFC 8621, 4.1.1).
fn keyword_of(flag: &str) -> String {
    match flag.to_ascii_lowercase().as_str() {
        "\\seen" => "$seen".into(),
        "\\answered" => "$answered".into(),
        "\\flagged" => "$flagged".into(),
        "\\draft" => "$draft".into(),
        "\\deleted" => "$deleted".into(),
        other => other.into(),
    }
}

fn flags_of(keywords: &[String]) -> Flags {
    let mut f = Flags::default();
    for k in keywords {
        match k.as_str() {
            "$seen" => f.seen = true,
            "$answered" => f.answered = true,
            "$flagged" => f.flagged = true,
            "$draft" => f.draft = true,
            "$deleted" => f.deleted = true,
            other => f.keywords.push(other.to_string()),
        }
    }
    f
}

fn keywords_of(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .get("keywords")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter(|(_, on)| on.as_bool() == Some(true)).map(|(k, _)| k.to_ascii_lowercase()).collect())
        .unwrap_or_default();
    k.sort();
    k
}

fn mailbox_ids_of(v: &Value) -> Vec<String> {
    v.get("mailboxIds")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter(|(_, on)| on.as_bool() == Some(true)).map(|(k, _)| k.clone()).collect())
        .unwrap_or_default()
}

/// A stable, non-zero UIDVALIDITY for a JMAP folder id (FNV-1a).
pub fn uid_validity_of(mailbox_id: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in mailbox_id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h.max(1)
}

/// RFC 3339 → Unix seconds (`2026-10-16T09:30:00Z`, with an offset or fractions).
pub fn parse_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, time) = s.split_once(['T', 't', ' '])?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let (clock, offset) = match time.find(['Z', 'z', '+', '-']) {
        Some(i) => (&time[..i], &time[i..]),
        None => (time, "Z"),
    };
    let mut c = clock.split(':');
    let h: i64 = c.next()?.parse().ok()?;
    let mi: i64 = c.next()?.parse().ok()?;
    let se: i64 = c.next().unwrap_or("0").split('.').next()?.parse().ok()?;
    let off = if offset.eq_ignore_ascii_case("z") {
        0
    } else {
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let mut o = offset[1..].split(':');
        let oh: i64 = o.next()?.parse().ok()?;
        let om: i64 = o.next().unwrap_or("0").parse().ok()?;
        sign * (oh * 3600 + om * 60)
    };
    Some(crate::imap::client::days_from_civil(y, m, day) * 86400 + h * 3600 + mi * 60 + se - off)
}

/// The header block of a message from JMAP's raw `headers` (name and the value as sent).
fn header_bytes(v: &Value) -> Vec<u8> {
    let mut out = String::new();
    for h in v.get("headers").and_then(Value::as_array).into_iter().flatten() {
        let name = h.get("name").and_then(Value::as_str).unwrap_or_default();
        let value = h.get("value").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        out.push_str(name);
        out.push(':');
        out.push_str(value);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.into_bytes()
}

/// The UID mapping of one account in the store (see the module comment).
struct Mapping<'a> {
    db: &'a Store,
    account: i64,
}

impl Mapping<'_> {
    fn state(&self) -> Result<(Option<String>, Option<String>, u64)> {
        let row = self
            .db
            .conn()
            .query_row(
                "SELECT email_state, mailbox_state, modseq FROM jmap_state WHERE account_id = ?1",
                [self.account],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64)),
            )
            .optional()?;
        Ok(row.unwrap_or((None, None, 1)))
    }

    fn save_state(&self, email: Option<&str>, mailbox: Option<&str>, modseq: u64) -> Result<()> {
        self.db.conn().execute(
            "INSERT INTO jmap_state (account_id, email_state, mailbox_state, modseq) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (account_id) DO UPDATE SET email_state = ?2, mailbox_state = ?3, modseq = ?4",
            params![self.account, email, mailbox, modseq as i64],
        )?;
        Ok(())
    }

    fn entries_of_email(&self, email_id: &str) -> Result<Vec<(String, Entry)>> {
        let mut st = self.db.conn().prepare_cached(
            "SELECT mailbox, uid, modseq, keywords, gone FROM jmap_emails WHERE account_id = ?1 AND email_id = ?2",
        )?;
        let rows = st.query_map(params![self.account, email_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Entry {
                    email_id: email_id.to_string(),
                    uid: r.get(1)?,
                    modseq: r.get::<_, i64>(2)? as u64,
                    keywords: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                    gone: r.get(4)?,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn entries(&self, mailbox: &str) -> Result<Vec<Entry>> {
        let mut st = self.db.conn().prepare_cached(
            "SELECT email_id, uid, modseq, keywords, gone FROM jmap_emails WHERE account_id = ?1 AND mailbox = ?2
             ORDER BY uid",
        )?;
        let rows = st.query_map(params![self.account, mailbox], |r| {
            Ok(Entry {
                email_id: r.get(0)?,
                uid: r.get(1)?,
                modseq: r.get::<_, i64>(2)? as u64,
                keywords: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                gone: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn uid_next(&self, mailbox: &str) -> Result<u32> {
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT uid_next FROM jmap_mailboxes WHERE account_id = ?1 AND mailbox = ?2",
                params![self.account, mailbox],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(1))
    }

    fn take_uid(&self, mailbox: &str) -> Result<u32> {
        let uid = self.uid_next(mailbox)?;
        self.db.conn().execute(
            "INSERT INTO jmap_mailboxes (account_id, mailbox, uid_next) VALUES (?1, ?2, ?3)
             ON CONFLICT (account_id, mailbox) DO UPDATE SET uid_next = ?3",
            params![self.account, mailbox, uid + 1],
        )?;
        Ok(uid)
    }

    /// Puts one message where the server says it is: gone from the folders it left, a new UID
    /// in folders it joined, the new keywords where they changed. All at `modseq`.
    fn place(&self, email_id: &str, mailboxes: &[String], keywords: &[String], modseq: u64) -> Result<()> {
        let have = self.entries_of_email(email_id)?;
        let kw = serde_json::to_string(keywords)?;
        for (mb, e) in &have {
            if !e.gone && !mailboxes.contains(mb) {
                self.db.conn().execute(
                    "UPDATE jmap_emails SET gone = 1, modseq = ?4 WHERE account_id = ?1 AND mailbox = ?2 AND email_id = ?3",
                    params![self.account, mb, email_id, modseq as i64],
                )?;
            }
        }
        for mb in mailboxes {
            match have.iter().find(|(m, _)| m == mb) {
                Some((_, e)) if !e.gone => {
                    if e.keywords != keywords {
                        self.db.conn().execute(
                            "UPDATE jmap_emails SET keywords = ?4, modseq = ?5
                             WHERE account_id = ?1 AND mailbox = ?2 AND email_id = ?3",
                            params![self.account, mb, email_id, kw, modseq as i64],
                        )?;
                    }
                }
                _ => {
                    let uid = self.take_uid(mb)?;
                    self.db.conn().execute(
                        "INSERT OR REPLACE INTO jmap_emails (account_id, mailbox, email_id, uid, modseq, keywords, gone)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
                        params![self.account, mb, email_id, uid, modseq as i64, kw],
                    )?;
                }
            }
        }
        Ok(())
    }

    fn remove(&self, email_id: &str, modseq: u64) -> Result<()> {
        self.db.conn().execute(
            "UPDATE jmap_emails SET gone = 1, modseq = ?3 WHERE account_id = ?1 AND email_id = ?2 AND gone = 0",
            params![self.account, email_id, modseq as i64],
        )?;
        Ok(())
    }
}

impl JmapSession {
    pub fn new(client: JmapClient, store: Store, account: i64, poll: Duration) -> Self {
        JmapSession {
            client,
            store: std::sync::Mutex::new(store),
            account,
            mailboxes: vec![],
            selected: None,
            poll,
            synced: HashMap::new(),
        }
    }

    /// Runs `f` on the mapping (the store locked for its duration only).
    fn map<R>(&self, f: impl FnOnce(&Mapping<'_>) -> R) -> R {
        let g = self.store.lock().unwrap_or_else(|e| e.into_inner());
        f(&Mapping { db: &g, account: self.account })
    }

    /// `Email/get` of folders and keywords for these ids, in batches.
    async fn placement(&self, ids: &[String]) -> Result<Vec<(String, Vec<String>, Vec<String>)>> {
        let mut out = vec![];
        for chunk in ids.chunks(self.client.max_get) {
            let v = self
                .client
                .call("Email/get", json!({ "ids": chunk, "properties": ["id", "mailboxIds", "keywords"] }))
                .await?;
            for e in v.get("list").and_then(Value::as_array).into_iter().flatten() {
                let Some(id) = e.get("id").and_then(Value::as_str) else { continue };
                out.push((id.to_string(), mailbox_ids_of(e), keywords_of(e)));
            }
        }
        Ok(out)
    }

    /// The current `state` of Email (`Email/get` with no ids).
    async fn email_state(&self) -> Result<String> {
        let v = self.client.call("Email/get", json!({ "ids": [], "properties": ["id"] })).await?;
        Ok(v.get("state").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    /// Brings the mapping in line with the server.
    pub async fn refresh(&mut self) -> Result<()> {
        let (since, mailbox_state, modseq) = self.map(|m| m.state())?;
        let next = modseq + 1;
        if let Some(since) = since.clone() {
            let mut from = since.clone();
            let mut touched: Vec<String> = vec![];
            let mut destroyed: Vec<String> = vec![];
            let mut full = false;
            loop {
                match self.client.call("Email/changes", json!({ "sinceState": from, "maxChanges": 500 })).await {
                    Ok(v) => {
                        for key in ["created", "updated"] {
                            for id in v.get(key).and_then(Value::as_array).into_iter().flatten() {
                                touched.extend(id.as_str().map(str::to_string));
                            }
                        }
                        for id in v.get("destroyed").and_then(Value::as_array).into_iter().flatten() {
                            destroyed.extend(id.as_str().map(str::to_string));
                        }
                        from = v.get("newState").and_then(Value::as_str).unwrap_or(&from).to_string();
                        if v.get("hasMoreChanges").and_then(Value::as_bool) != Some(true) {
                            break;
                        }
                    }
                    Err(e) if cannot_calculate_changes(&e) => {
                        full = true;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            if !full {
                if from == since && touched.is_empty() && destroyed.is_empty() {
                    return Ok(());
                }
                let placed = self.placement(&touched).await?;
                let found: HashSet<&str> = placed.iter().map(|(id, _, _)| id.as_str()).collect();
                let gone: Vec<String> =
                    touched.iter().filter(|id| !found.contains(id.as_str())).cloned().chain(destroyed).collect();
                // Another connection of this account may have applied the same changes.
                if self.map(|m| m.state())?.0.as_deref() != Some(since.as_str()) {
                    return Ok(());
                }
                self.map(|m| {
                    m.db.batch(|| {
                        for (id, mbs, kw) in &placed {
                            m.place(id, mbs, kw, next)?;
                        }
                        for id in &gone {
                            m.remove(id, next)?;
                        }
                        m.save_state(Some(&from), mailbox_state.as_deref(), next)
                    })
                })?;
                return Ok(());
            }
        }
        // Everything: the state first, so nothing that happens meanwhile is missed.
        let state = self.email_state().await?;
        let mut ids: Vec<String> = vec![];
        let mut position = 0usize;
        loop {
            let v = self
                .client
                .call(
                    "Email/query",
                    json!({ "sort": [{ "property": "receivedAt", "isAscending": true }], "position": position,
                            "limit": 1000, "calculateTotal": true }),
                )
                .await?;
            let page: Vec<String> = v
                .get("ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect();
            let total = v.get("total").and_then(Value::as_u64).unwrap_or(0) as usize;
            position += page.len();
            let done = page.is_empty() || position >= total;
            ids.extend(page);
            if done {
                break;
            }
        }
        let placed = self.placement(&ids).await?;
        let present: HashSet<&str> = placed.iter().map(|(id, _, _)| id.as_str()).collect();
        self.map(|m| {
            let known: Vec<String> = {
                let mut st =
                    m.db.conn()
                        .prepare("SELECT DISTINCT email_id FROM jmap_emails WHERE account_id = ?1 AND gone = 0")?;
                st.query_map([m.account], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?
            };
            m.db.batch(|| {
                for (id, mbs, kw) in &placed {
                    m.place(id, mbs, kw, next)?;
                }
                for id in known.iter().filter(|id| !present.contains(id.as_str())) {
                    m.remove(id, next)?;
                }
                m.save_state(Some(&state), mailbox_state.as_deref(), next)
            })
        })
    }

    // ------------------------------------------------------------------ folders

    async fn load_mailboxes(&mut self) -> Result<()> {
        let v = self
            .client
            .call("Mailbox/get", json!({ "ids": null, "properties": ["id", "name", "parentId", "role", "sortOrder"] }))
            .await?;
        let list = v.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
        let by_id: HashMap<String, &Value> =
            list.iter().filter_map(|m| Some((m.get("id")?.as_str()?.to_string(), m))).collect();
        let path = |m: &Value| {
            let mut parts = vec![m.get("name").and_then(Value::as_str).unwrap_or_default().replace('/', "∕")];
            let mut parent = m.get("parentId").and_then(Value::as_str).map(str::to_string);
            let mut guard = 0;
            while let Some(p) = parent.take().filter(|_| guard < 20) {
                guard += 1;
                if let Some(pm) = by_id.get(&p) {
                    parts.push(pm.get("name").and_then(Value::as_str).unwrap_or_default().replace('/', "∕"));
                    parent = pm.get("parentId").and_then(Value::as_str).map(str::to_string);
                }
            }
            parts.reverse();
            parts.join("/")
        };
        self.mailboxes = list
            .iter()
            .filter_map(|m| {
                let role = m.get("role").and_then(Value::as_str).map(str::to_string);
                let mut name = path(m);
                // The store and the engine find the inbox as `INBOX`.
                if role.as_deref() == Some("inbox") && !name.contains('/') {
                    name = "INBOX".into();
                }
                Some(JMailbox { id: m.get("id")?.as_str()?.to_string(), name, role })
            })
            .collect();
        let state = v.get("state").and_then(Value::as_str).map(str::to_string);
        let (email, _, modseq) = self.map(|m| m.state())?;
        self.map(|m| m.save_state(email.as_deref(), state.as_deref(), modseq))?;
        Ok(())
    }

    async fn mailbox_id(&mut self, name: &str) -> Result<String> {
        if self.mailboxes.is_empty() || !self.mailboxes.iter().any(|m| m.name == name) {
            self.load_mailboxes().await?;
        }
        self.mailboxes
            .iter()
            .find(|m| m.name == name || (name.eq_ignore_ascii_case("INBOX") && m.role.as_deref() == Some("inbox")))
            .map(|m| m.id.clone())
            .ok_or_else(|| Error::Imap(format!("NO [NONEXISTENT] no folder {name}")))
    }

    fn selected(&self) -> Result<String> {
        self.selected.clone().ok_or_else(|| Error::Imap("BAD no folder selected".into()))
    }

    /// The JMAP ids of UIDs of the selected folder.
    fn ids_of(&self, uids: &[u32]) -> Result<Vec<String>> {
        let mb = self.selected()?;
        let want: HashSet<u32> = uids.iter().copied().collect();
        Ok(self
            .map(|m| m.entries(&mb))?
            .into_iter()
            .filter(|e| !e.gone && want.contains(&e.uid))
            .map(|e| e.email_id)
            .collect())
    }

    async fn email_set(&self, args: Value) -> Result<Value> {
        let v = self.client.call("Email/set", args).await?;
        for key in ["notCreated", "notUpdated", "notDestroyed"] {
            if let Some(e) = super::set_failure(&v, key) {
                return Err(Error::Imap(format!("NO jmap {e}")));
            }
        }
        Ok(v)
    }

    async fn patch_all(&self, ids: &[String], patch: Value) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let update: serde_json::Map<String, Value> = ids.iter().map(|id| (id.clone(), patch.clone())).collect();
        self.email_set(json!({ "update": update })).await.map(drop)
    }

    async fn mailbox_set(&mut self, args: Value) -> Result<()> {
        let v = self.client.call("Mailbox/set", args).await?;
        for key in ["notCreated", "notUpdated", "notDestroyed"] {
            if let Some(e) = super::set_failure(&v, key) {
                return Err(Error::Imap(format!("NO jmap {e}")));
            }
        }
        self.load_mailboxes().await
    }

    fn parent_of(&self, name: &str) -> Result<(Option<String>, String)> {
        match name.rsplit_once('/') {
            Some((parent, leaf)) => {
                let p = self
                    .mailboxes
                    .iter()
                    .find(|m| m.name == parent)
                    .ok_or_else(|| Error::Imap(format!("NO [NONEXISTENT] no folder {parent}")))?;
                Ok((Some(p.id.clone()), leaf.to_string()))
            }
            None => Ok((None, name.to_string())),
        }
    }
}

impl ImapClient for JmapSession {
    async fn capabilities(&mut self) -> Result<Capabilities> {
        Ok(Capabilities {
            idle: true,
            condstore: true,
            qresync: true,
            uidplus: true,
            moves: true,
            special_use: true,
            gmail: false,
        })
    }

    async fn list(&mut self) -> Result<Vec<MailboxEntry>> {
        self.load_mailboxes().await?;
        Ok(self
            .mailboxes
            .iter()
            .map(|m| MailboxEntry {
                name: m.name.clone(),
                delimiter: Some('/'),
                attributes: m.role.iter().map(|r| format!("\\{}{}", r[..1].to_ascii_uppercase(), &r[1..])).collect(),
            })
            .collect())
    }

    /// A sync's select (with QRESYNC) brings the mapping up to date first; a plain one (IDLE,
    /// a body to fetch) answers from it as it is, so only a sync moves the mapping on and a
    /// change seen by another select is never lost for the sync (see `idle`).
    async fn select(&mut self, mailbox: &str, qresync: Option<QresyncParams>) -> Result<Selected> {
        let id = self.mailbox_id(mailbox).await?;
        if qresync.is_some() || self.map(|m| m.state())?.0.is_none() {
            self.refresh().await?;
        }
        let (_, _, modseq) = self.map(|m| m.state())?;
        if qresync.is_some() {
            self.synced.insert(id.clone(), modseq);
        }
        let entries = self.map(|m| m.entries(&id))?;
        let uid_validity = uid_validity_of(&id);
        let info = SelectInfo {
            uid_validity,
            uid_next: self.map(|m| m.uid_next(&id))?,
            exists: entries.iter().filter(|e| !e.gone).count() as u32,
            highest_modseq: Some(modseq),
        };
        let mut out = Selected { info, vanished: vec![], changed: vec![] };
        if let Some(q) = qresync.filter(|q| q.uid_validity == uid_validity) {
            for e in entries.iter().filter(|e| e.modseq > q.modseq) {
                if e.gone {
                    out.vanished.push(e.uid);
                } else {
                    out.changed.push(FlagUpdate { uid: e.uid, flags: flags_of(&e.keywords), modseq: Some(e.modseq) });
                }
            }
        }
        self.selected = Some(id);
        Ok(out)
    }

    async fn uids(&mut self) -> Result<Vec<u32>> {
        let mb = self.selected()?;
        Ok(self.map(|m| m.entries(&mb))?.into_iter().filter(|e| !e.gone).map(|e| e.uid).collect())
    }

    async fn fetch_headers(&mut self, range: UidRange) -> Result<Vec<FetchedHeader>> {
        let mb = self.selected()?;
        let wanted: Vec<Entry> =
            self.map(|m| m.entries(&mb))?.into_iter().filter(|e| !e.gone && range.contains(e.uid)).collect();
        let mut out = vec![];
        for chunk in wanted.chunks(self.client.max_get.min(200)) {
            let ids: Vec<&str> = chunk.iter().map(|e| e.email_id.as_str()).collect();
            let v = self
                .client
                .call("Email/get", json!({ "ids": ids, "properties": ["id", "receivedAt", "size", "headers"] }))
                .await?;
            let list = v.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
            for e in chunk {
                let Some(m) = list.iter().find(|m| m.get("id").and_then(Value::as_str) == Some(e.email_id.as_str()))
                else {
                    continue;
                };
                out.push(FetchedHeader {
                    uid: e.uid,
                    flags: flags_of(&e.keywords),
                    modseq: Some(e.modseq),
                    internal_date: m.get("receivedAt").and_then(Value::as_str).and_then(parse_utc).unwrap_or(0),
                    size: m.get("size").and_then(Value::as_u64).unwrap_or(0) as u32,
                    header: header_bytes(m),
                });
            }
        }
        Ok(out)
    }

    async fn fetch_flags(&mut self, range: UidRange, changed_since: Option<u64>) -> Result<Vec<FlagUpdate>> {
        let mb = self.selected()?;
        Ok(self
            .map(|m| m.entries(&mb))?
            .into_iter()
            .filter(|e| !e.gone && range.contains(e.uid) && changed_since.is_none_or(|s| e.modseq > s))
            .map(|e| FlagUpdate { uid: e.uid, flags: flags_of(&e.keywords), modseq: Some(e.modseq) })
            .collect())
    }

    async fn fetch_message(&mut self, uid: u32) -> Result<Vec<u8>> {
        let id = self.ids_of(&[uid])?.pop().ok_or_else(|| Error::Imap(format!("NO no message {uid}")))?;
        let v = self.client.call("Email/get", json!({ "ids": [id], "properties": ["blobId"] })).await?;
        let blob = v
            .pointer("/list/0/blobId")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Imap(format!("NO no message {uid}")))?
            .to_string();
        self.client.download(&blob, "message.eml", "message/rfc822").await
    }

    async fn fetch_structure(&mut self, uid: u32) -> Result<Vec<BodyPart>> {
        let id = self.ids_of(&[uid])?.pop().ok_or_else(|| Error::Imap(format!("NO no message {uid}")))?;
        let v = self
            .client
            .call(
                "Email/get",
                json!({ "ids": [id], "properties": ["bodyStructure"],
                        "bodyProperties": ["partId", "blobId", "size", "type", "name", "disposition", "cid", "subParts"] }),
            )
            .await?;
        fn walk(p: &Value, out: &mut Vec<BodyPart>) {
            if let Some(subs) = p.get("subParts").and_then(Value::as_array).filter(|s| !s.is_empty()) {
                for s in subs {
                    walk(s, out);
                }
                return;
            }
            let Some(blob) = p.get("blobId").and_then(Value::as_str) else { return };
            let mime = p.get("type").and_then(Value::as_str).unwrap_or("application/octet-stream").to_ascii_lowercase();
            out.push(BodyPart {
                section: format!("blob:{blob}:{mime}"),
                mime_type: mime,
                filename: p.get("name").and_then(Value::as_str).map(str::to_string),
                size: p.get("size").and_then(Value::as_u64).unwrap_or(0),
                content_id: p.get("cid").and_then(Value::as_str).map(|c| c.trim_matches(['<', '>']).to_string()),
                inline: p.get("disposition").and_then(Value::as_str) != Some("attachment"),
            });
        }
        let mut out = vec![];
        if let Some(root) = v.pointer("/list/0/bodyStructure") {
            walk(root, &mut out);
        }
        Ok(out)
    }

    /// A part as a small MIME entity of its own (header and Base64 body), as the IMAP
    /// client's `BODY.PEEK[n.MIME]` and `BODY.PEEK[n]` give it.
    async fn fetch_section(&mut self, _uid: u32, section: &str) -> Result<Vec<u8>> {
        let rest = section.strip_prefix("blob:").ok_or_else(|| Error::Imap("NO not a JMAP part".into()))?;
        let (blob, mime) = rest.rsplit_once(':').unwrap_or((rest, "application/octet-stream"));
        let data = self.client.download(blob, "part", mime).await?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        let mut out = format!("Content-Type: {mime}\r\nContent-Transfer-Encoding: base64\r\n\r\n").into_bytes();
        for line in b64.as_bytes().chunks(76) {
            out.extend_from_slice(line);
            out.extend_from_slice(b"\r\n");
        }
        Ok(out)
    }

    /// Push over EventSource where the server offers it, else a check every `poll`; ends on
    /// a change of Email or Mailbox, at `timeout` or when `wake` is notified.
    async fn idle(&mut self, timeout: Duration, wake: &Notify) -> Result<IdleOutcome> {
        let (email_state, mailbox_state, _) = self.map(|m| m.state())?;
        let differs = |c: &super::StateChange| {
            c.email.as_ref().is_some_and(|s| Some(s) != email_state.as_ref())
                || c.mailbox.as_ref().is_some_and(|s| Some(s) != mailbox_state.as_ref())
        };
        // Changes of this folder that a sync of another folder took into the mapping.
        if let Some(mb) = self.selected.clone()
            && let Some(seen) = self.synced.get(&mb)
            && self.map(|m| m.entries(&mb))?.iter().any(|e| e.modseq > *seen)
        {
            return Ok(IdleOutcome::Changed);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        if self.client.has_push() {
            let mut resp = self.client.events().await?;
            let mut buf = String::new();
            loop {
                tokio::select! {
                    chunk = resp.chunk() => match chunk {
                        Ok(Some(bytes)) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                            if parse_events(&mut buf, &self.client.account_id).iter().any(differs) {
                                return Ok(IdleOutcome::Changed);
                            }
                        }
                        // The server closed the stream: look again (a sync is cheap with changes).
                        Ok(None) => return Ok(IdleOutcome::Timeout),
                        Err(e) => return Err(Error::Network(e.to_string())),
                    },
                    _ = tokio::time::sleep_until(deadline) => return Ok(IdleOutcome::Timeout),
                    _ = wake.notified() => return Ok(IdleOutcome::Interrupted),
                }
            }
        }
        loop {
            let next = (tokio::time::Instant::now() + self.poll).min(deadline);
            tokio::select! {
                _ = tokio::time::sleep_until(next) => {}
                _ = wake.notified() => return Ok(IdleOutcome::Interrupted),
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(IdleOutcome::Timeout);
            }
            let r = self
                .client
                .call_many(vec![
                    ("Email/get", json!({ "accountId": self.client.account_id, "ids": [], "properties": ["id"] })),
                    ("Mailbox/get", json!({ "accountId": self.client.account_id, "ids": [], "properties": ["id"] })),
                ])
                .await?;
            let state_of = |i: usize| {
                r.get(i)
                    .and_then(|x| x.as_ref().ok())
                    .and_then(|v| v.get("state"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            let c = super::StateChange { email: state_of(0), mailbox: state_of(1) };
            if differs(&c) {
                return Ok(IdleOutcome::Changed);
            }
        }
    }

    async fn store(&mut self, uids: &[u32], flags: &[&str], add: bool) -> Result<()> {
        let ids = self.ids_of(uids)?;
        let mut patch = serde_json::Map::new();
        for f in flags {
            patch.insert(format!("keywords/{}", keyword_of(f)), if add { Value::Bool(true) } else { Value::Null });
        }
        self.patch_all(&ids, Value::Object(patch)).await
    }

    async fn copy(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        let target = self.mailbox_id(to).await?;
        let ids = self.ids_of(uids)?;
        self.patch_all(&ids, json!({ format!("mailboxIds/{target}"): true })).await?;
        Ok(vec![])
    }

    async fn move_to(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        let target = self.mailbox_id(to).await?;
        let from = self.selected()?;
        let ids = self.ids_of(uids)?;
        if target != from {
            self.patch_all(&ids, json!({ format!("mailboxIds/{from}"): null, format!("mailboxIds/{target}"): true }))
                .await?;
        }
        Ok(vec![])
    }

    /// Out of the selected folder for good: destroyed when it is in no other folder, else
    /// only taken out of this one.
    async fn expunge(&mut self, uids: &[u32]) -> Result<()> {
        let from = self.selected()?;
        let ids = self.ids_of(uids)?;
        if ids.is_empty() {
            return Ok(());
        }
        let placed = self.placement(&ids).await?;
        let (only_here, elsewhere): (Vec<_>, Vec<_>) = placed.into_iter().partition(|(_, mbs, _)| mbs.len() <= 1);
        if !only_here.is_empty() {
            let destroy: Vec<&str> = only_here.iter().map(|(id, _, _)| id.as_str()).collect();
            self.email_set(json!({ "destroy": destroy })).await?;
        }
        let rest: Vec<String> = elsewhere.into_iter().map(|(id, _, _)| id).collect();
        self.patch_all(&rest, json!({ format!("mailboxIds/{from}"): null })).await
    }

    async fn append(&mut self, mailbox: &str, flags: &[&str], raw: &[u8]) -> Result<Option<u32>> {
        let target = self.mailbox_id(mailbox).await?;
        let blob = self.client.upload(raw, "message/rfc822").await?;
        let keywords: serde_json::Map<String, Value> =
            flags.iter().map(|f| (keyword_of(f), Value::Bool(true))).collect();
        let v = self
            .client
            .call(
                "Email/import",
                json!({ "emails": { "a": { "blobId": blob, "mailboxIds": { target: true }, "keywords": keywords } } }),
            )
            .await?;
        if let Some(e) = super::set_failure(&v, "notCreated") {
            return Err(Error::Imap(format!("NO jmap {e}")));
        }
        Ok(None)
    }

    async fn search(&mut self, query: &Search) -> Result<Vec<u32>> {
        let mb = self.selected()?;
        let filter = match query {
            Search::Text(t) => json!({ "inMailbox": mb, "text": t }),
            Search::MessageId(id) => {
                json!({ "inMailbox": mb, "header": ["Message-ID", format!("<{}>", id.trim_matches(['<', '>']))] })
            }
        };
        let v = self.client.call("Email/query", json!({ "filter": filter, "limit": 1000 })).await?;
        let found: BTreeSet<String> = v
            .get("ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect();
        // Messages that arrived since the last sync have no UID yet; the next sync brings them.
        Ok(self
            .map(|m| m.entries(&mb))?
            .into_iter()
            .filter(|e| !e.gone && found.contains(&e.email_id))
            .map(|e| e.uid)
            .collect())
    }

    async fn create(&mut self, mailbox: &str) -> Result<()> {
        self.load_mailboxes().await?;
        let (parent, leaf) = self.parent_of(mailbox)?;
        self.mailbox_set(json!({ "create": { "n": { "name": leaf, "parentId": parent } } })).await
    }

    async fn delete(&mut self, mailbox: &str) -> Result<()> {
        let id = self.mailbox_id(mailbox).await?;
        self.mailbox_set(json!({ "destroy": [id], "onDestroyRemoveEmails": false })).await
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let id = self.mailbox_id(from).await?;
        let (parent, leaf) = self.parent_of(to)?;
        self.mailbox_set(json!({ "update": { id: { "name": leaf, "parentId": parent } } })).await
    }

    async fn logout(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn dates_flags_and_validity() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2026-10-16T09:30:00+02:00"), Some(1_792_135_800));
        assert_eq!(parse_utc("2026-10-16T07:30:00.123Z"), Some(1_792_135_800));
        let f = flags_of(&["$seen".into(), "$flagged".into(), "projekt".into()]);
        assert!(f.seen && f.flagged && !f.answered);
        assert_eq!(f.keywords, ["projekt"]);
        assert_eq!(keyword_of("\\Answered"), "$answered");
        assert_ne!(uid_validity_of("a"), uid_validity_of("b"));
        assert_ne!(uid_validity_of(""), 0);
        let h = header_bytes(
            &json!({ "headers": [{ "name": "Subject", "value": " Hallo\r\n Welt" }, { "name": "From", "value": " a@x" }] }),
        );
        assert_eq!(h, b"Subject: Hallo\r\n Welt\r\nFrom: a@x\r\n\r\n");
    }
}
