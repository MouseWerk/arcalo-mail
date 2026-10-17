//! A Microsoft account's calendars over Graph (delegated permission Calendars.ReadWrite):
//! `/me/calendars`, each calendar's appointments in a window through `calendarView/delta`
//! (instances of series come one by one, so nothing is expanded here; the delta link reads only
//! what changed), writes with the event's ETag (`If-Match`; `412` is a conflict), and the
//! answers to invitations (`accept`, `tentativelyAccept`, `decline`). Times are asked for in UTC
//! (`Prefer: outlook.timezone="UTC"`).

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use serde_json::{Value, json};

use super::tz::Zone;
use super::{Busy, Editability, Fields, Occurrence, meeting_link};
use crate::error::{Error, Result};
use crate::graph::Graph;
use crate::{tr, trf};

const PREFER: (&str, &str) = ("Prefer", "outlook.timezone=\"UTC\", odata.maxpagesize=200");

fn parse_time(v: &Value) -> Option<(NaiveDateTime, Zone)> {
    let t = v["dateTime"].as_str()?;
    let t = t.split('.').next().unwrap_or(t);
    let wall = NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S").ok()?;
    let zone = v["timeZone"].as_str().and_then(Zone::named).unwrap_or(Zone::Utc);
    Some((wall, zone))
}

fn person(v: &Value) -> String {
    let name = v["emailAddress"]["name"].as_str().unwrap_or_default().trim();
    let addr = v["emailAddress"]["address"].as_str().unwrap_or_default().trim();
    match (name.is_empty() || name == addr, addr.is_empty()) {
        (false, false) => format!("{name} <{addr}>"),
        (true, false) => addr.to_string(),
        _ => name.to_string(),
    }
}

/// The appointment of a Graph event (one instance; series come expanded).
pub fn occurrence(data: &str) -> Option<Occurrence> {
    let v: Value = serde_json::from_str(data).ok()?;
    if v["isCancelled"].as_bool() == Some(true) {
        return None;
    }
    let all_day = v["isAllDay"].as_bool().unwrap_or(false);
    let (s_wall, s_zone) = parse_time(&v["start"])?;
    let (e_wall, e_zone) = parse_time(&v["end"]).unwrap_or((s_wall, s_zone.clone()));
    let (start, end) = if all_day {
        // Days, placed at the device's midnights.
        let a = Zone::Local.to_utc(s_wall.date().and_hms_opt(0, 0, 0)?);
        let b = Zone::Local.to_utc(e_wall.date().and_hms_opt(0, 0, 0)?);
        (a, b.max(a + Duration::days(1)))
    } else {
        let a = s_zone.to_utc(s_wall);
        (a, e_zone.to_utc(e_wall).max(a))
    };
    let location = v["location"]["displayName"].as_str().unwrap_or_default().trim().to_string();
    let body_html = v["body"]["content"].as_str().unwrap_or_default();
    let body = if v["body"]["contentType"].as_str() == Some("html") {
        crate::compose::html_to_text(body_html)
    } else {
        body_html.to_string()
    };
    let body = Some(body.trim().to_string())
        .filter(|b| !b.is_empty())
        .or_else(|| v["bodyPreview"].as_str().map(|b| b.trim().to_string()).filter(|b| !b.is_empty()));
    let join = v["onlineMeeting"]["joinUrl"].as_str().or(v["onlineMeetingUrl"].as_str()).unwrap_or_default();
    let link = meeting_link([join, location.as_str(), body.as_deref().unwrap_or(""), body_html]);
    let kind = v["type"].as_str().unwrap_or("singleInstance");
    let attendees = v["attendees"]
        .as_array()
        .map(|a| {
            a.iter().filter(|x| x["type"].as_str() != Some("resource")).map(person).filter(|p| !p.is_empty()).collect()
        })
        .unwrap_or_default();
    let title = v["subject"].as_str().unwrap_or_default().trim().to_string();
    Some(Occurrence {
        uid: v["iCalUId"].as_str().unwrap_or_default().to_string(),
        instance: String::new(),
        recurring: kind == "occurrence" || kind == "exception",
        start,
        end,
        all_day,
        title: if title.is_empty() { tr!("Ohne Titel", "No title").to_string() } else { title },
        location,
        organizer: if v["organizer"].is_object() { person(&v["organizer"]) } else { String::new() },
        attendees,
        body,
        link,
        busy: match v["showAs"].as_str().unwrap_or("busy") {
            "free" => Busy::Free,
            "tentative" => Busy::Tentative,
            "oof" => Busy::Oof,
            "workingElsewhere" => Busy::Elsewhere,
            _ => Busy::Busy,
        },
        private: matches!(v["sensitivity"].as_str(), Some("private" | "confidential")),
        categories: v["categories"]
            .as_array()
            .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        tzid: None,
    })
}

