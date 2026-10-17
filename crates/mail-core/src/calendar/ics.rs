//! iCalendar (RFC 5545), taken over from Arcalo (`calsync/ics.rs`): content lines (folding,
//! parameters, escapes), components, time zones (`TZID` with IANA or Windows names, `VTIMEZONE`
//! rules) and the appointments inside a window, series expanded (`RRULE`, `RDATE`, `EXDATE`,
//! changed and cancelled instances via `RECURRENCE-ID`). Added here: writing objects back
//! ([`Component::to_text`], [`new_event`], [`edit_event`]) with folded lines and the zone's
//! `VTIMEZONE`, and subscriptions fetched through the network module.
//!
//! Series are expanded in wall-clock time of their zone, so a meeting at 10:00 stays at 10:00
//! across a change to or from daylight saving time; each instance is then placed in UTC.

use crate::{tr, trf};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rrule::{RRule, Unvalidated};

use super::tz::{Zone, ZoneRules};
use super::{Busy, Occurrence, attendee_name, instant_id, meeting_link};
use crate::error::{Error, Result};

/// At most this many instances of one series are generated, and this many events kept per file.
const MAX_INSTANCES: u16 = 2000;
const MAX_EVENTS: usize = 20_000;
/// Prefix of the id given to an event without a UID (derived from its title and start). Never
/// shown.
const SYNTHETIC_UID: &str = "arcalo-mail-";

/// A content line: `NAME;PARAM=value:VALUE`.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    /// Upper case.
    pub name: String,
    /// Names upper case, values without quotes.
    pub params: Vec<(String, String)>,
    /// Raw value (escapes not resolved).
    pub value: String,
}

impl Prop {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Component {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> + 'a {
        self.props.iter().filter(move |p| p.name == name)
    }
    /// A text property with its escapes resolved, trimmed.
    pub fn text(&self, name: &str) -> Option<String> {
        self.prop(name).map(|p| unescape(&p.value).trim().to_owned())
    }
}

/// The bytes of a calendar file as UTF-8: UTF-16 (with its byte order mark, as Windows'
/// „Unicode“ saves it) is converted; other files are taken as they are.
fn utf8_bytes(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let utf16 = |b: &[u8], le: bool| -> Vec<u8> {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units).into_bytes()
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true).into(),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false).into(),
        _ => bytes.into(),
    }
}

/// Text of a file that is not UTF-8: Windows-1252 (older Outlook and Exchange exports), whose
/// 0x80–0x9F hold € and typographic quotes; the rest is Latin-1.
fn windows_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘',
        '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    bytes.iter().map(|&b| if (0x80..0xA0).contains(&b) { HIGH[(b - 0x80) as usize] } else { b as char }).collect()
}

/// Undoes the line folding (CRLF or LF followed by a space or tab) on the bytes, so a fold
/// inside a multi-byte character (allowed by the RFC: lines are folded at 75 octets) heals.
fn unfold(bytes: &[u8]) -> String {
    let bytes = utf8_bytes(bytes);
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let fold = |j: usize| j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t');
        if b == b'\r' && bytes.get(i + 1) == Some(&b'\n') && fold(i + 2) {
            i += 3;
            continue;
        }
        if b == b'\n' && fold(i + 1) {
            i += 2;
            continue;
        }
        out.push(b);
        i += 1;
    }
    match String::from_utf8(out) {
        Ok(s) => s,
        Err(e) => windows_1252(e.as_bytes()),
    }
}

/// One content line; `None` for lines without a `:`.
pub fn parse_line(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let mut colon = None;
    let mut cuts = vec![];
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => cuts.push(i),
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let head = &line[..colon];
    let mut bounds = vec![0];
    bounds.extend(cuts.iter().map(|c| c + 1));
    let mut parts = vec![];
    for (n, start) in bounds.iter().enumerate() {
        let end = cuts.get(n).copied().unwrap_or(head.len());
        parts.push(&head[*start..end]);
    }
    let name = parts.first()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts[1..]
        .iter()
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_owned()))
        })
        .collect();
    Some(Prop { name, params, value: line[colon + 1..].to_owned() })
}

/// The components of a file (usually one `VCALENDAR`), leniently: a missing `END` closes at
/// the end of the file, a stray one closes up to its `BEGIN`.
pub fn parse_components(bytes: &[u8]) -> Vec<Component> {
    let text = unfold(bytes);
    let mut top = vec![];
    let mut stack: Vec<Component> = vec![];
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let Some(p) = parse_line(line) else { continue };
        match p.name.as_str() {
            "BEGIN" => stack.push(Component { name: p.value.trim().to_ascii_uppercase(), ..Default::default() }),
            "END" => {
                let name = p.value.trim().to_ascii_uppercase();
                if !stack.iter().any(|c| c.name == name) {
                    continue;
                }
                while let Some(c) = stack.pop() {
                    let done = c.name == name;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(c),
                        None => top.push(c),
                    }
                    if done {
                        break;
                    }
                }
            }
            _ => {
                if let Some(c) = stack.last_mut() {
                    c.props.push(p);
                }
            }
        }
    }
    while let Some(c) = stack.pop() {
        match stack.last_mut() {
            Some(parent) => parent.children.push(c),
            None => top.push(c),
        }
    }
    top
}

