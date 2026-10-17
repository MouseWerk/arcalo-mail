//! A JMAP account's calendars, where the server offers them (`urn:ietf:params:jmap:calendars`,
//! the JMAP for Calendars draft): `Calendar/get`, the events of each calendar
//! (`CalendarEvent/query` and `get`, then `CalendarEvent/changes` from the state), and writes
//! with `CalendarEvent/set`. Events are JSCalendar (RFC 8984); they are stored as iCalendar
//! ([`to_ics`]) so series expand and times are placed as for every other source. JMAP has no
//! per-event ETag: a write goes over what the server has (the next sync shows the result).

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::{Map, Value, json};

use super::ics::{self, Component, Prop, escape};
use super::tz::Zone;
use crate::error::{Error, Result};
use crate::jmap::{CORE, JmapClient};

pub const CALENDARS: &str = "urn:ietf:params:jmap:calendars";

/// A calendar of the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JmapCalendar {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub writable: bool,
}

/// `2026-10-20T10:00:00` (JSCalendar's LocalDateTime) as iCalendar's `20261020T100000`.
fn local_text(v: &str) -> Option<String> {
    let t = NaiveDateTime::parse_from_str(v.trim().trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(t.format("%Y%m%dT%H%M%S").to_string())
}

/// A JSCalendar recurrence rule as an `RRULE` value.
fn rrule(r: &Value) -> Option<String> {
    let freq = r["frequency"].as_str()?.to_ascii_uppercase();
    let mut parts = vec![format!("FREQ={freq}")];
    if let Some(i) = r["interval"].as_u64().filter(|i| *i > 1) {
        parts.push(format!("INTERVAL={i}"));
    }
    if let Some(c) = r["count"].as_u64() {
        parts.push(format!("COUNT={c}"));
    }
    if let Some(u) = r["until"].as_str().and_then(local_text) {
        parts.push(format!("UNTIL={u}"));
    }
    if let Some(days) = r["byDay"].as_array().filter(|d| !d.is_empty()) {
        let list: Vec<String> = days
            .iter()
            .filter_map(|d| {
                let day = d["day"].as_str()?.to_ascii_uppercase();
                Some(match d["nthOfPeriod"].as_i64() {
                    Some(n) => format!("{n}{day}"),
                    None => day,
                })
            })
            .collect();
        parts.push(format!("BYDAY={}", list.join(",")));
    }
    for (key, name) in [("byMonthDay", "BYMONTHDAY"), ("byMonth", "BYMONTH"), ("bySetPosition", "BYSETPOS")] {
        if let Some(list) = r[key].as_array().filter(|l| !l.is_empty()) {
            let vals: Vec<String> = list
                .iter()
                .map(|x| {
                    x.as_i64().map(|n| n.to_string()).or_else(|| x.as_str().map(str::to_owned)).unwrap_or_default()
                })
                .collect();
            parts.push(format!("{name}={}", vals.join(",")));
        }
    }
    Some(parts.join(";"))
}

/// Start and end properties of a JSCalendar object (or of an override's patch over its base).
fn put_times(c: &mut Component, start: &str, zone: Option<&str>, all_day: bool, duration: Option<&str>) {
    let Some(local) = local_text(start) else { return };
    if all_day {
        c.set(Prop::new("DTSTART", &local[..8]).with("VALUE", "DATE"));
    } else {
        match zone {
            Some(z) if z.eq_ignore_ascii_case("Etc/UTC") || z == "UTC" => {
                c.set(Prop::new("DTSTART", &format!("{local}Z")))
            }
            Some(z) => c.set(Prop::new("DTSTART", &local).with("TZID", z)),
            None => c.set(Prop::new("DTSTART", &local)),
        }
    }
    if let Some(d) = duration {
        c.set(Prop::new("DURATION", d));
    }
}

fn participant(p: &Value) -> Option<(String, Prop)> {
    let email = p["email"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| p["sendTo"]["imip"].as_str().map(|s| s.trim_start_matches("mailto:").to_string()))?;
    let roles = &p["roles"];
    let kind = if roles["owner"].as_bool() == Some(true) { "ORGANIZER" } else { "ATTENDEE" };
    let mut prop = Prop::new(kind, &format!("mailto:{email}"));
    if let Some(n) = p["name"].as_str().filter(|n| !n.is_empty()) {
        prop = prop.with("CN", n);
    }
    if kind == "ATTENDEE"
        && let Some(s) = p["participationStatus"].as_str()
    {
        prop = prop.with("PARTSTAT", &s.to_ascii_uppercase());
    }
    Some((kind.to_string(), prop))
}

/// A JSCalendar event as an iCalendar object (`None` without a start).
pub fn to_ics(e: &Value) -> Option<String> {
    let uid = e["uid"].as_str().or(e["id"].as_str())?.to_string();
    let start = e["start"].as_str()?;
    let zone = e["timeZone"].as_str();
    let all_day = e["showWithoutTime"].as_bool().unwrap_or(false);
    let mut ev = Component::new("VEVENT");
    ev.props.push(Prop::new("UID", &uid));
    put_times(
        &mut ev,
        start,
        zone,
        all_day,
        Some(e["duration"].as_str().unwrap_or(if all_day { "P1D" } else { "PT0S" })),
    );
    ev.props.push(Prop::new("SUMMARY", &escape(e["title"].as_str().unwrap_or_default())));
    if let Some(d) = e["description"].as_str().filter(|d| !d.is_empty()) {
        ev.props.push(Prop::new("DESCRIPTION", &escape(d)));
    }
    if let Some(l) = e["locations"].as_object().and_then(|m| m.values().find_map(|l| l["name"].as_str())) {
        ev.props.push(Prop::new("LOCATION", &escape(l)));
    }
    if let Some(u) = e["virtualLocations"].as_object().and_then(|m| m.values().find_map(|l| l["uri"].as_str())) {
        ev.props.push(Prop::new("URL", u));
    }
    if let Some(ps) = e["participants"].as_object() {
        let mut list: Vec<(String, Prop)> = ps.values().filter_map(participant).collect();
        list.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in list {
            ev.props.push(p);
        }
    }
    if e["privacy"].as_str().is_some_and(|p| p != "public") {
        ev.props.push(Prop::new("CLASS", "PRIVATE"));
    }
    if e["freeBusyStatus"].as_str() == Some("free") {
        ev.props.push(Prop::new("TRANSP", "TRANSPARENT"));
    }
    if let Some(n) = e["sequence"].as_u64() {
        ev.props.push(Prop::new("SEQUENCE", &n.to_string()));
    }
    if let Some(rules) = e["recurrenceRules"].as_array() {
        for r in rules.iter().filter_map(rrule) {
            ev.props.push(Prop::new("RRULE", &r));
        }
    }
    let mut overrides = vec![];
    if let Some(o) = e["recurrenceOverrides"].as_object() {
        for (at, patch) in o {
            let Some(local) = local_text(at) else { continue };
            let rid = |name: &str| {
                if all_day {
                    Prop::new(name, &local[..8]).with("VALUE", "DATE")
                } else {
                    match zone {
                        Some(z) => Prop::new(name, &local).with("TZID", z),
                        None => Prop::new(name, &local),
                    }
                }
            };
            if patch["excluded"].as_bool() == Some(true) {
                ev.props.push(rid("EXDATE"));
                continue;
            }
            let mut o = Component::new("VEVENT");
            o.props.push(Prop::new("UID", &uid));
            o.props.push(rid("RECURRENCE-ID"));
            let start = patch["start"].as_str().unwrap_or(at);
            let duration = patch["duration"].as_str().or(e["duration"].as_str());
            put_times(&mut o, start, zone, all_day, duration);
            let title = patch["title"].as_str().or(e["title"].as_str()).unwrap_or_default();
            o.props.push(Prop::new("SUMMARY", &escape(title)));
            overrides.push(o);
        }
    }
    let mut cal = Component::new("VCALENDAR");
    cal.props.push(Prop::new("VERSION", "2.0"));
    cal.props.push(Prop::new("PRODID", "-//MouseWerk//Arcalo Mail JMAP//EN"));
    cal.children.push(ev);
    cal.children.extend(overrides);
    Some(cal.to_text())
}

/// What a write sends for an object's appointment: the JSCalendar properties the app changes.
pub fn patch_from_ics(data: &str) -> Option<Value> {
    let far = (
        DateTime::parse_from_rfc3339("1900-01-01T00:00:00Z").ok()?.with_timezone(&Utc),
        DateTime::parse_from_rfc3339("2200-01-01T00:00:00Z").ok()?.with_timezone(&Utc),
    );
    let o = ics::parse(data.as_bytes(), far, &Zone::Local).ok()?.into_iter().next()?;
    let zone = if o.all_day { None } else { o.tzid.clone().or_else(super::tz::local_name) };
    let wall = match zone.as_deref().and_then(Zone::named) {
        Some(z) => z.to_wall(o.start),
        None if o.all_day => Zone::Local.to_wall(o.start),
        None => o.start.naive_utc(),
    };
    let secs = (o.end - o.start).num_seconds().max(0);
    let duration = if o.all_day {
        format!("P{}D", (secs / 86_400).max(1))
    } else {
        format!("PT{}H{}M{}S", secs / 3600, (secs % 3600) / 60, secs % 60)
    };
    let mut locations = Map::new();
    if !o.location.is_empty() {
        locations.insert("l1".into(), json!({ "@type": "Location", "name": o.location }));
    }
    Some(json!({
        "@type": "Event",
        "uid": o.uid,
        "title": o.title,
        "start": wall.format("%Y-%m-%dT%H:%M:%S").to_string(),
        "timeZone": if o.all_day { Value::Null } else { json!(zone.unwrap_or_else(|| "Etc/UTC".into())) },
        "showWithoutTime": o.all_day,
        "duration": duration,
        "locations": locations,
        "description": o.body.unwrap_or_default(),
    }))
}

/// The calendars account of a JMAP session (`None`: the server offers no calendars).
pub fn account_of(client: &JmapClient) -> Option<String> {
    client.primary_account(CALENDARS)
}

async fn call(client: &JmapClient, name: &str, mut args: Value) -> Result<Value> {
    let account = account_of(client).ok_or_else(|| Error::Invalid("JMAP: no calendars".into()))?;
    if let Some(o) = args.as_object_mut() {
        o.insert("accountId".into(), json!(account));
    }
    client.call_using(&[CORE, CALENDARS], name, args).await
}

/// The account's calendars.
pub async fn calendars(client: &JmapClient) -> Result<Vec<JmapCalendar>> {
    let v = call(client, "Calendar/get", json!({ "ids": null })).await?;
    Ok(v["list"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|c| {
                    Some(JmapCalendar {
                        id: c["id"].as_str()?.to_string(),
                        name: c["name"].as_str().unwrap_or_default().to_string(),
                        color: c["color"].as_str().map(str::to_owned),
                        writable: c["myRights"]["mayWriteAll"]
                            .as_bool()
                            .or(c["myRights"]["mayWriteOwn"].as_bool())
                            .unwrap_or(true),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// What changed since `state` (`None`: everything). Returns the events by calendar id (an event
/// may be in one), the ids gone and the new state.
pub struct Changes {
    pub full: bool,
    pub changed: Vec<Value>,
    pub removed: Vec<String>,
    pub state: String,
}

pub async fn changes(client: &JmapClient, state: Option<&str>) -> Result<Changes> {
    if let Some(s) = state {
        match call(client, "CalendarEvent/changes", json!({ "sinceState": s })).await {
            Ok(v) => {
                let mut ids: Vec<String> = vec![];
                for k in ["created", "updated"] {
                    ids.extend(v[k].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)));
                }
                let removed = v["destroyed"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect();
                let changed = get(client, &ids).await?;
                return Ok(Changes {
                    full: false,
                    changed,
                    removed,
                    state: v["newState"].as_str().unwrap_or(s).to_string(),
                });
            }
            Err(e) if crate::jmap::cannot_calculate_changes(&e) => {}
            Err(e) => return Err(e),
        }
    }
    let q = call(client, "CalendarEvent/query", json!({})).await?;
    let ids: Vec<String> =
        q["ids"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
    let got = call(client, "CalendarEvent/get", json!({ "ids": [] })).await?;
    let state = got["state"].as_str().unwrap_or_default().to_string();
    Ok(Changes { full: true, changed: get(client, &ids).await?, removed: vec![], state })
}

async fn get(client: &JmapClient, ids: &[String]) -> Result<Vec<Value>> {
    let mut out = vec![];
    for batch in ids.chunks(client.max_get.max(1)) {
        let v = call(client, "CalendarEvent/get", json!({ "ids": batch })).await?;
        out.extend(v["list"].as_array().cloned().unwrap_or_default());
    }
    Ok(out)
}

/// The calendar an event is in.
pub fn calendar_of(e: &Value) -> Option<String> {
    e["calendarIds"].as_object().and_then(|m| m.iter().find(|(_, v)| v.as_bool() == Some(true)).map(|(k, _)| k.clone()))
}

/// Creates an event; returns its id.
pub async fn create(client: &JmapClient, calendar: &str, data: &str) -> Result<String> {
    let mut e = patch_from_ics(data).ok_or_else(|| Error::Invalid("JMAP: no event".into()))?;
    e["calendarIds"] = json!({ calendar: true });
    let v = call(client, "CalendarEvent/set", json!({ "create": { "n": e } })).await?;
    if let Some(err) = v["notCreated"]["n"].as_object() {
        return Err(Error::Invalid(format!(
            "JMAP: {}",
            err.get("type").and_then(Value::as_str).unwrap_or("notCreated")
        )));
    }
    v["created"]["n"]["id"].as_str().map(str::to_owned).ok_or_else(|| Error::Network("JMAP: no id".into()))
}

/// Writes an event's changed properties.
pub async fn update(client: &JmapClient, id: &str, data: &str) -> Result<()> {
    let mut e = patch_from_ics(data).ok_or_else(|| Error::Invalid("JMAP: no event".into()))?;
    e.as_object_mut().map(|o| o.remove("uid"));
    let v = call(client, "CalendarEvent/set", json!({ "update": { id: e } })).await?;
    if let Some(err) = v["notUpdated"][id].as_object() {
        return Err(Error::Invalid(format!(
            "JMAP: {}",
            err.get("type").and_then(Value::as_str).unwrap_or("notUpdated")
        )));
    }
    Ok(())
}

pub async fn destroy(client: &JmapClient, id: &str) -> Result<()> {
    call(client, "CalendarEvent/set", json!({ "destroy": [id] })).await?;
    Ok(())
}

#[cfg(any(test, feature = "fake"))]
pub mod fake {
    //! The calendar methods of the fake JMAP server: two calendars, events by id, a state that
    //! counts every change.

    use super::*;

    #[derive(Debug, Clone, Default)]
    pub struct FakeJmapCalendars {
        pub events: BTreeMap<String, Value>,
        pub state: u64,
        /// What changed at which state: (state, id, destroyed).
        pub log: Vec<(u64, String, bool)>,
        next: u64,
    }

    impl FakeJmapCalendars {
        pub fn put(&mut self, mut e: Value) -> String {
            self.next += 1;
            self.state += 1;
            let id = e["id"].as_str().map(str::to_owned).unwrap_or_else(|| format!("ev{}", self.next));
            e["id"] = json!(id);
            if e["uid"].is_null() {
                e["uid"] = json!(format!("{id}@jmap.example"));
            }
            self.events.insert(id.clone(), e);
            self.log.push((self.state, id.clone(), false));
            id
        }

        pub fn method(&mut self, name: &str, args: &Value) -> std::result::Result<Value, Value> {
            match name {
                "Calendar/get" => Ok(json!({ "state": "c1", "list": [
                    {"id": "cal1", "name": "Persönlich", "color": "#ca8a04", "myRights": {"mayWriteAll": true}},
                    {"id": "cal2", "name": "Team", "color": "#0891b2", "myRights": {"mayWriteAll": false}}
                ], "notFound": [] })),
                "CalendarEvent/query" => {
                    Ok(json!({ "ids": self.events.keys().collect::<Vec<_>>(), "queryState": self.state.to_string() }))
                }
                "CalendarEvent/get" => {
                    let ids: Vec<String> = args["ids"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                        .unwrap_or_else(|| self.events.keys().cloned().collect());
                    let list: Vec<Value> = ids.iter().filter_map(|i| self.events.get(i).cloned()).collect();
                    Ok(json!({ "state": self.state.to_string(), "list": list, "notFound": [] }))
                }
                "CalendarEvent/changes" => {
                    let since: u64 = args["sinceState"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| json!({"type": "cannotCalculateChanges"}))?;
                    let mut created = vec![];
                    let mut destroyed = vec![];
                    let mut seen = std::collections::BTreeSet::new();
                    for (s, id, gone) in self.log.iter().rev() {
                        if *s <= since || !seen.insert(id.clone()) {
                            continue;
                        }
                        if *gone || !self.events.contains_key(id) {
                            destroyed.push(id.clone())
                        } else {
                            created.push(id.clone())
                        }
                    }
                    Ok(
                        json!({ "oldState": since.to_string(), "newState": self.state.to_string(), "created": [], "updated": created, "destroyed": destroyed, "hasMoreChanges": false }),
                    )
                }
                "CalendarEvent/set" => {
                    let mut out =
                        json!({ "oldState": self.state.to_string(), "created": {}, "updated": {}, "destroyed": [] });
                    if let Some(c) = args["create"].as_object() {
                        for (k, e) in c {
                            if calendar_of(e).as_deref() == Some("cal2") {
                                out["notCreated"][k] = json!({"type": "forbidden"});
                                continue;
                            }
                            let id = self.put(e.clone());
                            out["created"][k] = json!({ "id": id });
                        }
                    }
                    if let Some(u) = args["update"].as_object() {
                        for (id, patch) in u {
                            let Some(mut e) = self.events.get(id).cloned() else {
                                out["notUpdated"][id] = json!({"type": "notFound"});
                                continue;
                            };
                            for (k, v) in patch.as_object().cloned().unwrap_or_default() {
                                e[k] = v;
                            }
                            self.put(e);
                            out["updated"][id] = Value::Null;
                        }
                    }
                    if let Some(d) = args["destroy"].as_array() {
                        for id in d.iter().filter_map(|x| x.as_str()) {
                            if self.events.remove(id).is_some() {
                                self.state += 1;
                                self.log.push((self.state, id.to_string(), true));
                                if let Some(a) = out["destroyed"].as_array_mut() {
                                    a.push(json!(id));
                                }
                            }
                        }
                    }
                    out["newState"] = json!(self.state.to_string());
                    Ok(out)
                }
                _ => Err(json!({"type": "unknownMethod"})),
            }
        }
    }
}