/// How a Graph event can be changed here.
pub fn editability(data: &str) -> Editability {
    let v: Value = serde_json::from_str(data).unwrap_or_default();
    if matches!(v["type"].as_str(), Some("occurrence" | "exception" | "seriesMaster")) {
        return Editability::Series;
    }
    if v["isOrganizer"].as_bool() == Some(false) {
        return Editability::Received;
    }
    let others = v["attendees"].as_array().is_some_and(|a| a.iter().any(|x| x["type"].as_str() != Some("resource")));
    if others { Editability::Confirm } else { Editability::Yes }
}

/// `{dateTime, timeZone}` of an instant in UTC, or of a day (all-day appointments in the
/// device's zone).
fn time_json(t: DateTime<Utc>, all_day: bool) -> Value {
    if all_day {
        let day = Zone::Local.to_wall(t).date();
        let zone = super::tz::local_name().unwrap_or_else(|| "UTC".into());
        json!({ "dateTime": format!("{}T00:00:00", day.format("%Y-%m-%d")), "timeZone": zone })
    } else {
        json!({ "dateTime": t.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC" })
    }
}

/// The JSON of an appointment made here, before Graph has it (Graph gives it its id).
pub fn new_event_json(uid: &str, f: &Fields) -> String {
    let start = f.start.unwrap_or_else(Utc::now);
    let all_day = f.all_day.unwrap_or(false);
    let end = f.end.unwrap_or(start + Duration::minutes(30));
    let end = if all_day { end.max(start + Duration::days(1)) } else { end.max(start) };
    json!({
        "transactionId": uid,
        "iCalUId": uid,
        "subject": f.title.clone().unwrap_or_default(),
        "start": time_json(start, all_day),
        "end": time_json(end, all_day),
        "isAllDay": all_day,
        "location": { "displayName": f.location.clone().unwrap_or_default() },
        "body": { "contentType": "text", "content": f.description.clone().unwrap_or_default() },
        "isOrganizer": true,
        "type": "singleInstance",
        "showAs": "busy",
    })
    .to_string()
}

/// The event's JSON with `f` applied.
pub fn edit_json(data: &str, f: &Fields) -> Option<String> {
    let mut v: Value = serde_json::from_str(data).ok()?;
    let o = occurrence(data)?;
    if let Some(t) = &f.title {
        v["subject"] = json!(t.trim());
    }
    if let Some(l) = &f.location {
        v["location"] = json!({ "displayName": l.trim() });
    }
    if let Some(d) = &f.description {
        v["body"] = json!({ "contentType": "text", "content": d.trim() });
    }
    if f.start.is_some() || f.end.is_some() || f.all_day.is_some() {
        let all_day = f.all_day.unwrap_or(o.all_day);
        let start = f.start.unwrap_or(o.start);
        let end = f.end.unwrap_or(start + (o.end - o.start));
        let end = if all_day { end.max(start + Duration::days(1)) } else { end.max(start) };
        v["isAllDay"] = json!(all_day);
        v["start"] = time_json(start, all_day);
        v["end"] = time_json(end, all_day);
    }
    Some(v.to_string())
}

/// What a write sends: the fields Graph takes.
pub fn patch_body(data: &str) -> Value {
    let v: Value = serde_json::from_str(data).unwrap_or_default();
    let mut out = json!({
        "subject": v["subject"],
        "start": v["start"],
        "end": v["end"],
        "isAllDay": v["isAllDay"],
        "location": { "displayName": v["location"]["displayName"].as_str().unwrap_or_default() },
    });
    if v["body"]["contentType"].as_str() == Some("text") && v["id"].is_null() {
        out["body"] = v["body"].clone();
    }
    out
}

/// A calendar of the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCalendar {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub writable: bool,
    pub default: bool,
}

/// The state of a calendar's delta: the link and the window it covers (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeltaState {
    pub link: String,
    pub from: i64,
    pub to: i64,
}