/// TEXT escapes: `\n`, `\,`, `\;`, `\\`.
pub fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// A list value split at commas that are not escaped.
fn split_list(v: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut esc = false;
    for c in v.chars() {
        if esc {
            cur.push('\\');
            cur.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ',' {
            out.push(unescape(&cur).trim().to_owned());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out.push(unescape(&cur).trim().to_owned());
    out.retain(|s| !s.is_empty());
    out
}

/// A date or a date-time with its zone.
#[derive(Debug, Clone)]
enum When {
    Date(NaiveDate),
    Time(NaiveDateTime, Zone),
}

impl When {
    fn utc(&self, default: &Zone) -> DateTime<Utc> {
        match self {
            When::Date(d) => default.to_utc(d.and_hms_opt(0, 0, 0).unwrap()),
            When::Time(w, z) => z.to_utc(*w),
        }
    }
    /// The wall-clock time in `zone`.
    fn wall_in(&self, zone: &Zone) -> NaiveDateTime {
        match self {
            When::Date(d) => d.and_hms_opt(0, 0, 0).unwrap(),
            When::Time(w, z) => zone.to_wall(z.to_utc(*w)),
        }
    }
}

/// Zones of one file: its `VTIMEZONE`s and the zone of floating times.
struct Zones {
    defined: HashMap<String, Zone>,
    default: Zone,
}

impl Zones {
    fn resolve(&self, tzid: &str) -> Zone {
        Zone::named(tzid)
            .or_else(|| self.defined.get(tzid.trim().trim_matches('"')).cloned())
            .unwrap_or_else(|| self.default.clone())
    }
}

fn parse_naive(v: &str) -> Option<NaiveDateTime> {
    let v = v.trim().trim_end_matches(['Z', 'z']);
    NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M"))
        .or_else(|_| NaiveDateTime::parse_from_str(v, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}

fn parse_date(v: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v.trim(), "%Y%m%d").or_else(|_| NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d")).ok()
}

/// The values of a date property (one, or a comma list for `EXDATE`/`RDATE`).
fn whens(p: &Prop, zones: &Zones) -> Vec<When> {
    let is_date = p.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
    p.value
        .split(',')
        .filter_map(|v| {
            let v = v.trim();
            if is_date || (v.len() == 8 && !v.contains('T')) {
                return parse_date(v).map(When::Date);
            }
            let wall = parse_naive(v)?;
            let zone = if v.ends_with(['Z', 'z']) {
                Zone::Utc
            } else {
                p.param("TZID").map(|t| zones.resolve(t)).unwrap_or_else(|| zones.default.clone())
            };
            Some(When::Time(wall, zone))
        })
        .collect()
}

fn when(c: &Component, name: &str, zones: &Zones) -> Option<When> {
    c.prop(name).and_then(|p| whens(p, zones).into_iter().next())
}

/// `P1DT2H30M`, `PT15M`, `-PT5M`, `P2W`.
pub fn parse_duration(v: &str) -> Option<Duration> {
    let v = v.trim();
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let v = v.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut in_time = false;
    for c in v.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                // Out of range (a broken file): no duration rather than a panic.
                let part = match (c, in_time) {
                    ('W', _) => Duration::try_weeks(n),
                    ('D', _) => Duration::try_days(n),
                    ('H', true) => Duration::try_hours(n),
                    ('M', true) => Duration::try_minutes(n),
                    ('S', true) => Duration::try_seconds(n),
                    _ => return None,
                }?;
                total = total.checked_add(&part)?;
            }
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(if neg { -total } else { total })
}

/// `+0100`, `-0530`, `+010000`.
fn parse_offset(v: &str) -> Option<i32> {
    let v = v.trim();
    let sign = match v.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let d = &v[1..];
    if d.len() < 4 || !d.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i32 = d[..2].parse().ok()?;
    let m: i32 = d[2..4].parse().ok()?;
    let s: i32 = if d.len() >= 6 { d[4..6].parse().ok()? } else { 0 };
    Some(sign * (h * 3600 + m * 60 + s))
}

/// Wall-clock starts of `rule` from `dtstart` inside `from..=to`, expanded as floating time.
/// `until` replaces the rule's UNTIL (already converted to wall-clock time).
fn expand(
    dtstart: NaiveDateTime,
    rule: &str,
    until: Option<NaiveDateTime>,
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> std::result::Result<Vec<NaiveDateTime>, String> {
    let rest: Vec<&str> = rule
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.to_ascii_uppercase().starts_with("UNTIL="))
        .collect();
    let floating = |w: NaiveDateTime| rrule::Tz::UTC.from_utc_datetime(&w);
    let mut r: RRule<Unvalidated> =
        format!("RRULE:{}", rest.join(";")).parse().map_err(|e: rrule::RRuleError| e.to_string())?;
    if let Some(u) = until {
        r = r.until(floating(u.max(dtstart)));
    }
    let set = r.build(floating(dtstart)).map_err(|e| e.to_string())?;
    let dates = set.after(floating(from)).before(floating(to)).all(MAX_INSTANCES).dates;
    Ok(dates.into_iter().map(|d| d.naive_utc()).collect())
}

/// The UNTIL of a rule in wall-clock time of `zone`: UTC values are converted, dates end at 23:59:59.
fn rule_until(rule: &str, zone: &Zone) -> Option<NaiveDateTime> {
    let v = rule.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case("UNTIL").then(|| v.trim().to_owned())
    })?;
    if v.len() == 8 {
        return parse_date(&v).and_then(|d| d.and_hms_opt(23, 59, 59));
    }
    let wall = parse_naive(&v)?;
    Some(if v.ends_with(['Z', 'z']) { zone.to_wall(Utc.from_utc_datetime(&wall)) } else { wall })
}

/// The rules of a `VTIMEZONE` (transitions from 1970 to 2100).
fn zone_rules(c: &Component) -> Option<ZoneRules> {
    let mut transitions = vec![];
    let mut initial = None;
    for part in c.children.iter().filter(|p| p.name == "STANDARD" || p.name == "DAYLIGHT") {
        let (Some(from), Some(to)) = (
            part.prop("TZOFFSETFROM").and_then(|p| parse_offset(&p.value)),
            part.prop("TZOFFSETTO").and_then(|p| parse_offset(&p.value)),
        ) else {
            continue;
        };
        let Some(start) = part.prop("DTSTART").and_then(|p| parse_naive(&p.value)) else { continue };
        if part.name == "STANDARD" && initial.is_none() {
            initial = Some(to);
        }
        let mut walls = vec![start];
        if let Some(rule) = part.prop("RRULE").map(|p| p.value.clone()) {
            // Outlook starts its rules in 1601: begin in 1970 to keep the expansion short.
            let begin = if start.year() < 1970 {
                NaiveDate::from_ymd_opt(1970, start.month(), start.day().min(28)).unwrap().and_time(start.time())
            } else {
                start
            };
            let until = rule_until(
                &rule,
                &Zone::Fixed(chrono::FixedOffset::east_opt(from).unwrap_or(chrono::FixedOffset::east_opt(0).unwrap())),
            );
            let end = NaiveDate::from_ymd_opt(2100, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
            if until.is_none_or(|u| u >= begin)
                && let Ok(more) = expand(begin, &rule, until, begin, end)
            {
                walls.extend(more);
            }
        }
        for rd in part.all("RDATE") {
            walls.extend(rd.value.split(',').filter_map(parse_naive));
        }
        for w in walls {
            transitions.push((Utc.from_utc_datetime(&(w - Duration::seconds(from as i64))), to));
        }
    }
    if transitions.is_empty() {
        return None;
    }
    transitions.sort_by_key(|(t, _)| *t);
    transitions.dedup_by_key(|(t, _)| *t);
    let initial = initial.unwrap_or(transitions[0].1);
    Some(ZoneRules { transitions, initial })
}

/// What an appointment says, before it is placed in time.
struct Details {
    title: String,
    location: String,
    organizer: String,
    attendees: Vec<String>,
    body: Option<String>,
    link: Option<String>,
    busy: Busy,
    private: bool,
    categories: Vec<String>,
}

fn person(p: &Prop) -> String {
    match p.param("CN").map(str::trim).filter(|s| !s.is_empty()) {
        Some(cn) => cn.to_owned(),
        None => {
            let v = p.value.trim();
            v.strip_prefix("mailto:").or_else(|| v.strip_prefix("MAILTO:")).unwrap_or(v).to_owned()
        }
    }
}

/// An attendee as `Name <address>` when the entry has both (the follow-up mail needs the address,
/// everything shown takes the name via [`attendee_name`]), else the name or the address.
fn attendee(p: &Prop) -> String {
    let name = person(p);
    let v = p.value.trim();
    let address = v.strip_prefix("mailto:").or_else(|| v.strip_prefix("MAILTO:")).unwrap_or(v).trim();
    if address.contains('@') && !name.contains('@') && !address.contains(['<', '>', ' ']) && !name.is_empty() {
        format!("{name} <{address}>")
    } else {
        name
    }
}

fn details(c: &Component) -> Details {
    let body = c.text("DESCRIPTION").filter(|b| !b.is_empty());
    let location = c.text("LOCATION").unwrap_or_default();
    let mut busy = match c.text("X-MICROSOFT-CDO-BUSYSTATUS").unwrap_or_default().to_ascii_uppercase().as_str() {
        "FREE" => Busy::Free,
        "TENTATIVE" => Busy::Tentative,
        "OOF" => Busy::Oof,
        "WORKINGELSEWHERE" => Busy::Elsewhere,
        "BUSY" => Busy::Busy,
        _ if c.text("TRANSP").is_some_and(|t| t.eq_ignore_ascii_case("TRANSPARENT")) => Busy::Free,
        _ => Busy::Busy,
    };
    if busy == Busy::Busy && c.text("STATUS").is_some_and(|s| s.eq_ignore_ascii_case("TENTATIVE")) {
        busy = Busy::Tentative;
    }
    let mut attendees: Vec<String> = vec![];
    for a in c.all("ATTENDEE") {
        let kind = a.param("CUTYPE").unwrap_or("").to_ascii_uppercase();
        if kind == "RESOURCE" || kind == "ROOM" {
            continue;
        }
        let entry = attendee(a);
        if !entry.is_empty() && !attendees.iter().any(|x| attendee_name(x) == attendee_name(&entry)) {
            attendees.push(entry);
        }
    }
    let url_props: Vec<String> =
        ["X-MICROSOFT-SKYPETEAMSMEETINGURL", "X-GOOGLE-CONFERENCE", "URL"].iter().filter_map(|n| c.text(n)).collect();
    // The HTML description (Outlook/Exchange) often carries the join link when the plain one is cut.
    let alt_desc = c.text("X-ALT-DESC").unwrap_or_default();
    let link = meeting_link(url_props.iter().map(String::as_str).chain([
        location.as_str(),
        body.as_deref().unwrap_or(""),
        alt_desc.as_str(),
    ]));
    Details {
        title: c.text("SUMMARY").unwrap_or_default(),
        location,
        organizer: c.prop("ORGANIZER").map(attendee).unwrap_or_default(),
        attendees,
        body,
        link,
        busy,
        private: c
            .text("CLASS")
            .is_some_and(|v| v.eq_ignore_ascii_case("PRIVATE") || v.eq_ignore_ascii_case("CONFIDENTIAL")),
        categories: c.all("CATEGORIES").flat_map(|p| split_list(&p.value)).collect(),
    }
}

fn cancelled(c: &Component) -> bool {
    c.text("STATUS").is_some_and(|s| s.eq_ignore_ascii_case("CANCELLED"))
}

/// Length of an appointment: all-day in days (at least one), otherwise as a duration.
enum Length {
    Days(i64),
    Span(Duration),
}

/// Longest appointment taken from a file (a longer one is cut): placing it stays in range.
const MAX_LENGTH_DAYS: i64 = 3660;

fn length(c: &Component, start: &When, zones: &Zones) -> Length {
    let dur = c.prop("DURATION").and_then(|p| parse_duration(&p.value));
    let max = Duration::days(MAX_LENGTH_DAYS);
    match (start, when(c, "DTEND", zones)) {
        (When::Date(d), Some(When::Date(e))) => Length::Days((e - *d).num_days().clamp(1, MAX_LENGTH_DAYS)),
        (When::Date(_), _) => Length::Days(dur.map_or(1, |d| d.num_days().clamp(1, MAX_LENGTH_DAYS))),
        (When::Time(..), Some(end)) => {
            Length::Span((end.utc(&zones.default) - start.utc(&zones.default)).clamp(Duration::zero(), max))
        }
        (When::Time(..), None) => Length::Span(dur.unwrap_or_else(Duration::zero).clamp(Duration::zero(), max)),
    }
}

/// One placed instance: start, end and all-day flag.
fn place(start: &When, len: &Length, zones: &Zones) -> (DateTime<Utc>, DateTime<Utc>, bool) {
    match (start, len) {
        (When::Date(d), Length::Days(n)) => {
            let s = zones.default.to_utc(d.and_hms_opt(0, 0, 0).unwrap());
            let e = zones.default.to_utc((*d + Duration::days(*n)).and_hms_opt(0, 0, 0).unwrap());
            (s, e, true)
        }
        (When::Date(d), Length::Span(span)) => {
            let s = zones.default.to_utc(d.and_hms_opt(0, 0, 0).unwrap());
            (s, s + *span, true)
        }
        (When::Time(..), Length::Days(n)) => {
            let s = start.utc(&zones.default);
            (s, s + Duration::days(*n), false)
        }
        (When::Time(..), Length::Span(span)) => {
            let s = start.utc(&zones.default);
            (s, s + *span, false)
        }
    }
}

/// Key of an instance for matching `RECURRENCE-ID`s: the day of an all-day instance, the UTC
/// instant otherwise.
fn instance_key(w: &When, default: &Zone) -> String {
    match w {
        When::Date(d) => d.format("%Y-%m-%d").to_string(),
        When::Time(..) => instant_id(w.utc(default)),
    }
}

/// The appointments of an ICS file that overlap `window`. Floating times and all-day
/// appointments are placed in `local`.
pub fn parse(bytes: &[u8], window: (DateTime<Utc>, DateTime<Utc>), local: &Zone) -> Result<Vec<Occurrence>> {
    let top = parse_components(bytes);
    let cals: Vec<&Component> = top.iter().filter(|c| c.name == "VCALENDAR").collect();
    if cals.is_empty() {
        return Err(not_a_calendar(bytes));
    }
    let mut defined = HashMap::new();
    for tz in cals.iter().flat_map(|c| c.children.iter()).filter(|c| c.name == "VTIMEZONE") {
        if let (Some(id), Some(rules)) = (tz.text("TZID"), zone_rules(tz)) {
            defined.insert(id, Zone::Rules(Arc::new(rules)));
        }
    }
    let zones = Zones { defined, default: local.clone() };

    // Masters and changed instances by UID, in file order.
    let mut order: Vec<String> = vec![];
    let mut groups: HashMap<String, (Option<&Component>, Vec<&Component>)> = HashMap::new();
    for (n, ev) in cals.iter().flat_map(|c| c.children.iter()).filter(|c| c.name == "VEVENT").enumerate() {
        let uid = ev.text("UID").filter(|u| !u.is_empty()).unwrap_or_else(|| {
            let seed = format!(
                "{}|{}",
                ev.text("SUMMARY").unwrap_or_default(),
                ev.prop("DTSTART").map(|p| p.value.as_str()).unwrap_or("")
            );
            format!(
                "{SYNTHETIC_UID}{n}-{:x}",
                seed.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64))
            )
        });
        let entry = groups.entry(uid.clone()).or_insert_with(|| {
            order.push(uid.clone());
            (None, vec![])
        });
        if ev.prop("RECURRENCE-ID").is_some() {
            entry.1.push(ev);
        } else {
            entry.0 = Some(ev);
        }
    }

    let (from, to) = window;
    let in_window = |s: DateTime<Utc>, e: DateTime<Utc>| s < to && (e > from || (e == s && s >= from));
    let mut out = vec![];
    for uid in order {
        let (master, overrides) = &groups[&uid];
        // An all-day series: an EXDATE or RECURRENCE-ID given with a time (Exchange, some CalDAV
        // servers) means the date it names in its own zone (a UTC time: the local date).
        let all_day = master.and_then(|m| when(m, "DTSTART", &zones)).is_some_and(|s| matches!(s, When::Date(_)));
        let as_day = |w: When| match w {
            When::Time(t, Zone::Utc) if all_day => When::Date(zones.default.to_wall(Zone::Utc.to_utc(t)).date()),
            When::Time(t, _) if all_day => When::Date(t.date()),
            w => w,
        };
        let mut changed: HashMap<String, &Component> = HashMap::new();
        for o in overrides {
            if let Some(rid) = when(o, "RECURRENCE-ID", &zones) {
                changed.insert(instance_key(&as_day(rid), &zones.default), o);
            }
        }
        let mut used: HashSet<String> = HashSet::new();
        let push = |c: &Component,
                    start: &When,
                    len: &Length,
                    instance: String,
                    recurring: bool,
                    out: &mut Vec<Occurrence>| {
            let (s, e, all_day) = place(start, len, &zones);
            if !in_window(s, e) {
                return;
            }
            let d = details(c);
            let title = d.title.trim().to_owned();
            out.push(Occurrence {
                uid: uid.clone(),
                instance,
                recurring,
                start: s,
                end: e,
                all_day,
                title: if title.is_empty() { tr!("Ohne Titel", "No title").to_owned() } else { title },
                location: d.location,
                organizer: d.organizer,
                attendees: d.attendees,
                body: d.body,
                link: d.link,
                busy: d.busy,
                private: d.private,
                categories: d.categories,
                tzid: match start {
                    When::Time(_, _) => c.prop("DTSTART").and_then(|p| p.param("TZID")).map(str::to_owned),
                    When::Date(_) => None,
                },
            });
        };

        if let Some(m) = master.filter(|m| !cancelled(m)) {
            let Some(start) = when(m, "DTSTART", &zones) else { continue };
            let len = length(m, &start, &zones);
            let zone = match &start {
                When::Date(_) => Zone::Utc,
                When::Time(_, z) => z.clone(),
            };
            let start_wall = start.wall_in(&zone);
            let rule = m.prop("RRULE").map(|p| p.value.clone());
            let rdates: Vec<When> = m.all("RDATE").flat_map(|p| whens(p, &zones)).collect();
            let recurring = rule.is_some() || !rdates.is_empty();
            let mut walls = vec![];
            if recurring {
                let span = match &len {
                    Length::Days(n) => Duration::days(*n),
                    Length::Span(s) => *s,
                };
                let lo = zone.to_wall(from) - Duration::days(2) - span;
                let hi = zone.to_wall(to) + Duration::days(2);
                match &rule {
                    Some(r) => match expand(start_wall, r, rule_until(r, &zone), lo, hi) {
                        Ok(w) => walls = w,
                        // A rule this parser cannot expand: at least the first appointment.
                        Err(_) => walls.push(start_wall),
                    },
                    None => walls.push(start_wall),
                }
                walls.extend(rdates.iter().map(|r| r.wall_in(&zone)));
                let ex: Vec<When> = m.all("EXDATE").flat_map(|p| whens(p, &zones)).map(as_day).collect();
                let ex_days: HashSet<NaiveDate> =
                    ex.iter().filter_map(|w| if let When::Date(d) = w { Some(*d) } else { None }).collect();
                let ex_walls: HashSet<NaiveDateTime> =
                    ex.iter().filter(|w| matches!(w, When::Time(..))).map(|w| w.wall_in(&zone)).collect();
                walls.retain(|w| !ex_walls.contains(w) && !ex_days.contains(&w.date()));
                walls.sort();
                walls.dedup();
            } else {
                walls.push(start_wall);
            }
            for w in walls {
                let inst = match &start {
                    When::Date(_) => When::Date(w.date()),
                    When::Time(..) => When::Time(w, zone.clone()),
                };
                let key = instance_key(&inst, &zones.default);
                let id = if recurring { key.clone() } else { String::new() };
                match changed.get(&key) {
                    Some(o) => {
                        used.insert(key);
                        if cancelled(o) {
                            continue;
                        }
                        let os = when(o, "DTSTART", &zones).unwrap_or(inst);
                        let olen = length(o, &os, &zones);
                        push(o, &os, &olen, id, true, &mut out);
                    }
                    None => push(m, &inst, &len, id, recurring, &mut out),
                }
            }
        }
        // Changed instances whose original time is not part of the expansion (or without a master).
        if master.is_none_or(|m| !cancelled(m)) {
            for (key, o) in &changed {
                if used.contains(key) || cancelled(o) {
                    continue;
                }
                let Some(os) = when(o, "DTSTART", &zones) else { continue };
                let olen = length(o, &os, &zones);
                push(o, &os, &olen, key.clone(), true, &mut out);
            }
        }
        if out.len() > MAX_EVENTS {
            out.truncate(MAX_EVENTS);
            break;
        }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.title.cmp(&b.title)));
    Ok(out)
}

