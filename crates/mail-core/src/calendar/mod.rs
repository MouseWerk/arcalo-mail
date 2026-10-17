//! The calendar: appointments of the user's calendars in one view, as Arcalo's Kalender. Sources
//! are this device (`local`), CalDAV servers ([`caldav`]), a Microsoft account's calendars over
//! Graph ([`graph`]), a JMAP account's calendars where the server offers them ([`jmap`]) and ICS
//! subscriptions (read-only, [`ics::fetch`]).
//!
//! Each calendar keeps its objects as the server has them: iCalendar text (CalDAV, JMAP converted
//! on arrival, subscriptions, this device) or Graph's event JSON. Series are expanded when a
//! range is asked for ([`events`]), in the wall-clock time of their zone ([`ics`]). Changes made
//! here apply at once and wait in `cal_writes` until the server took them ([`sync`]): with the
//! ETag or change key they were based on, so a change made elsewhere meanwhile becomes a conflict
//! the user settles („Server-Version übernehmen“ / „Meine Änderung behalten“). Every change
//! returns an [`Undo`].

pub mod caldav;
pub mod graph;
pub mod ics;
pub mod jmap;
pub mod sync;
pub mod tz;

use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::{Store, now};
use crate::{tr, trf};

pub use ics::Fields;

/// Colors given to new calendars in turn (Arcalo's palette).
pub const PALETTE: [&str; 8] = ["#2563eb", "#0d9488", "#9333ea", "#ea580c", "#db2777", "#65a30d", "#0891b2", "#ca8a04"];

/// How an appointment blocks the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Busy {
    Free,
    Tentative,
    #[default]
    Busy,
    /// Out of office.
    Oof,
    /// Working elsewhere.
    Elsewhere,
}

/// One appointment (one instance of a series) as read from an object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Occurrence {
    pub uid: String,
    /// Original start of an instance of a series (RFC 3339 UTC, or `YYYY-MM-DD` all day); empty
    /// for single appointments.
    pub instance: String,
    pub recurring: bool,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// All-day: `start`/`end` are local midnights, `end` exclusive.
    pub all_day: bool,
    pub title: String,
    pub location: String,
    /// `Name <address>` (or one of them).
    pub organizer: String,
    pub attendees: Vec<String>,
    pub body: Option<String>,
    /// Teams, Zoom, Webex, Google Meet, Jitsi and other meeting links.
    pub link: Option<String>,
    pub busy: Busy,
    pub private: bool,
    pub categories: Vec<String>,
    /// The `TZID` of its start, kept when it is changed.
    pub tzid: Option<String>,
}

/// `2026-09-02T08:00:00Z`: an instant as the id of an instance.
pub fn instant_id(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The name of an attendee entry `Anna Müller <anna@firma.de>`; other entries stay as they are.
pub fn attendee_name(entry: &str) -> &str {
    let e = entry.trim();
    match e.strip_suffix('>').and_then(|rest| rest.rsplit_once(" <")) {
        Some((name, address)) if address.contains('@') && !name.trim().is_empty() => name.trim(),
        _ => e,
    }
}

/// The address of an attendee entry (`Name <a@b>` or `a@b`), lower case.
pub fn attendee_email(entry: &str) -> Option<String> {
    let e = entry.trim();
    let addr = match e.strip_suffix('>').and_then(|rest| rest.rsplit_once('<')) {
        Some((_, a)) => a,
        None => e,
    };
    let addr = addr.trim().trim_start_matches("mailto:").trim_start_matches("MAILTO:");
    addr.contains('@').then(|| addr.to_ascii_lowercase())
}

// ---------------------------------------------------------------------------- meeting links

/// Hosts of online meetings whose links get a „Beitreten“ button (Arcalo's list, with Jitsi's
/// own servers).
const MEETING_HOSTS: [&str; 20] = [
    "teams.microsoft.com",
    "teams.cloud.microsoft",
    "teams.live.com",
    "zoom.us",
    "zoomgov.com",
    "webex.com",
    "meet.google.com",
    "gotomeeting.com",
    "meet.goto.com",
    "gotomeet.me",
    "meet.lync.com",
    "join.skype.com",
    "meet.jit.si",
    "8x8.vc",
    "jitsi.",
    "whereby.com",
    "chime.aws",
    "bluejeans.com",
    "meet.ringcentral.com",
    "zoho.com/meeting",
];

/// The first online-meeting link in `texts`, also behind the link-protection wrappers of
/// corporate mail (Microsoft Safe Links, Proofpoint URL Defense, Google redirects).
pub fn meeting_link<'a>(texts: impl IntoIterator<Item = &'a str>) -> Option<String> {
    for text in texts {
        let mut rest = text;
        while let Some(i) = next_url(rest) {
            let url: String = rest[i..]
                .chars()
                .take_while(|c| !c.is_whitespace() && !matches!(c, '<' | '>' | '"' | '\'' | ')' | ']' | '|'))
                .collect();
            let url = url.trim_end_matches(['.', ',', ';']).replace("&amp;", "&");
            let url = unwrap_protected(&url).unwrap_or(url);
            if is_meeting_url(&url) {
                return Some(url);
            }
            rest = &rest[i + 7..];
        }
    }
    None
}

/// What a meeting link is called on its button: „Teams“, „Zoom“, …, „Online“.
pub fn meeting_kind(url: &str) -> &'static str {
    let host = url.split_once("://").map(|(_, r)| r).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("");
    let host = host.to_ascii_lowercase();
    if host.contains("teams.") {
        "Teams"
    } else if host.contains("zoom") {
        "Zoom"
    } else if host.contains("webex.") {
        "Webex"
    } else if host == "meet.google.com" {
        "Google Meet"
    } else if host.contains("jit.si") || host.starts_with("jitsi.") || host.contains("8x8.vc") {
        "Jitsi"
    } else {
        "Online"
    }
}