/// What a delta round brought.
#[derive(Debug, Clone, Default)]
pub struct Delta {
    /// The window was read anew: objects not in `changed` are gone.
    pub full: bool,
    pub changed: Vec<Value>,
    pub removed: Vec<String>,
    pub state: Option<DeltaState>,
}

/// The window read: two months back, a year ahead; read anew when its end comes within half a
/// year.
fn window(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    (now - Duration::days(62), now + Duration::days(366))
}

fn graph_error(status: u16, bytes: &[u8]) -> Error {
    let v: Value = serde_json::from_slice(bytes).unwrap_or_default();
    let msg = v["error"]["message"].as_str().unwrap_or_default().to_string();
    Error::Network(trf!("Microsoft Graph meldet {status}: {msg}", "Microsoft Graph reports {status}: {msg}"))
}

/// The answers a write can get.
#[derive(Debug, Clone, PartialEq)]
pub enum Written {
    /// The event as Graph has it now (`None` after a delete).
    Done(Option<Value>),
    Conflict,
}

/// The account's calendars.
pub async fn calendars(g: &Graph) -> Result<Vec<GraphCalendar>> {
    let mut out = vec![];
    let mut next = Some("/me/calendars?$top=100&$select=id,name,hexColor,canEdit,isDefaultCalendar".to_string());
    while let Some(path) = next.take() {
        let (status, bytes) = g.exchange(reqwest::Method::GET, &path, None, &[]).await?;
        if status >= 400 {
            return Err(graph_error(status, &bytes));
        }
        let v: Value = serde_json::from_slice(&bytes)?;
        for c in v["value"].as_array().cloned().unwrap_or_default() {
            let Some(id) = c["id"].as_str() else { continue };
            out.push(GraphCalendar {
                id: id.to_string(),
                name: c["name"].as_str().unwrap_or_default().to_string(),
                color: c["hexColor"].as_str().filter(|h| !h.is_empty()).map(str::to_owned),
                writable: c["canEdit"].as_bool().unwrap_or(true),
                default: c["isDefaultCalendar"].as_bool().unwrap_or(false),
            });
        }
        next = v["@odata.nextLink"].as_str().map(str::to_owned);
    }
    Ok(out)
}

