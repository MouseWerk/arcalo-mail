//! A JMAP server in memory for tests, over the fake IMAP server's mailboxes
//! ([`crate::imap::fake::FakeImap`]): the same folders and messages seen through RFC 8620/8621,
//! so a test can deliver mail one way and watch it arrive the other. A message in a folder is
//! one Email (`<folder id>-<uid>`); moving it gives it a new id, which a client sees as
//! destroyed and created, as JMAP allows.
//!
//! States are snapshots: `Email/changes` and `Mailbox/changes` compare the snapshot of the
//! given state with the current one. The core's tests serve it on loopback, the test server
//! (`crates/mail-testserver`) on its control port, with push over EventSource.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use base64::Engine;
use serde_json::{Map, Value, json};

use super::{CORE, MAIL, SUBMISSION};
use crate::imap::fake::FakeImap;
use crate::imap::{DRAFT, FLAGGED, Flags, SEEN, Search};
use crate::oauth::fake::FakeReply;

pub const ACCOUNT: &str = "a1";

/// A message JMAP's way, for the sender of submitted mail.
#[derive(Debug, Clone)]
pub struct Submitted {
    pub from: String,
    pub to: Vec<String>,
    pub raw: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct FakeJmapState {
    /// Folder name → Mailbox id.
    ids: BTreeMap<String, String>,
    next: u64,
    emails: Vec<BTreeMap<String, Vec<String>>>,
    boxes: Vec<BTreeMap<String, String>>,
    uploads: HashMap<String, Vec<u8>>,
    pub submitted: Vec<Submitted>,
    /// Offer push (`eventSourceUrl`) in the session.
    pub push: bool,
    pub user: String,
    /// Part of every state, so states of an earlier server are never taken for current ones.
    epoch: u64,
    /// Calendars offered (`urn:ietf:params:jmap:calendars`), when set.
    pub calendars: Option<crate::calendar::jmap::fake::FakeJmapCalendars>,
    /// A `role` sent as it is for a folder (a server that sends odd values).
    pub raw_roles: BTreeMap<String, String>,
    /// Further sender identities (aliases).
    pub aliases: Vec<String>,
}

static EPOCHS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn reply(status: u16, content_type: &str, body: Vec<u8>) -> FakeReply {
    FakeReply { status, content_type: content_type.into(), body, location: None }
}

fn error(kind: &str) -> Value {
    json!({ "type": kind })
}

fn date(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn keywords(f: &Flags) -> Vec<String> {
    let mut k: Vec<String> = f.keywords.iter().map(|k| k.to_ascii_lowercase()).collect();
    for (on, name) in [
        (f.seen, "$seen"),
        (f.answered, "$answered"),
        (f.flagged, "$flagged"),
        (f.draft, "$draft"),
        (f.deleted, "$deleted"),
    ] {
        if on {
            k.push(name.into());
        }
    }
    k.sort();
    k.dedup();
    k
}

fn flags(keywords: &BTreeSet<String>) -> Flags {
    let mut f = Flags::default();
    for k in keywords {
        match k.as_str() {
            "$seen" => f.seen = true,
            "$answered" => f.answered = true,
            "$flagged" => f.flagged = true,
            "$draft" => f.draft = true,
            "$deleted" => f.deleted = true,
            other => f.keywords.push(other.into()),
        }
    }
    f
}

/// The header fields of a message as JMAP's raw `headers` (the value as sent, unfolded lines
/// kept).
fn raw_headers(raw: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(raw);
    let head = text.split("\r\n\r\n").next().unwrap_or_default();
    let mut out: Vec<(String, String)> = vec![];
    for line in head.split("\r\n") {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.1.push_str("\r\n");
                last.1.push_str(line);
            }
        } else if let Some((name, value)) = line.split_once(':') {
            out.push((name.to_string(), value.to_string()));
        }
    }
    out.into_iter().map(|(name, value)| json!({ "name": name, "value": value })).collect()
}

impl FakeJmapState {
    pub fn new(user: &str, push: bool) -> Self {
        let epoch = EPOCHS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        FakeJmapState { user: user.into(), push, epoch, ..Default::default() }
    }