/// The address to fetch: `webcal://` becomes `https://`; only http(s) is allowed.
pub fn fetch_url(url: &str) -> Result<String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    let fixed = if let Some(rest) = lower.strip_prefix("webcals://").or_else(|| lower.strip_prefix("webcal://")) {
        format!("https://{}", &u[u.len() - rest.len()..])
    } else {
        u.to_owned()
    };
    let ok = fixed.to_ascii_lowercase().starts_with("https://") || fixed.to_ascii_lowercase().starts_with("http://");
    if !ok || fixed.len() < 10 {
        return Err(Error::Invalid(
            tr!(
                "Die Kalender-Adresse muss mit https://, http:// oder webcal:// beginnen",
                "The calendar address must start with https://, http:// or webcal://"
            )
            .into(),
        ));
    }
    Ok(fixed)
}

/// A subscription address for display: scheme and host only (path and query may hold a token).
pub fn display_url(url: &str) -> String {
    match fetch_url(url).ok().and_then(|u| reqwest::Url::parse(&u).ok()) {
        Some(u) => format!("{}://{}/…", u.scheme(), u.host_str().unwrap_or("")),
        None => "…".into(),
    }
}

/// The error for data without a `VCALENDAR`: a web page (a login or sharing page instead of the
/// calendar's ICS link) or any other file.
fn not_a_calendar(bytes: &[u8]) -> Error {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).trim_start().to_ascii_lowercase();
    let page = head.starts_with('<') && (head.contains("<html") || head.contains("<!doctype html"));
    Error::Invalid(if page {
        tr!(
            "Hier kommt eine Webseite statt eines Kalenders an (z. B. eine Anmelde- oder Freigabeseite). Kopiere den ICS-Link des Kalenders – er endet meist auf .ics – und abonniere ihn neu.",
            "This returns a web page instead of a calendar (e.g. a sign-in or sharing page). Copy the calendar's ICS link – it usually ends in .ics – and subscribe to it again."
        )
        .into()
    } else {
        tr!(
            "Das ist kein Kalender im iCalendar-Format (BEGIN:VCALENDAR fehlt). Exportiere den Kalender erneut als .ics-Datei oder prüfe den ICS-Link.",
            "This is no calendar in iCalendar format (BEGIN:VCALENDAR is missing). Export the calendar as an .ics file again or check the ICS link."
        )
        .into()
    })
}