/// What changed in a calendar since `state` (all of the window when there is none, it expired
/// or the window moved on).
pub async fn delta(g: &Graph, calendar_id: &str, state: Option<&DeltaState>, now: DateTime<Utc>) -> Result<Delta> {
    let (from, to) = window(now);
    let fresh = |s: &&DeltaState| s.to - now.timestamp() > 183 * 86_400;
    let mut full = state.filter(fresh).is_none();
    let first = || {
        format!(
            "/me/calendars/{calendar_id}/calendarView/delta?startDateTime={}&endDateTime={}",
            from.format("%Y-%m-%dT%H:%M:%SZ"),
            to.format("%Y-%m-%dT%H:%M:%SZ")
        )
    };
    let mut next = Some(match state.filter(fresh) {
        Some(s) => s.link.clone(),
        None => first(),
    });
    let mut out = Delta { full, ..Delta::default() };
    let mut window_of = state.filter(fresh).map(|s| (s.from, s.to)).unwrap_or((from.timestamp(), to.timestamp()));
    while let Some(path) = next.take() {
        let (status, bytes) = g.exchange(reqwest::Method::GET, &path, None, &[PREFER]).await?;
        if status == 410 && !full {
            // The link expired: the whole window anew.
            full = true;
            out = Delta { full, ..Delta::default() };
            window_of = (from.timestamp(), to.timestamp());
            next = Some(first());
            continue;
        }
        if status >= 400 {
            return Err(graph_error(status, &bytes));
        }
        let v: Value = serde_json::from_slice(&bytes)?;
        for e in v["value"].as_array().cloned().unwrap_or_default() {
            let Some(id) = e["id"].as_str() else { continue };
            if e.get("@removed").is_some() {
                out.removed.push(id.to_string());
            } else {
                out.changed.push(e);
            }
        }
        if let Some(n) = v["@odata.nextLink"].as_str() {
            next = Some(n.to_string());
        } else if let Some(d) = v["@odata.deltaLink"].as_str() {
            out.state = Some(DeltaState { link: d.to_string(), from: window_of.0, to: window_of.1 });
        }
    }
    Ok(out)
}

/// The event's ETag (`@odata.etag`) or change key.
pub fn etag_of(v: &Value) -> Option<String> {
    v["@odata.etag"].as_str().or(v["changeKey"].as_str()).map(str::to_owned)
}