fn next_url(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    match (lower.find("https://"), lower.find("http://")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn is_meeting_url(url: &str) -> bool {
    let Some(rest) = url.split_once("://").map(|(_, r)| r) else { return false };
    let lower = rest.to_ascii_lowercase();
    let host = lower.split(['/', '?', '#', ':']).next().unwrap_or("");
    MEETING_HOSTS.iter().any(|h| {
        if let Some(prefix) = h.strip_suffix('.') {
            // A Jitsi server of an organization: `jitsi.example.org`.
            return host.starts_with(&format!("{prefix}.")) && lower[host.len()..].len() > 1;
        }
        match h.split_once('/') {
            Some((h_host, path)) => {
                (host == h_host || host.ends_with(&format!(".{h_host}")))
                    && lower[host.len()..].starts_with(&format!("/{path}"))
            }
            None => host == *h || host.ends_with(&format!(".{h}")),
        }
    })
}

fn unwrap_protected(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let param = |name: &str| {
        query
            .split('&')
            .find_map(|kv| kv.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
            .map(percent_decode)
    };
    if host.ends_with("safelinks.protection.outlook.com") {
        return param("url");
    }
    if (host == "www.google.com" || host == "google.com") && rest.contains("/url?") {
        return param("q").or_else(|| param("url"));
    }
    if host == "urldefense.proofpoint.com" || host == "urldefense.com" {
        if let Some(i) = url.find("/v3/__") {
            let inner = &url[i + 6..];
            let end = inner.find("__").unwrap_or(inner.len());
            return Some(inner[..end].to_owned());
        }
        return param("u").map(|u| percent_decode(&u.replace('_', "/").replace('-', "%")));
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(a), Some(b)) = ((bytes[i + 1] as char).to_digit(16), (bytes[i + 2] as char).to_digit(16))
        {
            out.push((a * 16 + b) as u8);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------- sources and calendars

/// Where a calendar comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// On this device only.
    Local,
    Caldav,
    /// A Microsoft account's calendars (Graph).
    Graph,
    /// A JMAP account's calendars.
    Jmap,
    /// An ICS subscription (read-only); its address lives in the credential store.
    Ics,
}

impl SourceKind {
    fn as_str(self) -> &'static str {
        match self {
            SourceKind::Local => "local",
            SourceKind::Caldav => "caldav",
            SourceKind::Graph => "graph",
            SourceKind::Jmap => "jmap",
            SourceKind::Ics => "ics",
        }
    }

    fn parse(s: &str) -> SourceKind {
        match s {
            "caldav" => SourceKind::Caldav,
            "graph" => SourceKind::Graph,
            "jmap" => SourceKind::Jmap,
            "ics" => SourceKind::Ics,
            _ => SourceKind::Local,
        }
    }
}

/// A source as the settings list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    pub id: i64,
    pub kind: SourceKind,
    pub name: String,
    /// CalDAV: the address to start from; JMAP: the session; ICS: scheme and host only.
    pub url: String,
    pub username: String,
    pub account_id: Option<i64>,
    /// The CalDAV server sends invitations and updates to attendees itself (RFC 6638).
    pub auto_schedule: bool,
    pub last_sync: Option<i64>,
    pub error: Option<String>,
    /// Changes waiting for the server.
    pub pending: u32,
}

/// A calendar of a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Calendar {
    pub id: i64,
    pub source_id: i64,
    pub kind: SourceKind,
    pub source_name: String,
    pub remote_id: String,
    pub name: String,
    pub color: String,
    pub writable: bool,
    pub hidden: bool,
    pub account_id: Option<i64>,
}

/// The credential store name of a source's secret: a CalDAV password, an ICS address.
pub fn secret_key(source_id: i64) -> String {
    format!("calendar-{source_id}-secret")
}

fn source_row(r: &rusqlite::Row) -> rusqlite::Result<Source> {
    Ok(Source {
        id: r.get(0)?,
        kind: SourceKind::parse(&r.get::<_, String>(1)?),
        name: r.get(2)?,
        url: r.get(3)?,
        username: r.get(4)?,
        account_id: r.get(5)?,
        auto_schedule: r.get(6)?,
        last_sync: r.get(7)?,
        error: r.get(8)?,
        pending: r.get(9)?,
    })
}

const SOURCE_COLS: &str = "s.id, s.kind, s.name, s.url, s.username, s.account_id, s.auto_schedule, s.last_sync, s.error,
    (SELECT COUNT(*) FROM cal_writes w JOIN cal_objects o ON o.id = w.object_id JOIN calendars c ON c.id = o.calendar_id WHERE c.source_id = s.id)";

pub fn sources(store: &Store) -> Result<Vec<Source>> {
    let mut st = store.conn().prepare(&format!("SELECT {SOURCE_COLS} FROM cal_sources s ORDER BY s.id"))?;
    let rows = st.query_map([], source_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn source(store: &Store, id: i64) -> Result<Source> {
    store
        .conn()
        .query_row(&format!("SELECT {SOURCE_COLS} FROM cal_sources s WHERE s.id = ?1"), [id], source_row)
        .optional()?
        .ok_or_else(|| Error::NotFound(tr!("Den Kalender gibt es nicht mehr.", "The calendar is gone.").into()))
}

/// Adds a source; returns its id.
pub fn add_source(
    store: &Store,
    kind: SourceKind,
    name: &str,
    url: &str,
    username: &str,
    account_id: Option<i64>,
) -> Result<i64> {
    store.conn().execute(
        "INSERT INTO cal_sources (kind, name, url, username, account_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![kind.as_str(), name.trim(), url.trim(), username.trim(), account_id, now()],
    )?;
    Ok(store.conn().last_insert_rowid())
}

/// The source of an account's calendars (Graph or JMAP), if they are on.
pub fn account_source(store: &Store, account_id: i64) -> Result<Option<i64>> {
    Ok(store
        .conn()
        .query_row(
            "SELECT id FROM cal_sources WHERE account_id = ?1 AND kind IN ('graph', 'jmap')",
            [account_id],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn remove_source(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("DELETE FROM cal_sources WHERE id = ?1", [id])?;
    Ok(())
}

/// Notes the result of a sync.
pub fn synced(store: &Store, source_id: i64, error: Option<&str>, auto_schedule: Option<bool>) -> Result<()> {
    match error {
        None => store.conn().execute(
            "UPDATE cal_sources SET last_sync = ?2, error = NULL, auto_schedule = COALESCE(?3, auto_schedule) WHERE id = ?1",
            params![source_id, now(), auto_schedule],
        )?,
        Some(e) => store.conn().execute("UPDATE cal_sources SET error = ?2 WHERE id = ?1", params![source_id, e])?,
    };
    Ok(())
}

/// The calendar „Auf diesem Gerät“, made when first needed.
pub fn local_calendar(store: &Store) -> Result<i64> {
    if let Some(id) = store
        .conn()
        .query_row(
            "SELECT c.id FROM calendars c JOIN cal_sources s ON s.id = c.source_id WHERE s.kind = 'local' ORDER BY c.id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let source = add_source(store, SourceKind::Local, tr!("Auf diesem Gerät", "On this device"), "", "", None)?;
    upsert_calendar(store, source, "local", tr!("Kalender", "Calendar"), None, true)
}

fn calendar_row(r: &rusqlite::Row) -> rusqlite::Result<Calendar> {
    Ok(Calendar {
        id: r.get(0)?,
        source_id: r.get(1)?,
        kind: SourceKind::parse(&r.get::<_, String>(2)?),
        source_name: r.get(3)?,
        remote_id: r.get(4)?,
        name: r.get(5)?,
        color: r.get(6)?,
        writable: r.get(7)?,
        hidden: r.get(8)?,
        account_id: r.get(9)?,
    })
}

const CALENDAR_COLS: &str =
    "c.id, c.source_id, s.kind, s.name, c.remote_id, c.name, c.color, c.writable, c.hidden, s.account_id";

/// Every calendar, in the order of their sources.
pub fn calendars(store: &Store) -> Result<Vec<Calendar>> {
    let mut st = store.conn().prepare(&format!(
        "SELECT {CALENDAR_COLS} FROM calendars c JOIN cal_sources s ON s.id = c.source_id ORDER BY s.id, c.position, c.id"
    ))?;
    let rows = st.query_map([], calendar_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn calendar(store: &Store, id: i64) -> Result<Calendar> {
    store
        .conn()
        .query_row(
            &format!(
                "SELECT {CALENDAR_COLS} FROM calendars c JOIN cal_sources s ON s.id = c.source_id WHERE c.id = ?1"
            ),
            [id],
            calendar_row,
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(tr!("Den Kalender gibt es nicht mehr.", "The calendar is gone.").into()))
}

/// Adds or updates a calendar a source reported; a color the user chose stays. Returns its id.
pub fn upsert_calendar(
    store: &Store,
    source_id: i64,
    remote_id: &str,
    name: &str,
    color: Option<&str>,
    writable: bool,
) -> Result<i64> {
    let existing: Option<i64> = store
        .conn()
        .query_row(
            "SELECT id FROM calendars WHERE source_id = ?1 AND remote_id = ?2",
            params![source_id, remote_id],
            |r| r.get(0),
        )
        .optional()?;
    let color = color.map(normalize_color).filter(|c| !c.is_empty());
    if let Some(id) = existing {
        store.conn().execute(
            "UPDATE calendars SET name = ?2, writable = ?3, color = CASE WHEN own_color = 0 AND ?4 IS NOT NULL THEN ?4 ELSE color END WHERE id = ?1",
            params![id, name, writable, color],
        )?;
        return Ok(id);
    }
    let count: i64 = store.conn().query_row("SELECT COUNT(*) FROM calendars", [], |r| r.get(0))?;
    let color = color.unwrap_or_else(|| PALETTE[count as usize % PALETTE.len()].to_string());
    store.conn().execute(
        "INSERT INTO calendars (source_id, remote_id, name, color, writable, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![source_id, remote_id, name, color, writable, count],
    )?;
    Ok(store.conn().last_insert_rowid())
}

/// `#RRGGBB` from what servers send (`#RRGGBBAA` from Apple, names are kept out).
fn normalize_color(c: &str) -> String {
    let c = c.trim();
    let hex = c.trim_start_matches('#');
    if hex.len() >= 6 && hex[..6].chars().all(|ch| ch.is_ascii_hexdigit()) {
        format!("#{}", hex[..6].to_ascii_lowercase())
    } else {
        String::new()
    }
}

/// Calendars of a source the server no longer lists are removed with their objects.
pub fn keep_calendars(store: &Store, source_id: i64, remote_ids: &[String]) -> Result<()> {
    for c in calendars(store)?.into_iter().filter(|c| c.source_id == source_id) {
        if !remote_ids.contains(&c.remote_id) {
            store.conn().execute("DELETE FROM calendars WHERE id = ?1", [c.id])?;
        }
    }
    Ok(())
}

/// What the settings change of a calendar: its color, whether it shows, its name.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CalendarPatch {
    pub color: Option<String>,
    pub hidden: Option<bool>,
    pub name: Option<String>,
}

pub fn set_calendar(store: &Store, id: i64, p: &CalendarPatch) -> Result<Calendar> {
    if let Some(c) = &p.color {
        let c = normalize_color(c);
        if c.is_empty() {
            return Err(Error::Invalid(tr!("Diese Farbe kenne ich nicht.", "This color is not known.").into()));
        }
        store.conn().execute("UPDATE calendars SET color = ?2, own_color = 1 WHERE id = ?1", params![id, c])?;
    }
    if let Some(h) = p.hidden {
        store.conn().execute("UPDATE calendars SET hidden = ?2 WHERE id = ?1", params![id, h])?;
    }
    if let Some(n) = p.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        store.conn().execute("UPDATE calendars SET name = ?2 WHERE id = ?1", params![id, n])?;
    }
    calendar(store, id)
}

/// Where new appointments go: the chosen calendar when it can take them, else the first
/// writable one, else this device's.
pub fn default_calendar(store: &Store, chosen: Option<i64>) -> Result<i64> {
    let list = calendars(store)?;
    let usable = |c: &&Calendar| c.writable && c.kind != SourceKind::Ics;
    if let Some(c) = chosen.and_then(|id| list.iter().find(|c| c.id == id)).filter(usable) {
        return Ok(c.id);
    }
    if let Some(c) = list.iter().filter(usable).find(|c| c.kind != SourceKind::Local) {
        return Ok(c.id);
    }
    local_calendar(store)
}

// ---------------------------------------------------------------------------- objects

/// An object as a sync stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub id: i64,
    pub calendar_id: i64,
    pub href: String,
    pub etag: Option<String>,
    pub uid: String,
    /// `ics` or `graph`.
    pub format: String,
    pub data: String,
    pub deleted: bool,
    /// The organizer changed the time of a meeting the user had answered: the answer stays, the
    /// appointment asks for a new one.
    pub reanswer: bool,
}

fn object_row(r: &rusqlite::Row) -> rusqlite::Result<Object> {
    Ok(Object {
        id: r.get(0)?,
        calendar_id: r.get(1)?,
        href: r.get(2)?,
        etag: r.get(3)?,
        uid: r.get(4)?,
        format: r.get(5)?,
        data: r.get(6)?,
        deleted: r.get(7)?,
        reanswer: r.get(8)?,
    })
}

const OBJECT_COLS: &str = "id, calendar_id, href, etag, uid, format, data, deleted, reanswer";

pub fn object(store: &Store, id: i64) -> Result<Object> {
    store
        .conn()
        .query_row(&format!("SELECT {OBJECT_COLS} FROM cal_objects WHERE id = ?1"), [id], object_row)
        .optional()?
        .ok_or_else(|| Error::NotFound(tr!("Den Termin gibt es nicht mehr.", "The appointment is gone.").into()))
}

/// The objects with this UID (an invitation's appointment in any calendar).
pub fn objects_by_uid(store: &Store, uid: &str) -> Result<Vec<Object>> {
    let mut st = store.conn().prepare(&format!("SELECT {OBJECT_COLS} FROM cal_objects WHERE uid = ?1 ORDER BY id"))?;
    let rows = st.query_map([uid], object_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The UID and the bounds of an object's data.
fn describe(format: &str, data: &str) -> (String, Option<i64>, Option<i64>) {
    match format {
        "graph" => {
            let o = graph::occurrence(data);
            let uid = serde_json::from_str::<serde_json::Value>(data)
                .ok()
                .and_then(|v| v["iCalUId"].as_str().map(str::to_owned))
                .unwrap_or_default();
            (uid, o.as_ref().map(|o| o.start.timestamp() - 86_400), o.map(|o| o.end.timestamp() + 86_400))
        }
        _ => {
            let top = ics::parse_components(data.as_bytes());
            let uid = top
                .iter()
                .flat_map(|c| c.children.iter())
                .find(|c| c.name == "VEVENT")
                .and_then(|c| c.text("UID"))
                .unwrap_or_default();
            let (a, b) = ics::bounds(data);
            (uid, a, b)
        }
    }
}

/// The ETags (or change keys) of a calendar's objects by href.
pub fn known_etags(store: &Store, calendar_id: i64) -> Result<Vec<(String, Option<String>)>> {
    let mut st = store.conn().prepare("SELECT href, etag FROM cal_objects WHERE calendar_id = ?1")?;
    let rows = st.query_map([calendar_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn has_write(store: &Store, object_id: i64) -> Result<bool> {
    Ok(store
        .conn()
        .query_row("SELECT COUNT(*) FROM cal_writes WHERE object_id = ?1", [object_id], |r| r.get::<_, i64>(0))?
        > 0)
}

/// Stores an object as the server has it (a sync). An object with a change waiting here keeps
/// the change: the replay finds out whether the server's copy conflicts.
pub fn put_object(
    store: &Store,
    calendar_id: i64,
    href: &str,
    etag: Option<&str>,
    format: &str,
    data: &str,
) -> Result<()> {
    let existing: Option<i64> = store
        .conn()
        .query_row("SELECT id FROM cal_objects WHERE calendar_id = ?1 AND href = ?2", params![calendar_id, href], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(id) = existing
        && has_write(store, id)?
    {
        return Ok(());
    }
    let (uid, first, last) = describe(format, data);
    store.conn().execute(
        "INSERT INTO cal_objects (calendar_id, href, etag, uid, format, data, first_start, last_end)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (calendar_id, href) DO UPDATE SET etag = excluded.etag, uid = excluded.uid, format = excluded.format,
           data = excluded.data, first_start = excluded.first_start, last_end = excluded.last_end, deleted = 0",
        params![calendar_id, href, etag, uid, format, data, first, last],
    )?;
    Ok(())
}

/// Removes an object the server no longer has (unless a change waits for it here).
pub fn remove_object(store: &Store, calendar_id: i64, href: &str) -> Result<()> {
    store.conn().execute(
        "DELETE FROM cal_objects WHERE calendar_id = ?1 AND href = ?2 AND id NOT IN (SELECT object_id FROM cal_writes)",
        params![calendar_id, href],
    )?;
    Ok(())
}

fn set_data(store: &Store, id: i64, data: &str) -> Result<()> {
    let format: String = store.conn().query_row("SELECT format FROM cal_objects WHERE id = ?1", [id], |r| r.get(0))?;
    let (uid, first, last) = describe(&format, data);
    store.conn().execute(
        "UPDATE cal_objects SET data = ?2, uid = CASE WHEN ?3 = '' THEN uid ELSE ?3 END, first_start = ?4, last_end = ?5 WHERE id = ?1",
        params![id, data, uid, first, last],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------- the view

/// Whether and how an appointment can be changed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Editability {
    Yes,
    /// A meeting the user organizes: a change goes to its attendees after a confirmation.
    Confirm,
    /// Someone else's meeting: only the answer is the user's.
    Received,
    /// An instance of a series (changed in the app that made the series).
    Series,
    /// A calendar the user may only read.
    ReadOnly,
    /// A subscription.
    Ics,
}

/// A change waiting for the server, as the view shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WriteState {
    /// `create`, `update`, `delete`.
    pub op: String,
    /// `pending`, `conflict`, `refused`.
    pub state: String,
    pub error: Option<String>,
    /// A conflict: what the server has now.
    pub server: Option<Fields>,
    /// What was changed here.
    pub mine: Option<Fields>,
    pub send: bool,
}

/// An appointment as the calendar view shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CalendarEvent {
    /// `<calendar>|<object>|<instance>`.
    pub key: String,
    pub calendar_id: i64,
    pub object_id: i64,
    pub uid: String,
    pub instance: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub location: String,
    pub organizer: String,
    pub attendees: Vec<String>,
    pub body: Option<String>,
    pub link: Option<String>,
    /// „Teams“, „Zoom“, … for the „Beitreten“ button.
    pub link_kind: Option<String>,
    pub busy: Busy,
    pub private: bool,
    pub recurring: bool,
    pub categories: Vec<String>,
    pub editable: Editability,
    pub write: Option<WriteState>,
    /// The organizer moved the meeting since the user answered (see [`Object::reanswer`]).
    pub reanswer: bool,
}

/// The parts of a key.
pub fn parse_key(key: &str) -> Option<(i64, i64, String)> {
    let mut it = key.splitn(3, '|');
    let cal = it.next()?.parse().ok()?;
    let obj = it.next()?.parse().ok()?;
    Some((cal, obj, it.next().unwrap_or_default().to_string()))
}

/// The occurrences of an object in a window.
pub fn occurrences(o: &Object, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Occurrence> {
    match o.format.as_str() {
        "graph" => graph::occurrence(&o.data).into_iter().filter(|x| x.start < to && x.end > from).collect(),
        _ => ics::parse(o.data.as_bytes(), (from, to), &tz::Zone::Local).unwrap_or_default(),
    }
}

/// How an occurrence of an object in `cal` may be changed; `me`: the user's addresses.
fn editability(cal: &Calendar, o: &Object, x: &Occurrence, me: &[String]) -> Editability {
    if cal.kind == SourceKind::Ics {
        return Editability::Ics;
    }
    if !cal.writable {
        return Editability::ReadOnly;
    }
    if o.format == "graph" {
        return graph::editability(&o.data);
    }
    if x.recurring {
        return Editability::Series;
    }
    let mine = |entry: &str| attendee_email(entry).is_some_and(|a| me.iter().any(|m| m.eq_ignore_ascii_case(&a)));
    let organizer = attendee_email(&x.organizer);
    if organizer.is_some() && !mine(&x.organizer) {
        return Editability::Received;
    }
    let others = x.attendees.iter().filter(|a| !mine(a) && attendee_email(a) != organizer).count();
    if others > 0 { Editability::Confirm } else { Editability::Yes }
}

/// The fields of a write as stored.
fn fields_of(json: Option<String>) -> Option<Fields> {
    json.and_then(|j| serde_json::from_str(&j).ok())
}

fn write_state(store: &Store, object_id: i64) -> Result<Option<WriteState>> {
    Ok(store
        .conn()
        .query_row(
            "SELECT op, state, error, server, mine, send FROM cal_writes WHERE object_id = ?1",
            [object_id],
            |r| {
                let server: Option<String> = r.get(3)?;
                Ok(WriteState {
                    op: r.get(0)?,
                    state: r.get(1)?,
                    error: r.get(2)?,
                    server: server
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                        .and_then(|v| serde_json::from_value(v["fields"].clone()).ok()),
                    mine: fields_of(r.get(4)?),
                    send: r.get(5)?,
                })
            },
        )
        .optional()?)
}

/// The key of an occurrence. A subscription keeps all of its appointments in one object, so its
/// keys also name the UID.
fn key_of(cal: &Calendar, o: &Object, x: &Occurrence) -> String {
    if cal.kind == SourceKind::Ics {
        format!("{}|{}|{}|{}", cal.id, o.id, x.instance, x.uid)
    } else {
        format!("{}|{}|{}", cal.id, o.id, x.instance)
    }
}

fn view(cal: &Calendar, o: &Object, x: Occurrence, me: &[String], write: Option<WriteState>) -> CalendarEvent {
    let editable = editability(cal, o, &x, me);
    CalendarEvent {
        key: key_of(cal, o, &x),
        calendar_id: cal.id,
        object_id: o.id,
        uid: x.uid,
        instance: x.instance,
        title: x.title,
        start: x.start,
        end: x.end,
        all_day: x.all_day,
        location: x.location,
        organizer: x.organizer,
        attendees: x.attendees,
        body: x.body,
        link_kind: x.link.as_deref().map(|l| meeting_kind(l).to_string()),
        link: x.link,
        busy: x.busy,
        private: x.private,
        recurring: x.recurring,
        categories: x.categories,
        editable,
        write,
        reanswer: o.reanswer,
    }
}

/// The appointments of the shown calendars between `from` and `to`, by start. `me`: the user's
/// addresses (which meetings are the user's own). `all`: also hidden calendars (conflicts of an
/// invitation look at every calendar).
pub fn events(
    store: &Store,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    me: &[String],
    all: bool,
) -> Result<Vec<CalendarEvent>> {
    let cals = calendars(store)?;
    let mut st = store.conn().prepare(&format!(
        "SELECT {OBJECT_COLS} FROM cal_objects WHERE deleted = 0
           AND (first_start IS NULL OR first_start < ?2) AND (last_end IS NULL OR last_end > ?1)"
    ))?;
    let objects: Vec<Object> =
        st.query_map(params![from.timestamp(), to.timestamp()], object_row)?.collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for o in objects {
        let Some(cal) = cals.iter().find(|c| c.id == o.calendar_id) else { continue };
        if cal.hidden && !all {
            continue;
        }
        let write = write_state(store, o.id)?;
        for x in occurrences(&o, from, to) {
            out.push(view(cal, &o, x, me, write.clone()));
        }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.title.cmp(&b.title)));
    Ok(out)
}

/// One appointment by its key.
pub fn event(store: &Store, key: &str, me: &[String]) -> Result<CalendarEvent> {
    let gone = || Error::NotFound(tr!("Den Termin gibt es nicht mehr.", "The appointment is gone.").into());
    let (cal_id, obj_id, rest) = parse_key(key).ok_or_else(gone)?;
    let (instance, uid) = match rest.split_once('|') {
        Some((i, u)) => (i.to_string(), Some(u.to_string())),
        None => (rest, None),
    };
    let o = object(store, obj_id)?;
    let cal = calendar(store, cal_id)?;
    let far = (Utc.with_ymd_and_hms(1900, 1, 1, 0, 0, 0).unwrap(), Utc.with_ymd_and_hms(2200, 1, 1, 0, 0, 0).unwrap());
    let same_uid = |x: &Occurrence| uid.as_deref().is_none_or(|u| u == x.uid);
    let x = if instance.is_empty() {
        occurrences(&o, far.0, far.1).into_iter().find(|x| same_uid(x))
    } else {
        let at = DateTime::parse_from_rfc3339(&instance).map(|t| t.with_timezone(&Utc)).ok();
        let (a, b) = match at {
            Some(t) => (t - Duration::days(400), t + Duration::days(400)),
            None => far,
        };
        occurrences(&o, a, b).into_iter().find(|x| x.instance == instance && same_uid(x))
    }
    .ok_or_else(gone)?;
    Ok(view(&cal, &o, x, me, write_state(store, o.id)?))
}

// ---------------------------------------------------------------------------- changes

/// What takes a change back: the object and its data before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Undo {
    pub object_id: i64,
    /// `created`, `changed`, `deleted`.
    pub kind: String,
    /// The data before a change.
    pub before: Option<String>,
    /// A deleted appointment: where it was, so it can be made again after the server took the
    /// delete (the object is gone then).
    #[serde(default)]
    pub calendar_id: Option<i64>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    /// The change went to a meeting's attendees: taking it back goes to them too.
    #[serde(default)]
    pub send: bool,
}

/// Rounds in which the server answered and a write still failed before it is parked (shown as
/// refused, with „Verwerfen“), so it no longer comes first in every round.
pub const MAX_WRITE_ATTEMPTS: u32 = 5;

/// Queues a write for an object, merged with one already waiting. Every merge counts up the
/// write's `version`, so a round that sent an earlier state leaves the newer one queued.
fn queue(store: &Store, object_id: i64, op: &str, mine: Option<&Fields>, send: bool) -> Result<()> {
    let existing: Option<(i64, String, Option<String>, bool)> = store
        .conn()
        .query_row("SELECT id, op, mine, send FROM cal_writes WHERE object_id = ?1", [object_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()?;
    let Some((id, old_op, old_mine, old_send)) = existing else {
        store.conn().execute(
            "INSERT INTO cal_writes (object_id, op, mine, send, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![object_id, op, mine.map(serde_json::to_string).transpose()?, send, now()],
        )?;
        return Ok(());
    };
    match (old_op.as_str(), op) {
        // Never on the server: gone for good.
        ("create", "delete") => {
            store.conn().execute("DELETE FROM cal_objects WHERE id = ?1", [object_id])?;
        }
        ("create", _) => {
            store.conn().execute(
                "UPDATE cal_writes SET state = 'pending', error = NULL, strikes = 0, version = version + 1 WHERE id = ?1",
                [id],
            )?;
        }
        // What waited before the delete is kept for „Rückgängig“.
        (_, "delete") => {
            store.conn().execute(
                "UPDATE cal_writes SET prev_op = CASE WHEN op = 'delete' THEN prev_op ELSE op END,
                   prev_mine = CASE WHEN op = 'delete' THEN prev_mine ELSE mine END,
                   prev_send = CASE WHEN op = 'delete' THEN prev_send ELSE send END,
                   op = 'delete', state = 'pending', error = NULL, server = NULL, strikes = 0, version = version + 1
                 WHERE id = ?1",
                [id],
            )?;
        }
        _ => {
            let merged = match (fields_of(old_mine), mine) {
                (Some(a), Some(b)) => Some(a.merged(b)),
                (a, b) => b.cloned().or(a),
            };
            store.conn().execute(
                "UPDATE cal_writes SET op = ?2, mine = ?3, send = ?4, state = CASE WHEN state = 'conflict' THEN state ELSE 'pending' END, error = NULL,
                   strikes = 0, version = version + 1, prev_op = NULL, prev_mine = NULL, prev_send = 0 WHERE id = ?1",
                params![id, if old_op == "delete" { "update" } else { old_op.as_str() }, merged.map(|m| serde_json::to_string(&m)).transpose()?, old_send || send],
            )?;
        }
    }
    Ok(())
}

/// A new UID for an appointment made here.
pub fn new_uid() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}@arcalo-mail", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Makes a new appointment in a calendar; it goes to the server with the next sync.
pub fn create(store: &Store, calendar_id: i64, f: &Fields, me: &[String]) -> Result<(CalendarEvent, Undo)> {
    let cal = calendar(store, calendar_id)?;
    if !cal.writable || cal.kind == SourceKind::Ics {
        return Err(Error::Invalid(
            tr!("In diesen Kalender können keine Termine eingetragen werden.", "This calendar takes no appointments.")
                .into(),
        ));
    }
    let start = f.start.ok_or_else(|| Error::Invalid(tr!("Der Beginn fehlt.", "The start is missing.").into()))?;
    let end = f.end.unwrap_or(start + Duration::minutes(30));
    if end < start {
        return Err(Error::Invalid(tr!("Das Ende liegt vor dem Beginn.", "The end is before the start.").into()));
    }
    let title =
        f.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(tr!("Neuer Termin", "New appointment"));
    let f = Fields { title: Some(title.to_string()), end: Some(end), ..f.clone() };
    let uid = new_uid();
    let (format, data, href) = if cal.kind == SourceKind::Graph {
        ("graph", graph::new_event_json(&uid, &f), format!("local:{uid}"))
    } else {
        ("ics", ics::new_event(&uid, &f, Utc::now()), href_for(&cal, &uid))
    };
    let id = store.batch(|| {
        let (u, first, last) = describe(format, &data);
        store.conn().execute(
            "INSERT INTO cal_objects (calendar_id, href, uid, format, data, first_start, last_end) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![calendar_id, href, if u.is_empty() { uid.clone() } else { u }, format, data, first, last],
        )?;
        let id = store.conn().last_insert_rowid();
        if cal.kind != SourceKind::Local {
            queue(store, id, "create", Some(&f), false)?;
        }
        Ok(id)
    })?;
    let ev = first_event(store, id, me)?;
    Ok((
        ev,
        Undo {
            object_id: id,
            kind: "created".into(),
            before: None,
            calendar_id: None,
            uid: None,
            format: None,
            send: false,
        },
    ))
}

/// The first appointment of an object (a single one: itself).
fn first_event(store: &Store, object_id: i64, me: &[String]) -> Result<CalendarEvent> {
    let o = object(store, object_id)?;
    let cal = calendar(store, o.calendar_id)?;
    let far = (Utc.with_ymd_and_hms(1900, 1, 1, 0, 0, 0).unwrap(), Utc.with_ymd_and_hms(2200, 1, 1, 0, 0, 0).unwrap());
    let x = occurrences(&o, far.0, far.1)
        .into_iter()
        .next()
        .ok_or_else(|| Error::NotFound(tr!("Den Termin gibt es nicht mehr.", "The appointment is gone.").into()))?;
    Ok(view(&cal, &o, x, me, write_state(store, o.id)?))
}

/// The data of an object with `f` applied.
fn edited(o: &Object, f: &Fields) -> Result<String> {
    let out = match o.format.as_str() {
        "graph" => graph::edit_json(&o.data, f),
        _ => ics::edit_event(&o.data, f, Utc::now()),
    };
    out.ok_or_else(|| {
        Error::Invalid(tr!("Dieser Termin lässt sich nicht ändern.", "This appointment cannot be changed.").into())
    })
}

/// Changes an appointment (title, times, place). `send`: the user confirmed that a meeting's
/// attendees get the update.
pub fn update(store: &Store, key: &str, f: &Fields, send: bool, me: &[String]) -> Result<(CalendarEvent, Undo)> {
    let ev = event(store, key, me)?;
    match ev.editable {
        Editability::Yes => {}
        Editability::Confirm if send => {}
        Editability::Confirm => {
            return Err(Error::Invalid(
                tr!(
                    "Die Teilnehmenden bekommen die Änderung erst nach deiner Bestätigung.",
                    "The attendees get the change only after you confirm it."
                )
                .into(),
            ));
        }
        _ => return Err(read_only(ev.editable)),
    }
    if let Some(e) = f.end
        && e < f.start.unwrap_or(ev.start)
    {
        return Err(Error::Invalid(tr!("Das Ende liegt vor dem Beginn.", "The end is before the start.").into()));
    }
    let o = object(store, ev.object_id)?;
    let data = edited(&o, f)?;
    let cal = calendar(store, o.calendar_id)?;
    store.batch(|| {
        set_data(store, o.id, &data)?;
        if cal.kind != SourceKind::Local {
            queue(store, o.id, "update", Some(f), send)?;
        }
        Ok(())
    })?;
    let confirmed = send && ev.editable == Editability::Confirm;
    let ev = event(store, &format!("{}|{}|{}", o.calendar_id, o.id, ev.instance), me)?;
    Ok((
        ev,
        Undo {
            object_id: o.id,
            kind: "changed".into(),
            before: Some(o.data),
            calendar_id: None,
            uid: None,
            format: None,
            send: confirmed,
        },
    ))
}

fn read_only(e: Editability) -> Error {
    Error::Invalid(
        match e {
            Editability::Received => tr!(
                "Diesen Termin hat jemand anderes eingeladen; du kannst nur zu- oder absagen.",
                "Someone else invited to this appointment; you can only accept or decline."
            ),
            Editability::Series => tr!(
                "Termine einer Serie änderst du in der App, die die Serie angelegt hat.",
                "Appointments of a series are changed in the app that made the series."
            ),
            Editability::Ics => {
                tr!("Ein abonnierter Kalender lässt sich nur lesen.", "A subscribed calendar is read-only.")
            }
            _ => tr!("Diesen Kalender darfst du nur lesen.", "You may only read this calendar."),
        }
        .into(),
    )
}

/// Deletes an appointment (a single one the user owns).
pub fn delete(store: &Store, key: &str, me: &[String]) -> Result<Undo> {
    let ev = event(store, key, me)?;
    if ev.editable != Editability::Yes {
        return Err(read_only(ev.editable));
    }
    let o = object(store, ev.object_id)?;
    let cal = calendar(store, o.calendar_id)?;
    store.batch(|| {
        if cal.kind == SourceKind::Local {
            store.conn().execute("UPDATE cal_objects SET deleted = 1 WHERE id = ?1", [o.id])?;
        } else {
            store.conn().execute("UPDATE cal_objects SET deleted = 1 WHERE id = ?1", [o.id])?;
            queue(store, o.id, "delete", None, false)?;
        }
        Ok(())
    })?;
    Ok(Undo {
        object_id: o.id,
        kind: "deleted".into(),
        before: Some(o.data),
        calendar_id: Some(o.calendar_id),
        uid: Some(o.uid),
        format: Some(o.format),
        send: false,
    })
}

/// Takes a change back; returns the appointment as it is again (`None` after taking back a new one).
pub fn undo(store: &Store, u: &Undo, me: &[String]) -> Result<Option<CalendarEvent>> {
    let o = match object(store, u.object_id) {
        Ok(o) => o,
        // A new appointment that never reached the server is gone with its delete.
        Err(_) if u.kind == "created" => return Ok(None),
        // Deleted and gone from the server already: made again from what it was.
        Err(_) if u.kind == "deleted" => {
            let (Some(cal), Some(uid), Some(data)) = (u.calendar_id, u.uid.as_deref(), u.before.as_deref()) else {
                return Err(Error::NotFound(
                    crate::tr!("Den Termin gibt es nicht mehr.", "The appointment no longer exists.").into(),
                ));
            };
            let data = match u.format.as_deref() {
                Some("graph") => {
                    let f = fields_of_data("graph", data).ok_or_else(|| {
                        Error::Invalid(
                            crate::tr!("Der Termin lässt sich nicht lesen.", "The appointment cannot be read.").into(),
                        )
                    })?;
                    ics::new_event(uid, &f, Utc::now())
                }
                _ => data.to_string(),
            };
            let id = insert_internal(store, cal, uid, &data)?;
            return Ok(first_event(store, id, me).ok());
        }
        Err(e) => return Err(e),
    };
    let cal = calendar(store, o.calendar_id)?;
    let local = cal.kind == SourceKind::Local;
    let op: Option<String> =
        store.conn().query_row("SELECT op FROM cal_writes WHERE object_id = ?1", [o.id], |r| r.get(0)).optional()?;
    match u.kind.as_str() {
        "created" => {
            store.batch(|| {
                if local || op.as_deref() == Some("create") {
                    store.conn().execute("DELETE FROM cal_objects WHERE id = ?1", [o.id])?;
                } else {
                    store.conn().execute("UPDATE cal_objects SET deleted = 1 WHERE id = ?1", [o.id])?;
                    queue(store, o.id, "delete", None, false)?;
                }
                Ok(())
            })?;
            Ok(None)
        }
        "deleted" => {
            store.batch(|| {
                store.conn().execute("UPDATE cal_objects SET deleted = 0 WHERE id = ?1", [o.id])?;
                if let Some(before) = &u.before {
                    set_data(store, o.id, before)?;
                }
                match op.as_deref() {
                    // Not on its way yet: nothing happened on the server; a change that waited
                    // before the delete waits again.
                    Some("delete") if !local => {
                        let (sent, prev): (bool, Option<String>) = store.conn().query_row(
                            "SELECT attempts > 0, prev_op FROM cal_writes WHERE object_id = ?1",
                            [o.id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )?;
                        match prev {
                            Some(prev) => {
                                store.conn().execute(
                                    "UPDATE cal_writes SET op = ?2, mine = prev_mine, send = prev_send, prev_op = NULL,
                                       prev_mine = NULL, prev_send = 0, state = 'pending', error = NULL, server = NULL,
                                       strikes = 0, version = version + 1
                                     WHERE object_id = ?1",
                                    params![o.id, prev],
                                )?;
                            }
                            None => {
                                store.conn().execute("DELETE FROM cal_writes WHERE object_id = ?1", [o.id])?;
                                if sent {
                                    queue(store, o.id, "create", None, false)?;
                                }
                            }
                        }
                    }
                    None if !local => {
                        // Already deleted on the server: made again there.
                        store.conn().execute("UPDATE cal_objects SET etag = NULL WHERE id = ?1", [o.id])?;
                        queue(store, o.id, "create", None, false)?;
                    }
                    _ => {}
                }
                Ok(())
            })?;
            Ok(first_event(store, o.id, me).ok())
        }
        _ => {
            let mut before = u.before.clone().unwrap_or_default();
            // The attendees had the change: the old state goes to them as a newer version.
            if u.send && o.format == "ics" {
                let next = ics::sequence(&o.data).max(ics::sequence(&before)) + 1;
                before = ics::with_sequence(&before, next).unwrap_or(before);
            }
            store.batch(|| {
                set_data(store, o.id, &before)?;
                if !local {
                    queue(store, o.id, "update", None, u.send)?;
                }
                Ok(())
            })?;
            Ok(first_event(store, o.id, me).ok())
        }
    }
}

// ---------------------------------------------------------------------------- writes

/// A change waiting for the server.
#[derive(Debug, Clone, PartialEq)]
pub struct Write {
    pub id: i64,
    pub object: Object,
    pub op: String,
    pub send: bool,
    pub attempts: u32,
    /// The merges the write had when it was read (see [`queue`]).
    pub version: i64,
}

/// The writes of a source that can go now (not the conflicts the user settles first).
pub fn pending_writes(store: &Store, source_id: i64) -> Result<Vec<Write>> {
    let mut st = store.conn().prepare(
        "SELECT w.id, w.op, w.send, w.attempts, w.object_id, w.version FROM cal_writes w
           JOIN cal_objects o ON o.id = w.object_id JOIN calendars c ON c.id = o.calendar_id
          WHERE c.source_id = ?1 AND w.state = 'pending' ORDER BY w.id",
    )?;
    let rows: Vec<(i64, String, bool, u32, i64, i64)> = st
        .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = vec![];
    for (id, op, send, attempts, object_id, version) in rows {
        out.push(Write { id, object: object(store, object_id)?, op, send, attempts, version });
    }
    Ok(out)
}

/// The server took a write: the object's new name, ETag and data (when the server changed it).
/// A change made here while the write was on its way stays queued, based on the new ETag.
pub fn write_done(store: &Store, w: &Write, href: Option<&str>, etag: Option<&str>, data: Option<&str>) -> Result<()> {
    store.batch(|| {
        let now: Option<(i64, String)> = store
            .conn()
            .query_row("SELECT version, op FROM cal_writes WHERE id = ?1", [w.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if let Some((_, op)) = now.filter(|(v, _)| *v != w.version) {
            if w.op == "delete" {
                // Gone on the server, wanted again here: made there again.
                store.conn().execute("UPDATE cal_objects SET etag = NULL WHERE id = ?1", [w.object.id])?;
                store.conn().execute("UPDATE cal_writes SET op = 'create', state = 'pending' WHERE id = ?1", [w.id])?;
                return Ok(());
            }
            if let Some(h) = href {
                store.conn().execute("UPDATE cal_objects SET href = ?2 WHERE id = ?1", params![w.object.id, h])?;
            }
            store.conn().execute("UPDATE cal_objects SET etag = ?2 WHERE id = ?1", params![w.object.id, etag])?;
            if w.op == "create" && op == "create" {
                store.conn().execute("UPDATE cal_writes SET op = 'update' WHERE id = ?1", [w.id])?;
            }
            return Ok(());
        }
        store.conn().execute("DELETE FROM cal_writes WHERE id = ?1", [w.id])?;
        if w.op == "delete" {
            store.conn().execute("DELETE FROM cal_objects WHERE id = ?1", [w.object.id])?;
            return Ok(());
        }
        if let Some(h) = href {
            store.conn().execute("UPDATE cal_objects SET href = ?2 WHERE id = ?1", params![w.object.id, h])?;
        }
        store.conn().execute("UPDATE cal_objects SET etag = ?2 WHERE id = ?1", params![w.object.id, etag])?;
        if let Some(d) = data {
            set_data(store, w.object.id, d)?;
        }
        Ok(())
    })
}

/// A write failed for now (offline, the server's trouble); it is tried again.
pub fn write_failed(store: &Store, w: &Write, error: &str) -> Result<()> {
    store
        .conn()
        .execute("UPDATE cal_writes SET attempts = attempts + 1, error = ?2 WHERE id = ?1", params![w.id, error])?;
    Ok(())
}

/// A write failed in a round in which the server answered otherwise: after
/// [`MAX_WRITE_ATTEMPTS`] such rounds it is parked as refused (with its last error) and waits
/// for the user.
pub fn write_struck(store: &Store, w: &Write) -> Result<()> {
    store.conn().execute(
        "UPDATE cal_writes SET strikes = strikes + 1,
           state = CASE WHEN strikes + 1 >= ?2 AND state = 'pending' THEN 'refused' ELSE state END
         WHERE id = ?1",
        params![w.id, MAX_WRITE_ATTEMPTS],
    )?;
    Ok(())
}

/// The server refused a write for good (no permission, a broken object).
pub fn write_refused(store: &Store, w: &Write, error: &str) -> Result<()> {
    store.conn().execute(
        "UPDATE cal_writes SET state = 'refused', attempts = attempts + 1, error = ?2 WHERE id = ?1",
        params![w.id, error],
    )?;
    Ok(())
}

/// The fields an object's data says (for the conflict's two sides).
pub fn fields_of_data(format: &str, data: &str) -> Option<Fields> {
    let far = (Utc.with_ymd_and_hms(1900, 1, 1, 0, 0, 0).unwrap(), Utc.with_ymd_and_hms(2200, 1, 1, 0, 0, 0).unwrap());
    let o = Object {
        id: 0,
        calendar_id: 0,
        href: String::new(),
        etag: None,
        uid: String::new(),
        format: format.into(),
        data: data.into(),
        deleted: false,
        reanswer: false,
    };
    let x = occurrences(&o, far.0, far.1).into_iter().next()?;
    Some(Fields {
        title: Some(x.title),
        start: Some(x.start),
        end: Some(x.end),
        location: Some(x.location),
        all_day: Some(x.all_day),
        description: None,
    })
}

/// Someone changed the object on the server since it was read here (`data` and `etag`: the
/// server's copy now; `None`: deleted there). The user decides.
pub fn write_conflict(store: &Store, w: &Write, data: Option<&str>, etag: Option<&str>) -> Result<()> {
    let fields = data.and_then(|d| fields_of_data(&w.object.format, d));
    let server = serde_json::json!({ "data": data, "etag": etag, "fields": fields });
    store.conn().execute(
        "UPDATE cal_writes SET state = 'conflict', server = ?2, error = NULL, attempts = attempts + 1 WHERE id = ?1",
        params![w.id, server.to_string()],
    )?;
    Ok(())
}

/// Settles a conflict: keep the change made here (written over the server's copy), or take the
/// server's version.
pub fn resolve(store: &Store, key: &str, keep_mine: bool) -> Result<()> {
    let (_, object_id, _) = parse_key(key).ok_or_else(|| {
        Error::NotFound(crate::tr!("Den Konflikt gibt es nicht mehr.", "The conflict no longer exists.").into())
    })?;
    let o = object(store, object_id)?;
    let row: Option<(String, Option<String>, Option<String>)> = store
        .conn()
        .query_row("SELECT op, server, mine FROM cal_writes WHERE object_id = ?1 AND state = 'conflict'", [o.id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let Some((op, server, mine)) = row else { return Ok(()) };
    let server: serde_json::Value = server.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let data = server["data"].as_str().map(str::to_owned);
    let etag = server["etag"].as_str().map(str::to_owned);
    store.batch(|| {
        if keep_mine {
            match &data {
                // The change applied to the server's copy, written with its ETag.
                Some(d) if op != "delete" => {
                    let mut base = o.clone();
                    base.data = d.clone();
                    let next = match fields_of(mine.clone()) {
                        Some(f) => edited(&base, &f).unwrap_or_else(|_| o.data.clone()),
                        None => o.data.clone(),
                    };
                    set_data(store, o.id, &next)?;
                    store.conn().execute("UPDATE cal_objects SET etag = ?2 WHERE id = ?1", params![o.id, etag])?;
                }
                // Deleted there: made again.
                None if op != "delete" => {
                    store.conn().execute("UPDATE cal_objects SET etag = NULL WHERE id = ?1", [o.id])?;
                    store.conn().execute("UPDATE cal_writes SET op = 'create' WHERE object_id = ?1", [o.id])?;
                }
                _ => {
                    store.conn().execute("UPDATE cal_objects SET etag = ?2 WHERE id = ?1", params![o.id, etag])?;
                }
            }
            store.conn().execute(
                "UPDATE cal_writes SET state = 'pending', server = NULL, error = NULL, strikes = 0, version = version + 1
                 WHERE object_id = ?1",
                [o.id],
            )?;
        } else {
            store.conn().execute("DELETE FROM cal_writes WHERE object_id = ?1", [o.id])?;
            match &data {
                Some(d) => {
                    store
                        .conn()
                        .execute("UPDATE cal_objects SET etag = ?2, deleted = 0 WHERE id = ?1", params![o.id, etag])?;
                    set_data(store, o.id, d)?;
                }
                None => {
                    store.conn().execute("DELETE FROM cal_objects WHERE id = ?1", [o.id])?;
                }
            }
        }
        Ok(())
    })
}

/// Drops a change that failed or that the server refused: the appointment shows as the server
/// has it after the next sync.
pub fn discard(store: &Store, key: &str) -> Result<()> {
    let (_, object_id, _) = parse_key(key).ok_or_else(|| {
        Error::NotFound(crate::tr!("Den Konflikt gibt es nicht mehr.", "The conflict no longer exists.").into())
    })?;
    let o = object(store, object_id)?;
    let op: Option<String> =
        store.conn().query_row("SELECT op FROM cal_writes WHERE object_id = ?1", [o.id], |r| r.get(0)).optional()?;
    store.batch(|| {
        store.conn().execute("DELETE FROM cal_writes WHERE object_id = ?1", [o.id])?;
        if op.as_deref() == Some("create") {
            store.conn().execute("DELETE FROM cal_objects WHERE id = ?1", [o.id])?;
        } else {
            // Read again at the next sync.
            store.conn().execute("UPDATE cal_objects SET etag = NULL, deleted = 0 WHERE id = ?1", [o.id])?;
            store
                .conn()
                .execute("UPDATE calendars SET sync_state = NULL, ctag = NULL WHERE id = ?1", [o.calendar_id])?;
        }
        Ok(())
    })
}

/// The sync state of a calendar (CalDAV sync token, Graph's delta link, JMAP state, ICS ETag)
/// and its CTag.
pub fn sync_state(store: &Store, calendar_id: i64) -> Result<(Option<String>, Option<String>)> {
    Ok(store.conn().query_row("SELECT sync_state, ctag FROM calendars WHERE id = ?1", [calendar_id], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?)
}

pub fn set_sync_state(store: &Store, calendar_id: i64, state: Option<&str>, ctag: Option<&str>) -> Result<()> {
    store
        .conn()
        .execute("UPDATE calendars SET sync_state = ?2, ctag = ?3 WHERE id = ?1", params![calendar_id, state, ctag])?;
    Ok(())
}

/// The user's own addresses (which meetings the user organizes or is invited to): the accounts'
/// addresses, then the ones calendar servers and JMAP identities name.
pub fn my_addresses(store: &Store) -> Result<Vec<String>> {
    let mut out: Vec<String> = store.accounts()?.into_iter().map(|a| a.email.to_ascii_lowercase()).collect();
    let mut st = store.conn().prepare("SELECT address FROM user_addresses ORDER BY id")?;
    for a in st.query_map([], |r| r.get::<_, String>(0))? {
        let a = a?.trim().to_ascii_lowercase();
        if a.contains('@') && !out.contains(&a) {
            out.push(a);
        }
    }
    Ok(out)
}

/// Where further addresses of the user come from.
#[derive(Debug, Clone, Copy)]
pub enum AddressOrigin {
    /// A CalDAV server's `calendar-user-address-set`.
    Source(i64),
    /// A JMAP account's sender identities.
    Account(i64),
}

/// Replaces the addresses one origin names.
pub fn set_user_addresses(store: &Store, origin: AddressOrigin, addresses: &[String]) -> Result<()> {
    let (source, account) = match origin {
        AddressOrigin::Source(id) => (Some(id), None),
        AddressOrigin::Account(id) => (None, Some(id)),
    };
    store.batch(|| {
        store.conn().execute(
            "DELETE FROM user_addresses WHERE cal_source_id IS ?1 AND account_id IS ?2",
            params![source, account],
        )?;
        for a in addresses {
            let a = a.trim().to_ascii_lowercase();
            if a.contains('@') {
                store.conn().execute(
                    "INSERT INTO user_addresses (address, cal_source_id, account_id) VALUES (?1, ?2, ?3)",
                    params![a, source, account],
                )?;
            }
        }
        Ok(())
    })
}

/// Marks a meeting whose time the organizer changed since the user answered (or clears it).
pub fn set_reanswer(store: &Store, object_id: i64, on: bool) -> Result<()> {
    store.conn().execute("UPDATE cal_objects SET reanswer = ?2 WHERE id = ?1", params![object_id, on])?;
    Ok(())
}

/// Appointments overlapping `start..end` in any calendar but the one with `uid` (an
/// invitation's conflicts); busy ones only.
pub fn conflicts(store: &Store, start: DateTime<Utc>, end: DateTime<Utc>, uid: &str) -> Result<Vec<CalendarEvent>> {
    let me = my_addresses(store)?;
    Ok(events(store, start, end, &me, false)?
        .into_iter()
        .filter(|e| e.uid != uid && e.busy != Busy::Free && !e.all_day && e.start < end && e.end > start)
        .collect())
}

// ---------------------------------------------------------------------------- invitations

/// The address of a new object in a calendar (CalDAV: below the collection; others: the UID).
fn href_for(cal: &Calendar, uid: &str) -> String {
    match cal.kind {
        SourceKind::Caldav => format!("{}/{}.ics", cal.remote_id.trim_end_matches('/'), uid.replace(['@', '/'], "-")),
        SourceKind::Graph => format!("local:{uid}"),
        _ => uid.to_string(),
    }
}

/// Puts an invitation's appointment (iCalendar) into a calendar, whatever the user may change
/// there (it is someone else's meeting); returns the object.
pub fn insert_internal(store: &Store, calendar_id: i64, uid: &str, data: &str) -> Result<i64> {
    let cal = calendar(store, calendar_id)?;
    let (format, data) = if cal.kind == SourceKind::Graph {
        let f = fields_of_data("ics", data).ok_or_else(|| {
            Error::Invalid(crate::tr!("Der Termin lässt sich nicht lesen.", "The appointment cannot be read.").into())
        })?;
        ("graph", graph::new_event_json(uid, &f))
    } else {
        ("ics", data.to_string())
    };
    store.batch(|| {
        let (_, first, last) = describe(format, &data);
        store.conn().execute(
            "INSERT INTO cal_objects (calendar_id, href, uid, format, data, first_start, last_end) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![calendar_id, href_for(&cal, uid), uid, format, data, first, last],
        )?;
        let id = store.conn().last_insert_rowid();
        if cal.kind != SourceKind::Local {
            queue(store, id, "create", None, false)?;
        }
        Ok(id)
    })
}

/// Replaces an object's data (an organizer's update, the user's answer) and tells the server.
pub fn replace_internal(store: &Store, object_id: i64, data: &str) -> Result<()> {
    let o = object(store, object_id)?;
    let cal = calendar(store, o.calendar_id)?;
    store.batch(|| {
        set_data(store, o.id, data)?;
        store.conn().execute("UPDATE cal_objects SET deleted = 0 WHERE id = ?1", [o.id])?;
        if cal.kind != SourceKind::Local {
            queue(store, o.id, "update", None, false)?;
        }
        Ok(())
    })
}

/// Removes an object (a cancelled or declined meeting) and tells the server.
pub fn remove_internal(store: &Store, object_id: i64) -> Result<()> {
    let o = object(store, object_id)?;
    let cal = calendar(store, o.calendar_id)?;
    store.batch(|| {
        if cal.kind == SourceKind::Local {
            store.conn().execute("DELETE FROM cal_objects WHERE id = ?1", [o.id])?;
        } else {
            store.conn().execute("UPDATE cal_objects SET deleted = 1 WHERE id = ?1", [o.id])?;
            queue(store, o.id, "delete", None, false)?;
        }
        Ok(())
    })
}

/// Says what a sync failure means for the user (the URL of a subscription is never in it).
pub fn describe_error(e: &Error) -> String {
    match e {
        Error::Connection(p) => p.to_string(),
        other => trf!("{}", "{}", other),
    }
}

#[cfg(test)]
mod tests;