/// Largest ICS file read (bytes).
pub const MAX_BYTES: usize = 30 * 1024 * 1024;

/// What a subscription's server gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// The same as last time (`If-None-Match`).
    Unchanged,
    Data {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

/// Downloads a subscription (`etag`: the one of the last download). Errors never contain the
/// URL (it may carry a secret token).
pub async fn fetch(url: &str, etag: Option<&str>) -> Result<Fetched> {
    let target = fetch_url(url)?;
    crate::network::check_https_url(&target)?;
    let http = crate::network::client_for(crate::network::Service::Calendar)?;
    let mut req = http.get(&target).header(reqwest::header::ACCEPT, "text/calendar, */*;q=0.5");
    if let Some(e) = etag {
        req = req.header(reqwest::header::IF_NONE_MATCH, e);
    }
    let resp = req.send().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
    let status = resp.status();
    if status.as_u16() == 304 {
        return Ok(Fetched::Unchanged);
    }
    if !status.is_success() {
        let hint = match status.as_u16() {
            401 | 403 => tr!(
                " – die Adresse ist nicht (mehr) freigegeben oder der Zugriffsschlüssel ist ungültig. Kopiere die Adresse neu aus den Freigabe-Einstellungen des Kalenders.",
                " – the address is not shared (any more) or the access key is invalid. Copy the address again from the calendar's sharing settings."
            ),
            404 => tr!(
                " – die Adresse gibt es nicht (mehr). Kopiere die Adresse neu aus den Freigabe-Einstellungen des Kalenders.",
                " – the address does not exist (any more). Copy the address again from the calendar's sharing settings."
            ),
            500..=599 => tr!(
                " – ein Problem auf dem Server. Die Synchronisierung versucht es später von selbst erneut.",
                " – a problem on the server. The sync tries again later by itself."
            ),
            _ => "",
        };
        return Err(Error::Network(trf!(
            "Der Kalender-Server antwortet mit {status}{hint}",
            "The calendar server answers with {status}{hint}"
        )));
    }
    let etag = resp.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let too_big = || {
        Error::Invalid(tr!("Die Kalenderdatei ist größer als 30 MB.", "The calendar file is larger than 30 MB.").into())
    };
    if resp.content_length().is_some_and(|n| n as usize > MAX_BYTES) {
        return Err(too_big());
    }
    let bytes = resp.bytes().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
    if bytes.len() > MAX_BYTES {
        return Err(too_big());
    }
    if !parse_components(&bytes).iter().any(|c| c.name == "VCALENDAR") {
        return Err(not_a_calendar(&bytes));
    }
    Ok(Fetched::Data { bytes: bytes.to_vec(), etag })
}

// ---------------------------------------------------------------------------- writing

/// TEXT escapes for a value: `\\`, `\;`, `\,`, `\n`.
pub fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// One content line folded at 75 octets (never inside a character), with CRLF.
pub fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + line.len() / 70 * 3 + 2);
    let mut used = 0;
    for c in line.chars() {
        let n = c.len_utf8();
        if used + n > 75 {
            out.push_str("\r\n ");
            used = 1;
        }
        out.push(c);
        used += n;
    }
    out.push_str("\r\n");
    out
}

impl Prop {
    pub fn new(name: &str, value: &str) -> Prop {
        Prop { name: name.to_ascii_uppercase(), params: vec![], value: value.to_owned() }
    }

    pub fn with(mut self, param: &str, value: &str) -> Prop {
        self.params.push((param.to_ascii_uppercase(), value.to_owned()));
        self
    }

    /// The content line (unfolded).
    pub fn line(&self) -> String {
        let mut out = self.name.clone();
        for (k, v) in &self.params {
            let quote = v.contains([':', ';', ',']);
            out.push(';');
            out.push_str(k);
            out.push('=');
            if quote {
                out.push('"');
                out.push_str(&v.replace('"', "'"));
                out.push('"');
            } else {
                out.push_str(v);
            }
        }
        out.push(':');
        out.push_str(&self.value);
        out
    }
}

impl Component {
    pub fn new(name: &str) -> Component {
        Component { name: name.to_ascii_uppercase(), ..Default::default() }
    }