/// Creates an event in a calendar.
pub async fn create(g: &Graph, calendar_id: &str, data: &str) -> Result<Value> {
    let mut body = patch_body(data);
    let v: Value = serde_json::from_str(data).unwrap_or_default();
    if let Some(t) = v["transactionId"].as_str() {
        body["transactionId"] = json!(t);
    }
    let (status, bytes) = g
        .exchange(reqwest::Method::POST, &format!("/me/calendars/{calendar_id}/events"), Some(&body), &[PREFER])
        .await?;
    if status >= 400 {
        return Err(graph_error(status, &bytes));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Changes an event (the version with `etag`).
pub async fn update(g: &Graph, id: &str, data: &str, etag: Option<&str>) -> Result<Written> {
    let body = patch_body(data);
    let mut headers = vec![PREFER];
    if let Some(e) = etag {
        headers.push(("If-Match", e));
    }
    let (status, bytes) =
        g.exchange(reqwest::Method::PATCH, &format!("/me/events/{id}"), Some(&body), &headers).await?;
    match status {
        412 | 409 => Ok(Written::Conflict),
        404 => Ok(Written::Conflict),
        s if s >= 400 => Err(graph_error(s, &bytes)),
        _ => Ok(Written::Done(serde_json::from_slice(&bytes).ok())),
    }
}

/// Deletes an event (the version with `etag`).
pub async fn delete(g: &Graph, id: &str, etag: Option<&str>) -> Result<Written> {
    let headers: Vec<(&str, &str)> = etag.map(|e| vec![("If-Match", e)]).unwrap_or_default();
    let (status, bytes) = g.exchange(reqwest::Method::DELETE, &format!("/me/events/{id}"), None, &headers).await?;
    match status {
        412 | 409 => Ok(Written::Conflict),
        404 | 410 => Ok(Written::Done(None)),
        s if s >= 400 => Err(graph_error(s, &bytes)),
        _ => Ok(Written::Done(None)),
    }
}

/// One event (`None`: gone).
pub async fn get(g: &Graph, id: &str) -> Result<Option<Value>> {
    let (status, bytes) = g.exchange(reqwest::Method::GET, &format!("/me/events/{id}"), None, &[PREFER]).await?;
    match status {
        404 | 410 => Ok(None),
        s if s >= 400 => Err(graph_error(s, &bytes)),
        _ => Ok(Some(serde_json::from_slice(&bytes)?)),
    }
}

/// The event an invitation is about, by its iCalendar UID.
pub async fn find_by_uid(g: &Graph, uid: &str) -> Result<Option<Value>> {
    let filter: String = format!("iCalUId eq '{}'", uid.replace('\'', "''"))
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let (status, bytes) =
        g.exchange(reqwest::Method::GET, &format!("/me/events?$filter={filter}&$top=1"), None, &[PREFER]).await?;
    if status >= 400 {
        return Err(graph_error(status, &bytes));
    }
    let v: Value = serde_json::from_slice(&bytes)?;
    Ok(v["value"].as_array().and_then(|a| a.first().cloned()))
}

/// Answers an invitation: `accept`, `tentativelyAccept` or `decline`; Graph tells the organizer.
pub async fn respond(g: &Graph, id: &str, how: &str, comment: &str) -> Result<()> {
    let body = json!({ "comment": comment, "sendResponse": true });
    let (status, bytes) =
        g.exchange(reqwest::Method::POST, &format!("/me/events/{id}/{how}"), Some(&body), &[]).await?;
    if status >= 400 {
        return Err(graph_error(status, &bytes));
    }
    Ok(())
}

/// A day as Graph's all-day value (`2026-10-20T00:00:00`), for tests.
pub fn day_value(d: NaiveDate) -> String {
    format!("{}T00:00:00", d.format("%Y-%m-%d"))
}

#[cfg(any(test, feature = "fake"))]
pub mod fake {
    //! Graph's calendar endpoints as far as the app uses them, without sockets: one calendar
    //! `C1` („Kalender“) and a read-only one `C2`; events by id with a change key; a delta link
    //! per calendar that returns what changed since it was handed out.

    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    #[derive(Debug, Clone, Default)]
    pub struct FakeGraphCalendar {
        /// Event id → (calendar id, event).
        pub events: BTreeMap<String, (String, Value)>,
        /// What changed at which round: (round, calendar, id, removed).
        pub log: Vec<(u64, String, String, bool)>,
        pub round: u64,
        next: u64,
        /// Answers given: (method, path).
        pub requests: Vec<(String, String)>,
        /// Invitations answered: (event id, how).
        pub responses: Vec<(String, String)>,
    }

    impl FakeGraphCalendar {
        /// An event put there as another client (or Exchange) would; returns its id.
        pub fn put(&mut self, calendar: &str, mut event: Value) -> String {
            self.next += 1;
            self.round += 1;
            let id = event["id"].as_str().map(str::to_owned).unwrap_or_else(|| format!("E{}", self.next));
            event["id"] = json!(id);
            event["changeKey"] = json!(format!("ck{}", self.next));
            event["@odata.etag"] = json!(format!("W/\"ck{}\"", self.next));
            if event["iCalUId"].is_null() {
                event["iCalUId"] = json!(format!("uid-{id}"));
            }
            if event["type"].is_null() {
                event["type"] = json!("singleInstance");
            }
            if event["isOrganizer"].is_null() {
                event["isOrganizer"] = json!(true);
            }
            self.events.insert(id.clone(), (calendar.to_string(), event));
            self.log.push((self.round, calendar.to_string(), id.clone(), false));
            id
        }

        pub fn remove(&mut self, id: &str) {
            if let Some((cal, _)) = self.events.remove(id) {
                self.round += 1;
                self.log.push((self.round, cal, id.to_string(), true));
            }
        }

        /// Answers a request below `/v1.0`: status and JSON. `base`: the server's address for
        /// the links it hands out.
        pub fn handle(
            &mut self,
            method: &str,
            target: &str,
            headers: &[(String, String)],
            body: &[u8],
            base: &str,
        ) -> (u16, String) {
            self.requests.push((method.to_string(), target.to_string()));
            let path = target.split('?').next().unwrap_or_default().trim_start_matches("/v1.0").to_string();
            let query = target.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
            let header = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
            let body: Value = serde_json::from_slice(body).unwrap_or_default();
            let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            match (method, parts.as_slice()) {
                ("GET", ["me", "calendars"]) => (
                    200,
                    json!({"value": [
                        {"id": "C1", "name": "Kalender", "hexColor": "#2563eb", "canEdit": true, "isDefaultCalendar": true},
                        {"id": "C2", "name": "Feiertage", "hexColor": "", "canEdit": false, "isDefaultCalendar": false}
                    ]})
                    .to_string(),
                ),
                ("GET", ["me", "calendars", cal, "calendarView", "delta"]) => {
                    let since = query
                        .split('&')
                        .find_map(|kv| kv.strip_prefix("since="))
                        .and_then(|v| v.parse::<u64>().ok());
                    let mut value = vec![];
                    match since {
                        None => {
                            for (id, (c, e)) in &self.events {
                                if c == cal {
                                    let _ = id;
                                    value.push(e.clone());
                                }
                            }
                        }
                        Some(s) => {
                            let mut seen = std::collections::BTreeSet::new();
                            for (round, c, id, _) in self.log.iter().rev() {
                                if *round <= s || c != cal || !seen.insert(id.clone()) {
                                    continue;
                                }
                                match self.events.get(id) {
                                    Some((_, e)) => value.push(e.clone()),
                                    None => value.push(json!({"id": id, "@removed": {"reason": "deleted"}})),
                                }
                            }
                        }
                    }
                    let link = format!("{base}/me/calendars/{cal}/calendarView/delta?since={}", self.round);
                    (200, json!({"value": value, "@odata.deltaLink": link}).to_string())
                }
                ("POST", ["me", "calendars", cal, "events"]) => {
                    if *cal == "C2" {
                        return (403, json!({"error": {"message": "read only"}}).to_string());
                    }
                    let mut e = body.clone();
                    e.as_object_mut().map(|o| o.remove("transactionId"));
                    let id = self.put(cal, e);
                    (201, self.events[&id].1.to_string())
                }
                ("GET", ["me", "events"]) => {
                    let uid = query.split("iCalUId%20eq%20%27").nth(1).and_then(|r| r.split("%27").next()).unwrap_or_default();
                    let found: Vec<Value> =
                        self.events.values().filter(|(_, e)| e["iCalUId"].as_str() == Some(uid)).map(|(_, e)| e.clone()).collect();
                    (200, json!({"value": found}).to_string())
                }
                ("GET", ["me", "events", id]) => match self.events.get(*id) {
                    Some((_, e)) => (200, e.to_string()),
                    None => (404, json!({"error": {"message": "not found"}}).to_string()),
                },
                ("PATCH", ["me", "events", id]) => {
                    let Some((cal, current)) = self.events.get(*id).cloned() else {
                        return (404, json!({"error": {"message": "not found"}}).to_string());
                    };
                    if header("if-match").is_some_and(|m| Some(m.as_str()) != current["@odata.etag"].as_str()) {
                        return (412, json!({"error": {"message": "precondition failed"}}).to_string());
                    }
                    let mut e = current.clone();
                    for (k, v) in body.as_object().cloned().unwrap_or_default() {
                        e[k] = v;
                    }
                    let id = self.put(&cal, e);
                    (200, self.events[&id].1.to_string())
                }
                ("DELETE", ["me", "events", id]) => {
                    let Some((_, current)) = self.events.get(*id).cloned() else { return (404, String::new()) };
                    if header("if-match").is_some_and(|m| Some(m.as_str()) != current["@odata.etag"].as_str()) {
                        return (412, json!({"error": {"message": "precondition failed"}}).to_string());
                    }
                    self.remove(id);
                    (204, String::new())
                }
                ("POST", ["me", "events", id, how]) => {
                    if !self.events.contains_key(*id) {
                        return (404, json!({"error": {"message": "not found"}}).to_string());
                    }
                    self.responses.push((id.to_string(), how.to_string()));
                    let (cal, mut e) = self.events[*id].clone();
                    e["responseStatus"] = json!({"response": match *how { "accept" => "accepted", "decline" => "declined", _ => "tentativelyAccepted" }});
                    if *how == "decline" {
                        self.remove(id);
                    } else {
                        self.put(&cal, e);
                    }
                    (202, String::new())
                }
                _ => (404, json!({"error": {"message": "unknown"}}).to_string()),
            }
        }
    }
}