    fn sync_ids(&mut self, fake: &FakeImap) {
        self.ids.retain(|name, _| fake.mailboxes.contains_key(name));
        for name in fake.mailboxes.keys() {
            if !self.ids.contains_key(name) {
                self.next += 1;
                self.ids.insert(name.clone(), format!("m{}", self.next));
            }
        }
    }

    fn name_of(&self, id: &str) -> Option<String> {
        self.ids.iter().find(|(_, v)| *v == id).map(|(k, _)| k.clone())
    }

    fn email_id(&self, mailbox: &str, uid: u32) -> String {
        format!("{}-{uid}", self.ids.get(mailbox).cloned().unwrap_or_default())
    }

    /// The folder and UID of an Email id.
    fn locate(&self, fake: &FakeImap, id: &str) -> Option<(String, u32)> {
        let (mb, uid) = id.rsplit_once('-')?;
        let name = self.name_of(mb)?;
        let uid: u32 = uid.parse().ok()?;
        fake.mailboxes.get(&name)?.messages.contains_key(&uid).then_some((name, uid))
    }

    fn snapshot(&self, fake: &FakeImap) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, String>) {
        let mut emails = BTreeMap::new();
        for (name, m) in &fake.mailboxes {
            for (uid, msg) in &m.messages {
                emails.insert(self.email_id(name, *uid), keywords(&msg.flags));
            }
        }
        let boxes = self.ids.iter().map(|(name, id)| (id.clone(), name.clone())).collect();
        (emails, boxes)
    }

    /// The current states of Email and Mailbox (a new one after every change).
    pub fn states(&mut self, fake: &FakeImap) -> (String, String) {
        self.sync_ids(fake);
        let (emails, boxes) = self.snapshot(fake);
        if self.emails.last() != Some(&emails) {
            self.emails.push(emails);
        }
        if self.boxes.last() != Some(&boxes) {
            self.boxes.push(boxes);
        }
        (format!("{}-{}", self.epoch, self.emails.len() - 1), format!("{}-{}", self.epoch, self.boxes.len() - 1))
    }

    fn since(&self, args: &Value) -> Option<usize> {
        let (epoch, n) = args.get("sinceState")?.as_str()?.split_once('-')?;
        (epoch.parse::<u64>().ok()? == self.epoch).then_some(())?;
        n.parse().ok()
    }

    /// One EventSource `state` event (RFC 8620, 7.3).
    pub fn event(&mut self, fake: &FakeImap) -> String {
        let (e, m) = self.states(fake);
        let data = json!({ "@type": "StateChange", "changed": { ACCOUNT: { "Email": e, "Mailbox": m } } });
        format!("event: state\ndata: {data}\n\n")
    }

    pub fn session(&self, base: &str) -> Value {
        let mut s = json!({
            "capabilities": {
                CORE: { "maxSizeUpload": 50_000_000, "maxObjectsInGet": 100, "maxCallsInRequest": 16 },
                MAIL: {},
                SUBMISSION: {},
            },
            "accounts": { ACCOUNT: { "name": self.user, "isPersonal": true, "accountCapabilities": { MAIL: {}, SUBMISSION: {} } } },
            "primaryAccounts": { MAIL: ACCOUNT, SUBMISSION: ACCOUNT, CORE: ACCOUNT },
            "username": self.user,
            "apiUrl": format!("{base}/jmap/api"),
            "downloadUrl": format!("{base}/jmap/download/{{accountId}}/{{blobId}}/{{name}}?type={{type}}"),
            "uploadUrl": format!("{base}/jmap/upload/{{accountId}}/"),
            "state": "1",
        });
        if self.calendars.is_some() {
            let cal = crate::calendar::jmap::CALENDARS;
            s["capabilities"][cal] = json!({});
            s["primaryAccounts"][cal] = json!(ACCOUNT);
        }
        if self.push {
            s["eventSourceUrl"] =
                json!(format!("{base}/jmap/events?types={{types}}&closeafter={{closeafter}}&ping={{ping}}"));
        }
        s
    }

    /// Answers a request to the session, the API, a download or an upload. The caller checked
    /// the sign-in.
    pub fn handle(&mut self, fake: &mut FakeImap, method: &str, target: &str, body: &[u8], base: &str) -> FakeReply {
        self.sync_ids(fake);
        let path = target.split('?').next().unwrap_or(target);
        if method == "GET" && (path == "/.well-known/jmap" || path == "/jmap/session") {
            return reply(200, "application/json", self.session(base).to_string().into_bytes());
        }
        if method == "POST" && path == "/jmap/api" {
            let Ok(req) = serde_json::from_slice::<Value>(body) else {
                return reply(400, "application/json", br#"{"type":"urn:ietf:params:jmap:error:notJSON"}"#.to_vec());
            };
            let mut responses = vec![];
            for call in req.get("methodCalls").and_then(Value::as_array).into_iter().flatten() {
                let name = call.get(0).and_then(Value::as_str).unwrap_or_default();
                let args = call.get(1).cloned().unwrap_or(Value::Null);
                let id = call.get(2).cloned().unwrap_or(Value::Null);
                match self.method(fake, name, &args) {
                    Ok(v) => responses.push(json!([name, v, id])),
                    Err(e) => responses.push(json!(["error", e, id])),
                }
            }
            let out = json!({ "methodResponses": responses, "sessionState": "1" });
            return reply(200, "application/json", out.to_string().into_bytes());
        }
        if method == "GET"
            && let Some(rest) = path.strip_prefix("/jmap/download/")
        {
            let blob = crate::oauth::form_decode(&format!("b={}", rest.split('/').nth(1).unwrap_or_default()))
                .pop()
                .map(|(_, v)| v)
                .unwrap_or_default();
            return match self.blob(fake, &blob) {
                Some(data) => reply(200, "application/octet-stream", data),
                None => reply(404, "application/json", b"{}".to_vec()),
            };
        }
        if method == "POST" && path.starts_with("/jmap/upload/") {
            self.next += 1;
            let id = format!("u{}", self.next);
            self.uploads.insert(id.clone(), body.to_vec());
            let v = json!({ "accountId": ACCOUNT, "blobId": id, "type": "message/rfc822", "size": body.len() });
            return reply(201, "application/json", v.to_string().into_bytes());
        }
        reply(404, "application/json", b"{}".to_vec())
    }

    fn blob(&self, fake: &FakeImap, blob: &str) -> Option<Vec<u8>> {
        if let Some(data) = self.uploads.get(blob) {
            return Some(data.clone());
        }
        if let Some(rest) = blob.strip_prefix("p:") {
            let (email, section) = rest.split_once(':')?;
            let (mb, uid) = self.locate(fake, email)?;
            let raw = &fake.mailboxes[&mb].messages[&uid].raw;
            let part = crate::parse::section(raw, section)?;
            let decoded = mail_parser::MessageParser::default()
                .parse(&part)
                .and_then(|m| m.part(0).map(|p| p.contents().to_vec()))
                .unwrap_or(part);
            return Some(decoded);
        }
        let email = blob.strip_prefix("b:")?;
        let (mb, uid) = self.locate(fake, email)?;
        Some(fake.mailboxes[&mb].messages[&uid].raw.clone())
    }

    fn mailbox_json(&self, fake: &FakeImap, name: &str, id: &str) -> Value {
        let m = &fake.mailboxes[name];
        let role = if let Some(r) = self.raw_roles.get(name) {
            Some(r.clone())
        } else if name.eq_ignore_ascii_case("INBOX") {
            Some("inbox".to_string())
        } else {
            m.attributes.iter().find_map(|a| {
                let r = a.trim_start_matches('\\').to_ascii_lowercase();
                ["sent", "drafts", "trash", "junk", "archive", "all", "flagged", "important"]
                    .contains(&r.as_str())
                    .then_some(r)
            })
        };
        let (parent, leaf) = match name.rsplit_once('/') {
            Some((p, l)) => (self.ids.get(p).cloned(), l.to_string()),
            None => (None, if name == "INBOX" { "Inbox".into() } else { name.to_string() }),
        };
        json!({
            "id": id, "name": leaf, "parentId": parent, "role": role, "sortOrder": 0,
            "totalEmails": m.messages.len(),
            "unreadEmails": m.messages.values().filter(|x| !x.flags.seen).count(),
        })
    }

    fn email_json(&self, fake: &FakeImap, mb: &str, uid: u32) -> Value {
        let msg = &fake.mailboxes[mb].messages[&uid];
        let id = self.email_id(mb, uid);
        let kw: Map<String, Value> = keywords(&msg.flags).into_iter().map(|k| (k, Value::Bool(true))).collect();
        let leaves: Vec<Value> = crate::parse::structure(&msg.raw)
            .into_iter()
            .map(|p| {
                json!({
                    "partId": p.section, "blobId": format!("p:{id}:{}", p.section), "size": p.size,
                    "type": p.mime_type, "name": p.filename, "cid": p.content_id,
                    "disposition": if p.inline { "inline" } else { "attachment" },
                })
            })
            .collect();
        json!({
            "id": id, "blobId": format!("b:{id}"), "threadId": format!("t{id}"),
            "mailboxIds": { self.ids[mb].clone(): true }, "keywords": kw,
            "size": msg.raw.len(), "receivedAt": date(msg.internal_date),
            "headers": raw_headers(&msg.raw),
            "bodyStructure": { "type": "multipart/mixed", "subParts": leaves },
        })
    }

    fn method(&mut self, fake: &mut FakeImap, name: &str, args: &Value) -> Result<Value, Value> {
        if name.starts_with("Calendar")
            && let Some(c) = self.calendars.as_mut()
        {
            return c.method(name, args);
        }
        let (email_state, mailbox_state) = self.states(fake);
        let ids_arg = |a: &Value| -> Option<Vec<String>> {
            a.get("ids")
                .and_then(Value::as_array)
                .map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        };
        match name {
            "Mailbox/get" => {
                let wanted = ids_arg(args);
                let list: Vec<Value> = self
                    .ids
                    .iter()
                    .filter(|(_, id)| wanted.as_ref().is_none_or(|w| w.contains(id)))
                    .map(|(n, id)| self.mailbox_json(fake, n, id))
                    .collect();
                Ok(json!({ "accountId": ACCOUNT, "state": mailbox_state, "list": list, "notFound": [] }))
            }
            "Mailbox/changes" => {
                let since = self.since(args).ok_or_else(|| error("cannotCalculateChanges"))?;
                let old = self.boxes.get(since).cloned().ok_or_else(|| error("cannotCalculateChanges"))?;
                let cur = self.boxes.last().cloned().unwrap_or_default();
                let created: Vec<&String> = cur.keys().filter(|k| !old.contains_key(*k)).collect();
                let destroyed: Vec<&String> = old.keys().filter(|k| !cur.contains_key(*k)).collect();
                let updated: Vec<&String> =
                    cur.iter().filter(|(k, v)| old.get(*k).is_some_and(|o| o != *v)).map(|(k, _)| k).collect();
                Ok(
                    json!({ "accountId": ACCOUNT, "oldState": format!("{}-{since}", self.epoch), "newState": mailbox_state,
                    "hasMoreChanges": false, "created": created, "updated": updated, "destroyed": destroyed, "updatedProperties": null }),
                )
            }
            "Mailbox/set" => self.mailbox_set(fake, args, &mailbox_state),
            "Email/get" => {
                let wanted = ids_arg(args).unwrap_or_default();
                let mut list = vec![];
                let mut not_found = vec![];
                for id in wanted {
                    match self.locate(fake, &id) {
                        Some((mb, uid)) => list.push(self.email_json(fake, &mb, uid)),
                        None => not_found.push(id),
                    }
                }
                Ok(json!({ "accountId": ACCOUNT, "state": email_state, "list": list, "notFound": not_found }))
            }
            "Email/changes" => {
                let since = self.since(args).ok_or_else(|| error("cannotCalculateChanges"))?;
                let old = self.emails.get(since).cloned().ok_or_else(|| error("cannotCalculateChanges"))?;
                let cur = self.emails.last().cloned().unwrap_or_default();
                let created: Vec<&String> = cur.keys().filter(|k| !old.contains_key(*k)).collect();
                let destroyed: Vec<&String> = old.keys().filter(|k| !cur.contains_key(*k)).collect();
                let updated: Vec<&String> =
                    cur.iter().filter(|(k, v)| old.get(*k).is_some_and(|o| o != *v)).map(|(k, _)| k).collect();
                Ok(
                    json!({ "accountId": ACCOUNT, "oldState": format!("{}-{since}", self.epoch), "newState": email_state,
                    "hasMoreChanges": false, "created": created, "updated": updated, "destroyed": destroyed }),
                )
            }
            "Email/query" => {
                let filter = args.get("filter").cloned().unwrap_or(Value::Null);
                let in_mailbox = filter.get("inMailbox").and_then(Value::as_str).and_then(|id| self.name_of(id));
                let mut hits: Vec<(i64, String)> = vec![];
                for (name, m) in &fake.mailboxes {
                    if in_mailbox.as_ref().is_some_and(|n| n != name) {
                        continue;
                    }
                    for (uid, msg) in &m.messages {
                        hits.push((msg.internal_date, self.email_id(name, *uid)));
                    }
                }
                if let Some(mb) = &in_mailbox {
                    let search = if let Some(t) = filter.get("text").and_then(Value::as_str) {
                        Some(Search::Text(t.into()))
                    } else {
                        filter.pointer("/header/1").and_then(Value::as_str).map(|v| Search::MessageId(v.into()))
                    };
                    if let Some(s) = search {
                        fake.selected = Some(mb.clone());
                        let uids: BTreeSet<u32> = fake.do_search(&s).unwrap_or_default().into_iter().collect();
                        hits.retain(|(_, id)| {
                            id.rsplit_once('-').and_then(|(_, u)| u.parse().ok()).is_some_and(|u| uids.contains(&u))
                        });
                    }
                }
                let ascending = args.pointer("/sort/0/isAscending").and_then(Value::as_bool).unwrap_or(false);
                hits.sort();
                if !ascending {
                    hits.reverse();
                }
                let total = hits.len();
                let position = args.get("position").and_then(Value::as_u64).unwrap_or(0) as usize;
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(1000) as usize;
                let ids: Vec<String> = hits.into_iter().skip(position).take(limit).map(|(_, id)| id).collect();
                Ok(json!({ "accountId": ACCOUNT, "queryState": email_state, "canCalculateChanges": false,
                    "position": position, "ids": ids, "total": total }))
            }
            "Email/set" => {
                let mut updated = Map::new();
                let mut not_updated = Map::new();
                for (id, patch) in args.get("update").and_then(Value::as_object).into_iter().flatten() {
                    match self.patch(fake, id, patch) {
                        Ok(()) => {
                            updated.insert(id.clone(), Value::Null);
                        }
                        Err(e) => {
                            not_updated.insert(id.clone(), e);
                        }
                    }
                }
                let mut destroyed = vec![];
                let mut not_destroyed = Map::new();
                for id in ids_of(args.get("destroy")) {
                    match self.locate(fake, &id) {
                        Some((mb, uid)) => {
                            fake.selected = Some(mb);
                            let _ = fake.do_expunge(&[uid]);
                            destroyed.push(id);
                        }
                        None => {
                            not_destroyed.insert(id, error("notFound"));
                        }
                    }
                }
                let (new_state, _) = self.states(fake);
                Ok(json!({ "accountId": ACCOUNT, "oldState": email_state, "newState": new_state,
                    "updated": updated, "notUpdated": not_updated, "destroyed": destroyed, "notDestroyed": not_destroyed }))
            }
            "Email/import" => {
                let mut created = Map::new();
                let mut not_created = Map::new();
                for (k, e) in args.get("emails").and_then(Value::as_object).into_iter().flatten() {
                    let blob = e.get("blobId").and_then(Value::as_str).unwrap_or_default();
                    let target = e.get("mailboxIds").and_then(Value::as_object).and_then(|m| m.keys().next().cloned());
                    let (Some(raw), Some(mb)) = (self.blob(fake, blob), target.and_then(|t| self.name_of(&t))) else {
                        not_created.insert(k.clone(), error("invalidProperties"));
                        continue;
                    };
                    let kw: BTreeSet<String> = e
                        .get("keywords")
                        .and_then(Value::as_object)
                        .map(|m| m.keys().cloned().collect())
                        .unwrap_or_default();
                    let f = flags(&kw);
                    let mut list: Vec<&str> = vec![];
                    if f.seen {
                        list.push(SEEN);
                    }
                    if f.draft {
                        list.push(DRAFT);
                    }
                    if f.flagged {
                        list.push(FLAGGED);
                    }
                    let uid = match fake.do_append(&mb, &list, &raw) {
                        Ok(Some(uid)) => uid,
                        _ => fake.mailboxes[&mb].messages.keys().last().copied().unwrap_or(0),
                    };
                    let id = self.email_id(&mb, uid);
                    created.insert(k.clone(), json!({ "id": id, "blobId": format!("b:{id}"), "size": raw.len() }));
                }
                let (new_state, _) = self.states(fake);
                Ok(json!({ "accountId": ACCOUNT, "oldState": email_state, "newState": new_state,
                    "created": created, "notCreated": not_created }))
            }
            "Identity/get" => {
                let mut list = vec![json!({ "id": "i1", "name": "", "email": self.user, "mayDelete": false })];
                for (n, a) in self.aliases.iter().enumerate() {
                    list.push(json!({ "id": format!("i{}", n + 2), "name": "", "email": a, "mayDelete": true }));
                }
                Ok(json!({ "accountId": ACCOUNT, "state": "1", "list": list, "notFound": [] }))
            }
            "EmailSubmission/set" => {
                let mut created = Map::new();
                let mut not_created = Map::new();
                let mut done: Vec<(String, String)> = vec![];
                for (k, s) in args.get("create").and_then(Value::as_object).into_iter().flatten() {
                    let email = s.get("emailId").and_then(Value::as_str).unwrap_or_default().to_string();
                    let Some((mb, uid)) = self.locate(fake, &email) else {
                        not_created.insert(k.clone(), error("invalidProperties"));
                        continue;
                    };
                    let from =
                        s.pointer("/envelope/mailFrom/email").and_then(Value::as_str).unwrap_or_default().to_string();
                    if !from.eq_ignore_ascii_case(&self.user) {
                        not_created
                            .insert(k.clone(), json!({ "type": "forbiddenFrom", "description": "not your address" }));
                        continue;
                    }
                    let to: Vec<String> = s
                        .pointer("/envelope/rcptTo")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|r| r.get("email").and_then(Value::as_str).map(String::from))
                        .collect();
                    self.submitted.push(Submitted { from, to, raw: fake.mailboxes[&mb].messages[&uid].raw.clone() });
                    created.insert(
                        k.clone(),
                        json!({ "id": format!("s{}", self.submitted.len()), "undoStatus": "final" }),
                    );
                    done.push((format!("#{k}"), email));
                }
                for (key, email) in done {
                    if let Some(patch) = args.pointer(&format!("/onSuccessUpdateEmail/{}", key.replace('/', "~1"))) {
                        let _ = self.patch(fake, &email, patch);
                    }
                }
                Ok(json!({ "accountId": ACCOUNT, "created": created, "notCreated": not_created }))
            }
            _ => Err(error("unknownMethod")),
        }
    }

    /// An `Email/set` update: keywords and folders, whole or by path.
    fn patch(&mut self, fake: &mut FakeImap, id: &str, patch: &Value) -> Result<(), Value> {
        let (mb, uid) = self.locate(fake, id).ok_or_else(|| error("notFound"))?;
        let msg = &fake.mailboxes[&mb].messages[&uid];
        let mut kw: BTreeSet<String> = keywords(&msg.flags).into_iter().collect();
        let mut boxes: BTreeSet<String> = [mb.clone()].into_iter().collect();
        for (k, v) in patch.as_object().into_iter().flatten() {
            if k == "keywords" {
                kw = v.as_object().map(|m| m.keys().map(|x| x.to_ascii_lowercase()).collect()).unwrap_or_default();
            } else if let Some(word) = k.strip_prefix("keywords/") {
                if v.as_bool() == Some(true) {
                    kw.insert(word.to_ascii_lowercase());
                } else {
                    kw.remove(&word.to_ascii_lowercase());
                }
            } else if k == "mailboxIds" {
                boxes = v.as_object().map(|m| m.keys().filter_map(|i| self.name_of(i)).collect()).unwrap_or_default();
            } else if let Some(b) = k.strip_prefix("mailboxIds/") {
                let name = self.name_of(b).ok_or_else(|| error("invalidProperties"))?;
                if v.as_bool() == Some(true) {
                    boxes.insert(name);
                } else {
                    boxes.remove(&name);
                }
            }
        }
        if boxes.is_empty() {
            return Err(error("invalidProperties"));
        }
        fake.set_flags(&mb, uid, flags(&kw));
        fake.selected = Some(mb.clone());
        let others: Vec<String> = boxes.iter().filter(|b| **b != mb).cloned().collect();
        let keep = boxes.contains(&mb);
        for (i, to) in others.iter().enumerate() {
            let last = i + 1 == others.len();
            fake.selected = Some(mb.clone());
            fake.do_copy(&[uid], to, last && !keep).map_err(|_| error("serverFail"))?;
        }
        Ok(())
    }

    fn mailbox_set(&mut self, fake: &mut FakeImap, args: &Value, old_state: &str) -> Result<Value, Value> {
        let mut created = Map::new();
        let mut not_created = Map::new();
        for (k, m) in args.get("create").and_then(Value::as_object).into_iter().flatten() {
            let leaf = m.get("name").and_then(Value::as_str).unwrap_or_default();
            let parent = m.get("parentId").and_then(Value::as_str).and_then(|p| self.name_of(p));
            let full = match parent {
                Some(p) => format!("{p}/{leaf}"),
                None => leaf.to_string(),
            };
            match fake.do_create(&full) {
                Ok(()) => {
                    self.sync_ids(fake);
                    created.insert(k.clone(), json!({ "id": self.ids[&full] }));
                }
                Err(_) => {
                    not_created.insert(k.clone(), error("invalidProperties"));
                }
            }
        }
        let mut updated = Map::new();
        let mut not_updated = Map::new();
        for (id, m) in args.get("update").and_then(Value::as_object).into_iter().flatten() {
            let Some(old) = self.name_of(id) else {
                not_updated.insert(id.clone(), error("notFound"));
                continue;
            };
            let leaf = m
                .get("name")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| old.rsplit_once('/').map_or(old.clone(), |(_, l)| l.to_string()));
            let parent = match m.get("parentId") {
                Some(Value::String(p)) => self.name_of(p),
                Some(_) => None,
                None => old.rsplit_once('/').map(|(p, _)| p.to_string()),
            };
            let full = match parent {
                Some(p) => format!("{p}/{leaf}"),
                None => leaf,
            };
            match fake.do_rename(&old, &full) {
                Ok(()) => {
                    self.ids.remove(&old);
                    self.ids.insert(full, id.clone());
                    updated.insert(id.clone(), Value::Null);
                }
                Err(_) => {
                    not_updated.insert(id.clone(), error("invalidProperties"));
                }
            }
        }
        let mut destroyed = vec![];
        let mut not_destroyed = Map::new();
        for id in ids_of(args.get("destroy")) {
            match self.name_of(&id).map(|n| fake.do_delete(&n)) {
                Some(Ok(())) => destroyed.push(id),
                _ => {
                    not_destroyed.insert(id, error("notFound"));
                }
            }
        }
        let (_, new_state) = self.states(fake);
        Ok(json!({ "accountId": ACCOUNT, "oldState": old_state, "newState": new_state,
            "created": created, "notCreated": not_created, "updated": updated, "notUpdated": not_updated,
            "destroyed": destroyed, "notDestroyed": not_destroyed }))
    }
}

fn ids_of(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// Whether a request signs in as `user` with `password` (Basic) or with `password` as the
/// token (Bearer).
pub fn authorized(headers: &[(String, String)], user: &str, password: &str) -> bool {
    let Some((_, v)) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")) else { return false };
    if let Some(t) = v.strip_prefix("Bearer ") {
        return t.trim() == password;
    }
    let Some(b) = v.strip_prefix("Basic ") else { return false };
    let decoded = base64::engine::general_purpose::STANDARD.decode(b.trim()).unwrap_or_default();
    let text = String::from_utf8_lossy(&decoded);
    text.split_once(':').is_some_and(|(u, p)| u.eq_ignore_ascii_case(user) && p == password)
}