    /// Replaces every property `name` with `p` (or appends it).
    pub fn set(&mut self, p: Prop) {
        match self.props.iter().position(|x| x.name == p.name) {
            Some(i) => {
                self.props[i] = p.clone();
                let name = p.name;
                let mut seen = false;
                self.props.retain(|x| {
                    if x.name != name {
                        return true;
                    }
                    let keep = !seen;
                    seen = true;
                    keep
                });
            }
            None => self.props.push(p),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.props.retain(|p| p.name != name);
    }

    /// The component as iCalendar text with folded lines.
    pub fn to_text(&self) -> String {
        let mut out = fold(&format!("BEGIN:{}", self.name));
        for p in &self.props {
            out.push_str(&fold(&p.line()));
        }
        for c in &self.children {
            out.push_str(&c.to_text());
        }
        out.push_str(&fold(&format!("END:{}", self.name)));
        out
    }
}

/// `20261020T081500Z`.
pub fn utc_text(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Sets a start or end property: a date for all-day appointments, the wall-clock time with
/// `TZID` when the zone is known, else UTC.
fn set_time(c: &mut Component, name: &str, t: DateTime<Utc>, all_day: bool, tzid: Option<&str>) {
    if all_day {
        let day = Zone::Local.to_wall(t).date();
        c.set(Prop::new(name, &day.format("%Y%m%d").to_string()).with("VALUE", "DATE"));
        return;
    }
    match tzid.and_then(|id| Zone::named(id).map(|z| (id, z))) {
        Some((id, zone)) if !matches!(zone, Zone::Utc) => {
            let wall = zone.to_wall(t);
            c.set(Prop::new(name, &wall.format("%Y%m%dT%H%M%S").to_string()).with("TZID", id));
        }
        _ => c.set(Prop::new(name, &utc_text(t))),
    }
}

/// What a change of an appointment says (fields not given stay).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_day: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Fields {
    pub fn merged(&self, newer: &Fields) -> Fields {
        Fields {
            title: newer.title.clone().or_else(|| self.title.clone()),
            start: newer.start.or(self.start),
            end: newer.end.or(self.end),
            location: newer.location.clone().or_else(|| self.location.clone()),
            all_day: newer.all_day.or(self.all_day),
            description: newer.description.clone().or_else(|| self.description.clone()),
        }
    }
}

/// The `VTIMEZONE` of `tzid` around `t`, when the calendar does not have it yet.
fn add_zone(cal: &mut Component, tzid: Option<&str>, t: DateTime<Utc>) {
    let Some(id) = tzid else { return };
    let known = cal.children.iter().any(|c| c.name == "VTIMEZONE" && c.text("TZID").as_deref() == Some(id));
    if known {
        return;
    }
    if let Some(text) = super::tz::vtimezone(id, t, t)
        && let Some(z) = parse_components(text.as_bytes()).into_iter().next()
    {
        cal.children.insert(0, z);
    }
}

/// A new calendar object with one appointment. Timed appointments are written in the device's
/// zone (with its `VTIMEZONE`), so a client in another zone shows them at the same instant.
pub fn new_event(uid: &str, f: &Fields, now: DateTime<Utc>) -> String {
    let mut cal = Component::new("VCALENDAR");
    cal.props.push(Prop::new("VERSION", "2.0"));
    cal.props.push(Prop::new("PRODID", "-//MouseWerk//Arcalo Mail//EN"));
    cal.props.push(Prop::new("CALSCALE", "GREGORIAN"));
    let mut ev = Component::new("VEVENT");
    ev.props.push(Prop::new("UID", uid));
    ev.props.push(Prop::new("DTSTAMP", &utc_text(now)));
    ev.props.push(Prop::new("CREATED", &utc_text(now)));
    ev.props.push(Prop::new("SEQUENCE", "0"));
    let all_day = f.all_day.unwrap_or(false);
    let start = f.start.unwrap_or(now);
    let end = f.end.unwrap_or(start + Duration::minutes(30)).max(start);
    let zone = if all_day { None } else { super::tz::local_name() };
    set_time(&mut ev, "DTSTART", start, all_day, zone.as_deref());
    set_time(
        &mut ev,
        "DTEND",
        if all_day { end.max(start + Duration::days(1)) } else { end },
        all_day,
        zone.as_deref(),
    );
    ev.props.push(Prop::new("SUMMARY", &escape(f.title.as_deref().unwrap_or_default())));
    if let Some(l) = f.location.as_deref().filter(|l| !l.trim().is_empty()) {
        ev.props.push(Prop::new("LOCATION", &escape(l.trim())));
    }
    if let Some(d) = f.description.as_deref().filter(|d| !d.trim().is_empty()) {
        ev.props.push(Prop::new("DESCRIPTION", &escape(d.trim())));
    }
    add_zone(&mut cal, zone.as_deref(), start);
    cal.children.push(ev);
    cal.to_text()
}

/// The object with its appointment changed (the single appointment, or the master of a series):
/// the fields given, `SEQUENCE` counted up when the time changed, `DTSTAMP` now. Everything else
/// in the object stays as the server wrote it. `None` when it holds no appointment.
pub fn edit_event(data: &str, f: &Fields, now: DateTime<Utc>) -> Option<String> {
    let mut top = parse_components(data.as_bytes());
    let cal = top.iter_mut().find(|c| c.name == "VCALENDAR")?;
    let i = cal.children.iter().position(|c| c.name == "VEVENT" && c.prop("RECURRENCE-ID").is_none())?;
    let zones = Zones { defined: HashMap::new(), default: Zone::Local };
    let ev = &cal.children[i];
    let old_start = when(ev, "DTSTART", &zones);
    let was_all_day = matches!(old_start, Some(When::Date(_)));
    let tzid = ev.prop("DTSTART").and_then(|p| p.param("TZID")).map(str::to_owned);
    let old_len = old_start.as_ref().map(|s| length(ev, s, &zones));
    let mut ev = ev.clone();
    if let Some(t) = &f.title {
        ev.set(Prop::new("SUMMARY", &escape(t.trim())));
    }
    if let Some(l) = &f.location {
        if l.trim().is_empty() {
            ev.remove("LOCATION");
        } else {
            ev.set(Prop::new("LOCATION", &escape(l.trim())));
        }
    }
    if let Some(d) = &f.description {
        if d.trim().is_empty() {
            ev.remove("DESCRIPTION");
        } else {
            ev.set(Prop::new("DESCRIPTION", &escape(d.trim())));
        }
    }
    let all_day = f.all_day.unwrap_or(was_all_day);
    let timed = f.start.is_some() || f.end.is_some() || f.all_day.is_some_and(|a| a != was_all_day);
    let mut zone_at = None;
    if timed {
        let (s0, e0) = match (&old_start, &old_len) {
            (Some(s), Some(len)) => {
                let (s, e, _) = place(s, len, &zones);
                (s, e)
            }
            _ => (now, now + Duration::minutes(30)),
        };
        let start = f.start.unwrap_or(s0);
        let end = f.end.unwrap_or(start + (e0 - s0)).max(start);
        // A timed appointment keeps its zone; one that was all-day takes the device's.
        let zone = if all_day { None } else { tzid.clone().or_else(super::tz::local_name) };
        ev.remove("DURATION");
        set_time(&mut ev, "DTSTART", start, all_day, zone.as_deref());
        set_time(
            &mut ev,
            "DTEND",
            if all_day { end.max(start + Duration::days(1)) } else { end },
            all_day,
            zone.as_deref(),
        );
        let seq = ev.prop("SEQUENCE").and_then(|p| p.value.trim().parse::<u32>().ok()).unwrap_or(0);
        ev.set(Prop::new("SEQUENCE", &(seq + 1).to_string()));
        zone_at = zone.map(|z| (z, start));
    }
    ev.set(Prop::new("DTSTAMP", &utc_text(now)));
    ev.set(Prop::new("LAST-MODIFIED", &utc_text(now)));
    cal.children[i] = ev;
    if let Some((z, t)) = zone_at {
        add_zone(cal, Some(&z), t);
    }
    Some(cal.to_text())
}

/// The first and last instant an object can show at (for picking the objects of a range);
/// `None` for the end of a series without an end.
pub fn bounds(data: &str) -> (Option<i64>, Option<i64>) {
    let top = parse_components(data.as_bytes());
    let events: Vec<&Component> = top
        .iter()
        .filter(|c| c.name == "VCALENDAR")
        .flat_map(|c| c.children.iter())
        .filter(|c| c.name == "VEVENT")
        .collect();
    if events.is_empty() {
        return (None, None);
    }
    let open_ended = events.iter().any(|e| e.prop("RRULE").is_some() || e.prop("RDATE").is_some());
    let far = (Utc.with_ymd_and_hms(1900, 1, 1, 0, 0, 0).unwrap(), Utc.with_ymd_and_hms(2200, 1, 1, 0, 0, 0).unwrap());
    if open_ended {
        // A series: from its first start (a day early for zones), no end.
        let zones = Zones { defined: HashMap::new(), default: Zone::Local };
        let first = events.iter().filter_map(|e| when(e, "DTSTART", &zones)).map(|w| w.utc(&Zone::Utc)).min();
        return (first.map(|t| t.timestamp() - 2 * 86_400), None);
    }
    let Ok(list) = parse(data.as_bytes(), far, &Zone::Local) else { return (None, None) };
    let lo = list.iter().map(|o| o.start.timestamp()).min();
    let hi = list.iter().map(|o| o.end.timestamp().max(o.start.timestamp() + 1)).max();
    (lo.map(|t| t - 86_400), hi.map(|t| t + 86_400))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn berlin() -> Zone {
        Zone::named("Europe/Berlin").unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn window() -> (DateTime<Utc>, DateTime<Utc>) {
        (utc(2026, 9, 1, 0, 0), utc(2026, 12, 31, 0, 0))
    }

    fn cal(body: &str) -> Vec<u8> {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//DE\r\n{body}END:VCALENDAR\r\n").into_bytes()
    }

    /// Outlook/Exchange writes Windows zone names with a VTIMEZONE that starts in 1601.
    const OUTLOOK_TZ: &str = "BEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\nBEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nRRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\nEND:STANDARD\r\nBEGIN:DAYLIGHT\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nRRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\n";

    #[test]
    fn content_lines_fold_escape_and_quote() {
        let p = parse_line("ATTENDEE;CN=\"Müller, Jörg\";ROLE=REQ-PARTICIPANT:mailto:j@x.de").unwrap();
        assert_eq!(
            (p.name.as_str(), p.param("CN"), p.value.as_str()),
            ("ATTENDEE", Some("Müller, Jörg"), "mailto:j@x.de")
        );
        assert_eq!(
            parse_line("DESCRIPTION:a\\, b\\; c\\nd\\\\e").map(|p| unescape(&p.value)).unwrap(),
            "a, b; c\nd\\e"
        );
        assert_eq!(split_list("A,B\\,C, D"), ["A", "B,C", "D"]);
        assert!(parse_line("kein doppelpunkt").is_none());
        // A fold inside the two bytes of „ü“ heals on the byte level.
        let bytes = [b"SUMMARY:Gr\xC3".as_slice(), b"\r\n \xBC\xC3\x9Fe\r\n".as_slice()].concat();
        let comps = parse_components(&[b"BEGIN:VEVENT\r\n".as_slice(), &bytes, b"END:VEVENT\r\n"].concat());
        assert_eq!(comps[0].text("SUMMARY").unwrap(), "Grüße");
        assert_eq!(parse_duration("P1DT2H30M"), Some(Duration::minutes(26 * 60 + 30)));
        assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
        assert_eq!(parse_duration("P2W"), Some(Duration::days(14)));
        assert_eq!(parse_duration("1H"), None);
        // Out of range: none, no panic.
        for v in ["P999999999999999D", "P99999999999999W", "PT9223372036854775807S", "P9223372036854775807D"] {
            assert_eq!(parse_duration(v), None, "{v}");
        }
        assert_eq!(parse_offset("+0530"), Some(19800));
        assert_eq!(parse_offset("-0100"), Some(-3600));
    }

    #[test]
    fn outlook_series_with_windows_zone_exdate_and_changed_instances() {
        // Weekly on Wednesday 10:00 local, from 2 Sep until 4 Nov 2026 (UNTIL in UTC = last start).
        // 30 Sep is excluded, 14 Oct moved to 14:00, 21 Oct cancelled.
        let body = format!(
            "{OUTLOOK_TZ}BEGIN:VEVENT\r\nUID:040000008200E00074C5B7101A82E008000000001\r\nSUMMARY;LANGUAGE=de-DE:Jour fixe Änderungen\r\n\
             DTSTART;TZID=W. Europe Standard Time:20260902T100000\r\nDTEND;TZID=W. Europe Standard Time:20260902T110000\r\n\
             RRULE:FREQ=WEEKLY;UNTIL=20261104T090000Z;INTERVAL=1;BYDAY=WE;WKST=MO\r\n\
             EXDATE;TZID=W. Europe Standard Time:20260930T100000\r\nLOCATION:Raum Zürich\r\n\
             ORGANIZER;CN=Anna Müller:mailto:anna@example.com\r\nATTENDEE;CN=Jörg Weiß;RSVP=TRUE:mailto:j@example.com\r\n\
             ATTENDEE;CUTYPE=RESOURCE;CN=Raum Zürich:mailto:r@example.com\r\n\
             DESCRIPTION:Agenda\\n\\nMicrosoft Teams-Besprechung\\nhttps://teams.microsoft.com/l/meetup-join/19%3a1\r\n\
             X-MICROSOFT-CDO-BUSYSTATUS:BUSY\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:040000008200E00074C5B7101A82E008000000001\r\nRECURRENCE-ID;TZID=W. Europe Standard Time:20261014T100000\r\n\
             SUMMARY:Jour fixe (verschoben)\r\nDTSTART;TZID=W. Europe Standard Time:20261014T140000\r\nDTEND;TZID=W. Europe Standard Time:20261014T150000\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:040000008200E00074C5B7101A82E008000000001\r\nRECURRENCE-ID;TZID=W. Europe Standard Time:20261021T100000\r\n\
             STATUS:CANCELLED\r\nDTSTART;TZID=W. Europe Standard Time:20261021T100000\r\nEND:VEVENT\r\n"
        );
        let evs = parse(&cal(&body), window(), &berlin()).unwrap();
        let starts: Vec<_> = evs.iter().map(|e| e.start).collect();
        assert_eq!(
            starts,
            [
                utc(2026, 9, 2, 8, 0),
                utc(2026, 9, 9, 8, 0),
                utc(2026, 9, 16, 8, 0),
                utc(2026, 9, 23, 8, 0),
                utc(2026, 10, 7, 8, 0),
                utc(2026, 10, 14, 12, 0),
                utc(2026, 10, 28, 9, 0), // after the change to winter time: still 10:00 local
                utc(2026, 11, 4, 9, 0),
            ]
        );
        let first = &evs[0];
        assert_eq!(first.title, "Jour fixe Änderungen");
        assert_eq!(
            (first.location.as_str(), first.organizer.as_str()),
            ("Raum Zürich", "Anna Müller <anna@example.com>")
        );
        assert_eq!(first.attendees, ["Jörg Weiß <j@example.com>"], "rooms are no attendees");
        assert_eq!(attendee_name(&first.attendees[0]), "Jörg Weiß");
        assert_eq!(first.link.as_deref(), Some("https://teams.microsoft.com/l/meetup-join/19%3a1"));
        assert!(first.recurring && first.instance == "2026-09-02T08:00:00Z");
        assert_eq!(first.end - first.start, Duration::hours(1));
        let moved = &evs[5];
        assert_eq!((moved.title.as_str(), moved.instance.as_str()), ("Jour fixe (verschoben)", "2026-10-14T08:00:00Z"));
    }

    #[test]
    fn custom_zone_definitions_are_used_for_unknown_names() {
        let tz = OUTLOOK_TZ
            .replace("W. Europe Standard Time", "(UTC+01:00) Amsterdam\\, Berlin\\, Bern\\, Rom\\, Stockholm\\, Wien");
        let body = format!(
            "{tz}BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Sommer\r\nDTSTART;TZID=\"(UTC+01:00) Amsterdam, Berlin, Bern, Rom, Stockholm, Wien\":20260915T090000\r\nDURATION:PT30M\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Winter\r\nDTSTART;TZID=\"(UTC+01:00) Amsterdam, Berlin, Bern, Rom, Stockholm, Wien\":20261115T090000\r\nDURATION:PT30M\r\nEND:VEVENT\r\n"
        );
        // The floating default zone (UTC here) must not be used for these.
        let evs = parse(&cal(&body), window(), &Zone::Utc).unwrap();
        assert_eq!(evs.iter().map(|e| e.start).collect::<Vec<_>>(), [utc(2026, 9, 15, 7, 0), utc(2026, 11, 15, 8, 0)]);
        assert_eq!(evs[0].end - evs[0].start, Duration::minutes(30));
    }

    #[test]
    fn all_day_floating_and_utc_values() {
        let body = "BEGIN:VEVENT\r\nUID:d1\r\nSUMMARY:Betriebsausflug\r\nDTSTART;VALUE=DATE:20260918\r\nDTEND;VALUE=DATE:20260920\r\nTRANSP:TRANSPARENT\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:d2\r\nSUMMARY:Urlaub\r\nDTSTART;VALUE=DATE:20261005\r\nX-MICROSOFT-CDO-BUSYSTATUS:OOF\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:f\r\nSUMMARY:Schwebend\r\nDTSTART:20260910T083000\r\nDTEND:20260910T090000\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:z\r\nSUMMARY:UTC\r\nDTSTART:20260911T083000Z\r\nDTEND:20260911T090000Z\r\nCLASS:PRIVATE\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:old\r\nSUMMARY:Vorbei\r\nDTSTART:20250101T083000Z\r\nEND:VEVENT\r\n";
        let evs = parse(&cal(body), window(), &berlin()).unwrap();
        let by = |uid: &str| evs.iter().find(|e| e.uid == uid).unwrap();
        let trip = by("d1");
        assert!(trip.all_day && !trip.recurring && trip.instance.is_empty());
        assert_eq!(
            (trip.start, trip.end),
            (utc(2026, 9, 17, 22, 0), utc(2026, 9, 19, 22, 0)),
            "local midnights, end exclusive"
        );
        assert_eq!(trip.busy, Busy::Free);
        let vac = by("d2");
        assert_eq!((vac.end - vac.start, vac.busy), (Duration::days(1), Busy::Oof));
        assert_eq!(by("f").start, utc(2026, 9, 10, 6, 30), "floating = local zone");
        let private = by("z");
        assert_eq!((private.start, private.title.as_str(), private.private), (utc(2026, 9, 11, 8, 30), "UTC", true));
        assert!(evs.iter().all(|e| e.uid != "old"), "outside the window");
    }

    #[test]
    fn google_style_count_rdate_and_all_day_series() {
        let body = "BEGIN:VEVENT\r\nUID:g1@google.com\r\nSUMMARY:Daily\r\nDTSTART;TZID=Europe/Berlin:20260921T091500\r\nDTEND;TZID=Europe/Berlin:20260921T093000\r\n\
                    RRULE:FREQ=DAILY;COUNT=5;BYDAY=MO,TU,WE,TH,FR\r\nRDATE;TZID=Europe/Berlin:20260926T100000\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:bd\r\nSUMMARY:Geburtstag\r\nDTSTART;VALUE=DATE:20200930\r\nRRULE:FREQ=YEARLY\r\nEXDATE;VALUE=DATE:20270930\r\nEND:VEVENT\r\n";
        let evs = parse(&cal(body), (utc(2026, 9, 1, 0, 0), utc(2027, 12, 31, 0, 0)), &berlin()).unwrap();
        let daily: Vec<_> = evs.iter().filter(|e| e.uid == "g1@google.com").map(|e| e.start).collect();
        assert_eq!(
            daily,
            [
                utc(2026, 9, 21, 7, 15),
                utc(2026, 9, 22, 7, 15),
                utc(2026, 9, 23, 7, 15),
                utc(2026, 9, 24, 7, 15),
                utc(2026, 9, 25, 7, 15),
                utc(2026, 9, 26, 8, 0),
            ]
        );
        let bday: Vec<_> = evs.iter().filter(|e| e.uid == "bd").collect();
        assert_eq!(bday.len(), 1, "2027 excluded");
        assert_eq!((bday[0].instance.as_str(), bday[0].all_day), ("2026-09-30", true));
    }

    #[test]
    fn all_day_series_with_exdate_and_recurrence_id_given_as_date_time() {
        let series = |extra: &str| {
            format!(
                "BEGIN:VEVENT\r\nUID:w1\r\nSUMMARY:Ganztags\r\nDTSTART;VALUE=DATE:20261005\r\nDTEND;VALUE=DATE:20261006\r\n\
                 RRULE:FREQ=WEEKLY;COUNT=4\r\n{extra}END:VEVENT\r\n"
            )
        };
        let days = |body: &str| -> Vec<String> {
            let mut d: Vec<String> = parse(&cal(body), window(), &berlin())
                .unwrap()
                .iter()
                .map(|e| berlin().to_wall(e.start).date().to_string())
                .collect();
            d.sort();
            d
        };
        let all = ["2026-10-05", "2026-10-12", "2026-10-19", "2026-10-26"];
        assert_eq!(days(&series("")), all);
        for ex in [
            "EXDATE:20261012T000000\r\n",
            "EXDATE;TZID=Europe/Berlin:20261012T000000\r\n",
            "EXDATE:20261011T220000Z\r\n",
        ] {
            assert_eq!(days(&series(ex)), ["2026-10-05", "2026-10-19", "2026-10-26"], "{ex}");
        }
        // The instance of 12 October moved to the 13th: shown once, on the 13th.
        let moved = format!(
            "{}BEGIN:VEVENT\r\nUID:w1\r\nRECURRENCE-ID:20261012T000000\r\nSUMMARY:Ganztags verschoben\r\n\
             DTSTART;VALUE=DATE:20261013\r\nDTEND;VALUE=DATE:20261014\r\nEND:VEVENT\r\n",
            series("")
        );
        assert_eq!(days(&moved), ["2026-10-05", "2026-10-13", "2026-10-19", "2026-10-26"]);
    }

    #[test]
    fn broken_lengths_do_not_panic() {
        let body = "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Lang\r\nDTSTART;VALUE=DATE:20260924\r\nDURATION:P999999999999D\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Ewig\r\nDTSTART;VALUE=DATE:20260924\r\nDTEND;VALUE=DATE:99991231\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Zeit\r\nDTSTART:20260924T100000Z\r\nDURATION:PT9999999999999H\r\nEND:VEVENT\r\n";
        let evs = parse(&cal(body), window(), &Zone::Utc).unwrap();
        let ewig = evs.iter().find(|e| e.uid == "b").unwrap();
        assert_eq!((ewig.end - ewig.start).num_days(), MAX_LENGTH_DAYS);
        assert!(evs.iter().any(|e| e.uid == "a") && evs.iter().any(|e| e.uid == "c"));
    }

    #[test]
    fn dst_change_inside_a_daily_series() {
        let body = "BEGIN:VEVENT\r\nUID:s\r\nSUMMARY:Standup\r\nDTSTART;TZID=Europe/Berlin:20261023T090000\r\nDURATION:PT15M\r\nRRULE:FREQ=DAILY;COUNT=4\r\nEND:VEVENT\r\n";
        let evs = parse(&cal(body), window(), &Zone::Utc).unwrap();
        let hours: Vec<u32> = evs.iter().map(|e| chrono::Timelike::hour(&e.start)).collect();
        assert_eq!(hours, [7, 7, 8, 8], "09:00 local before and after 25 Oct");
    }

    #[test]
    fn broken_input_is_tolerated_or_refused() {
        assert!(parse(b"hello", window(), &berlin()).is_err());
        // Missing END lines, a stray END, an invalid rule and an event without start.
        let body = "BEGIN:VEVENT\r\nUID:x\r\nSUMMARY:Kaputte Regel\r\nDTSTART:20260915T080000Z\r\nRRULE:FREQ=SOMETIMES\r\nEND:VEVENT\r\nEND:VTODO\r\n\
                    BEGIN:VEVENT\r\nUID:y\r\nSUMMARY:Ohne Start\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nSUMMARY:Ohne UID\r\nDTSTART:20260916T080000Z\r\n";
        let evs = parse(format!("BEGIN:VCALENDAR\n{body}").as_bytes(), window(), &berlin()).unwrap();
        let titles: Vec<_> = evs.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["Kaputte Regel", "Ohne UID"]);
        assert!(evs[1].uid.starts_with(SYNTHETIC_UID));
    }

    #[test]
    fn subscription_addresses() {
        assert_eq!(
            fetch_url("webcal://outlook.office365.com/owa/calendar/abc/calendar.ics").unwrap(),
            "https://outlook.office365.com/owa/calendar/abc/calendar.ics"
        );
        assert_eq!(fetch_url(" https://x.de/a.ics ").unwrap(), "https://x.de/a.ics");
        assert!(fetch_url("file:///etc/passwd").is_err());
        assert!(fetch_url("ftp://x").is_err());
        assert_eq!(
            display_url("webcal://calendar.google.com/calendar/ical/private-abc/basic.ics"),
            "https://calendar.google.com/…"
        );
    }

    #[test]
    fn windows_encodings_keep_umlauts() {
        // Windows-1252 (older Outlook exports): ä = E4, € = 80, „ = 84, “ = 93.
        let mut bytes =
            cal("BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Abstimmung Kosten \r\nDTSTART:20260915T080000Z\r\nEND:VEVENT\r\n");
        let at = bytes.windows(7).position(|w| w == b"Kosten ").unwrap() + 7;
        bytes.splice(at..at, [0x84, b'M', 0xFC, b'l', b'l', 0x93, b' ', b'K', 0xE4, b's', b'e', b' ', 0x80]);
        let evs = parse(&bytes, window(), &berlin()).unwrap();
        assert_eq!(evs[0].title, "Abstimmung Kosten „Müll“ Käse €");
        // UTF-16 with its byte order mark (Windows „Unicode“).
        let text = String::from_utf8(cal(
            "BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Übergabe Größe\r\nDTSTART:20260915T080000Z\r\nEND:VEVENT\r\n",
        ))
        .unwrap();
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(parse(&le, window(), &berlin()).unwrap()[0].title, "Übergabe Größe");
        let mut be = vec![0xFE, 0xFF];
        be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(parse(&be, window(), &berlin()).unwrap()[0].title, "Übergabe Größe");
    }

    #[test]
    fn a_web_page_or_other_file_says_how_to_fix_it() {
        let page = parse(b"\n<!DOCTYPE html><html><head><title>Sign in</title>", window(), &berlin());
        let msg = page.unwrap_err().to_string();
        assert!(
            msg.contains("Webseite statt eines Kalenders") || msg.contains("web page instead of a calendar"),
            "{msg}"
        );
        let other = parse(b"Subject,Start Date\nJour fixe,15.09.2026", window(), &berlin()).unwrap_err().to_string();
        assert!(other.contains("kein Kalender") || other.contains("no calendar"), "{other}");
    }

    #[test]
    fn series_with_duration_moved_and_cancelled_instances() {
        // Weekly at 09:00 Berlin, DURATION 45 min; the EXDATE is given in UTC, one instance moved
        // (RECURRENCE-ID in UTC) and one cancelled through an override with STATUS:CANCELLED.
        let body = "BEGIN:VEVENT\r\nUID:s\r\nSUMMARY:Weekly\r\nDTSTART;TZID=Europe/Berlin:20260907T090000\r\nDURATION:PT45M\r\n\
                    RRULE:FREQ=WEEKLY;COUNT=5\r\nEXDATE:20260914T070000Z\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:s\r\nRECURRENCE-ID:20260921T070000Z\r\nSUMMARY:Weekly (verschoben)\r\n\
                    DTSTART;TZID=Europe/Berlin:20260922T140000\r\nDURATION:PT1H\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:s\r\nRECURRENCE-ID;TZID=Europe/Berlin:20260928T090000\r\nSTATUS:CANCELLED\r\n\
                    DTSTART;TZID=Europe/Berlin:20260928T090000\r\nEND:VEVENT\r\n\
                    BEGIN:VEVENT\r\nUID:allday\r\nSUMMARY:Messe\r\nDTSTART;VALUE=DATE:20260915\r\nDURATION:P2D\r\nEND:VEVENT\r\n";
        let evs = parse(&cal(body), window(), &berlin()).unwrap();
        let weekly: Vec<_> = evs.iter().filter(|e| e.uid == "s").map(|e| (e.start, e.end, e.title.as_str())).collect();
        assert_eq!(
            weekly,
            [
                (utc(2026, 9, 7, 7, 0), utc(2026, 9, 7, 7, 45), "Weekly"),
                (utc(2026, 9, 22, 12, 0), utc(2026, 9, 22, 13, 0), "Weekly (verschoben)"),
                (utc(2026, 10, 5, 7, 0), utc(2026, 10, 5, 7, 45), "Weekly"),
            ]
        );
        let fair = evs.iter().find(|e| e.uid == "allday").unwrap();
        assert!(fair.all_day);
        assert_eq!((fair.start, fair.end), (utc(2026, 9, 14, 22, 0), utc(2026, 9, 16, 22, 0)));
    }

    #[tokio::test]
    async fn subscriptions_are_fetched_with_their_etag_and_errors_hide_the_url() {
        use crate::network::tests::testhttp::{Reply, serve};
        use std::sync::Arc;
        let port = serve(Arc::new(|_m: &str, path: &str, headers: &[(String, String)], _b: &[u8]| {
            if path.contains("gone") {
                return Reply::status(403, "");
            }
            if headers.iter().any(|(k, v)| k == "if-none-match" && v == "\"v1\"") {
                return Reply::status(304, "");
            }
            Reply {
                status: 200,
                content_type: "text/calendar".into(),
                body: b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".to_vec(),
                location: None,
                headers: vec![],
            }
        }))
        .await;
        let url = format!("http://127.0.0.1:{port}/cal.ics?token=GEHEIM");
        let Fetched::Data { bytes, .. } = fetch(&url, None).await.unwrap() else { panic!() };
        assert!(bytes.starts_with(b"BEGIN:VCALENDAR"));
        assert_eq!(fetch(&url, Some("\"v1\"")).await.unwrap(), Fetched::Unchanged);
        let err = fetch(&format!("http://127.0.0.1:{port}/gone.ics?token=GEHEIM"), None).await.unwrap_err().to_string();
        assert!(err.contains("403") && !err.contains("GEHEIM"), "{err}");
    }

    #[test]
    fn written_events_read_back_and_edits_keep_the_rest() {
        let start = utc(2026, 10, 20, 8, 0);
        let f = Fields {
            title: Some("Review, Teil 1; mit \\ Rest".into()),
            start: Some(start),
            end: Some(start + Duration::minutes(90)),
            location: Some("Raum 4".into()),
            ..Fields::default()
        };
        let text = new_event("u-1@arcalo", &f, start);
        assert!(text.lines().all(|l| l.len() <= 76), "{text}");
        let evs = parse(text.as_bytes(), window(), &berlin()).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!((evs[0].start, evs[0].end), (start, start + Duration::minutes(90)));
        assert_eq!(evs[0].title, "Review, Teil 1; mit \\ Rest");
        // A server's object with its own properties: they stay, the time moves, SEQUENCE counts.
        let server = cal(&format!(
            "{OUTLOOK_TZ}BEGIN:VEVENT\r\nUID:s1\r\nSEQUENCE:3\r\nX-SERVER-THING;X-P=\"a:b\":keep me\r\n\
             DTSTART;TZID=W. Europe Standard Time:20261020T100000\r\nDTEND;TZID=W. Europe Standard Time:20261020T110000\r\n\
             SUMMARY:Alt\r\nEND:VEVENT\r\n"
        ));
        let moved = edit_event(
            std::str::from_utf8(&server).unwrap(),
            &Fields { start: Some(utc(2026, 10, 21, 12, 0)), title: Some("Neu".into()), ..Fields::default() },
            start,
        )
        .unwrap();
        assert!(moved.contains("X-SERVER-THING;X-P=\"a:b\":keep me"), "{moved}");
        assert!(
            moved.contains("SEQUENCE:4") && moved.contains("DTSTART;TZID=W. Europe Standard Time:20261021T140000"),
            "{moved}"
        );
        let evs = parse(moved.as_bytes(), window(), &Zone::Utc).unwrap();
        assert_eq!(
            (evs[0].start, evs[0].end, evs[0].title.as_str()),
            (utc(2026, 10, 21, 12, 0), utc(2026, 10, 21, 13, 0), "Neu")
        );
        assert_eq!(bounds(&moved).0, Some(utc(2026, 10, 20, 12, 0).timestamp()));
        assert_eq!(bounds("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:r\r\nDTSTART:20260101T100000Z\r\nRRULE:FREQ=DAILY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").1, None);
    }
}
