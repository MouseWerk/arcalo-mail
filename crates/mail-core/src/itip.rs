//! Meeting invitations in mail (iTIP, RFC 5546, sent as iMIP, RFC 6047): the `text/calendar`
//! part of a message read as an invitation card (time, place, attendees, the user's answer),
//! the answer to the organizer (`METHOD:REPLY`, a mail with the calendar part) and what an
//! invitation does to the user's calendars: an accepted one goes into the default calendar,
//! an update (`REQUEST` with a higher `SEQUENCE`) or a cancellation (`CANCEL`) changes the
//! appointment already there.

use chrono::{DateTime, Duration, TimeZone, Utc};
use mail_parser::{MessageParser, MimeHeaders};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::account::Account;
use crate::calendar::ics::{self, Component, Prop, escape};
use crate::calendar::{self, attendee_email, tz::Zone};
use crate::error::{Error, Result};
use crate::store::{Store, now};
use crate::{tr, trf};

/// The calendar part of a message (`text/calendar`, or an `.ics` attachment), as text.
pub fn find_in_message(raw: &[u8]) -> Option<String> {
    let m = MessageParser::default().parse(raw)?;
    let mut fallback = None;
    for part in m.parts.iter() {
        let Some(ct) = part.content_type() else { continue };
        let mime = format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or_default()).to_ascii_lowercase();
        let named_ics = part.attachment_name().is_some_and(|n| n.to_ascii_lowercase().ends_with(".ics"));
        if mime == "text/calendar" || mime == "application/ics" || named_ics {
            let text = String::from_utf8_lossy(part.contents()).into_owned();
            if !text.contains("BEGIN:VCALENDAR") {
                continue;
            }
            // The part with a method (the invitation itself) wins over a copy as attachment.
            if ct.attribute("method").is_some() || text.contains("METHOD:") {
                return Some(text);
            }
            fallback.get_or_insert(text);
        }
    }
    fallback
}

/// An attendee as the card shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attendee {
    pub name: String,
    pub email: String,
    /// `ACCEPTED`, `TENTATIVE`, `DECLINED`, `NEEDS-ACTION`, `DELEGATED`.
    pub status: String,
    /// `OPT-PARTICIPANT` is optional, `CHAIR` leads.
    pub role: String,
}

/// An invitation (or an update, a cancellation, an answer) read from a message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Invitation {
    /// `REQUEST`, `CANCEL`, `REPLY`, `PUBLISH`, `COUNTER`, …
    pub method: String,
    pub uid: String,
    pub sequence: u32,
    /// The instance of a series the message is about.
    pub recurrence_id: Option<String>,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub recurring: bool,
    pub location: String,
    pub description: Option<String>,
    /// `Name <address>`.
    pub organizer: String,
    pub attendees: Vec<Attendee>,
    pub link: Option<String>,
    pub link_kind: Option<String>,
    /// The user's address the invitation is for (one of the accounts), and the answer it holds.
    pub me: Option<String>,
    pub my_status: Option<String>,
    /// The organizer is the user (a reply to the user's own meeting).
    pub mine: bool,
}

fn attendee_of(p: &Prop) -> Attendee {
    let email = p.value.trim().trim_start_matches("mailto:").trim_start_matches("MAILTO:").trim().to_string();
    Attendee {
        name: p.param("CN").map(str::to_owned).unwrap_or_default(),
        email,
        status: p.param("PARTSTAT").unwrap_or("NEEDS-ACTION").to_ascii_uppercase(),
        role: p.param("ROLE").unwrap_or("REQ-PARTICIPANT").to_ascii_uppercase(),
    }
}

/// The method and the main `VEVENT` of an iCalendar text.
fn event_of(data: &str) -> Option<(String, Component, Component)> {
    let top = ics::parse_components(data.as_bytes());
    let cal = top.into_iter().find(|c| c.name == "VCALENDAR")?;
    let method = cal.text("METHOD").unwrap_or_else(|| "PUBLISH".into()).to_ascii_uppercase();
    let ev = cal
        .children
        .iter()
        .find(|c| c.name == "VEVENT" && c.prop("RECURRENCE-ID").is_none())
        .or_else(|| cal.children.iter().find(|c| c.name == "VEVENT"))?
        .clone();
    Some((method, ev, cal))
}

/// Reads an invitation; `me`: the user's addresses (lower case).
pub fn parse(data: &str, me: &[String]) -> Option<Invitation> {
    let (method, ev, _) = event_of(data)?;
    let uid = ev.text("UID")?;
    let far = (Utc.with_ymd_and_hms(1900, 1, 1, 0, 0, 0).unwrap(), Utc.with_ymd_and_hms(2200, 1, 1, 0, 0, 0).unwrap());
    // The first instance (a cancelled one is still described by its own times).
    let mut single = Component::new("VCALENDAR");
    let mut copy = ev.clone();
    copy.remove("STATUS");
    single.children.push(copy);
    let o = ics::parse(single.to_text().as_bytes(), far, &Zone::Local).ok()?.into_iter().next()?;
    let attendees: Vec<Attendee> = ev
        .all("ATTENDEE")
        .filter(|p| !matches!(p.param("CUTYPE").map(|c| c.to_ascii_uppercase()).as_deref(), Some("RESOURCE" | "ROOM")))
        .map(attendee_of)
        .collect();
    let organizer_email = ev.prop("ORGANIZER").map(|p| attendee_of(p).email.to_ascii_lowercase());
    let mine_addr = |e: &str| me.iter().any(|m| m.eq_ignore_ascii_case(e));
    let me_att = attendees.iter().find(|a| mine_addr(&a.email));
    Some(Invitation {
        method,
        uid,
        sequence: ev.text("SEQUENCE").and_then(|s| s.parse().ok()).unwrap_or(0),
        recurrence_id: ev.prop("RECURRENCE-ID").map(|p| p.value.clone()),
        title: o.title,
        start: o.start,
        end: o.end,
        all_day: o.all_day,
        recurring: ev.prop("RRULE").is_some() || ev.prop("RDATE").is_some(),
        location: o.location,
        description: o.body,
        organizer: o.organizer,
        link_kind: o.link.as_deref().map(|l| calendar::meeting_kind(l).to_string()),
        link: o.link,
        me: me_att.map(|a| a.email.clone()),
        my_status: me_att.map(|a| a.status.clone()),
        mine: organizer_email.as_deref().is_some_and(mine_addr),
        attendees,
    })
}

/// The user's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Accepted,
    Tentative,
    Declined,
}

impl Answer {
    pub fn partstat(self) -> &'static str {
        match self {
            Answer::Accepted => "ACCEPTED",
            Answer::Tentative => "TENTATIVE",
            Answer::Declined => "DECLINED",
        }
    }

    /// Graph's action for the answer.
    pub fn graph_action(self) -> &'static str {
        match self {
            Answer::Accepted => "accept",
            Answer::Tentative => "tentativelyAccept",
            Answer::Declined => "decline",
        }
    }

    fn subject(self, title: &str) -> String {
        match self {
            Answer::Accepted => trf!("Zugesagt: {title}", "Accepted: {title}"),
            Answer::Tentative => trf!("Mit Vorbehalt: {title}", "Tentative: {title}"),
            Answer::Declined => trf!("Abgesagt: {title}", "Declined: {title}"),
        }
    }
}

/// The `METHOD:REPLY` of the user (`email`, `name`) to an invitation: the event's identity,
/// the organizer and the user's own attendee line with the answer (RFC 5546, 3.2.3).
pub fn reply_ics(
    data: &str,
    email: &str,
    name: &str,
    answer: Answer,
    comment: &str,
    at: DateTime<Utc>,
) -> Option<String> {
    let (_, ev, _) = event_of(data)?;
    let mut r = Component::new("VEVENT");
    for keep in ["UID", "SEQUENCE", "RECURRENCE-ID", "DTSTART", "DTEND", "DURATION", "SUMMARY", "ORGANIZER"] {
        for p in ev.all(keep) {
            r.props.push(p.clone());
        }
    }
    r.props.push(Prop::new("DTSTAMP", &ics::utc_text(at)));
    let mut me = Prop::new("ATTENDEE", &format!("mailto:{email}")).with("PARTSTAT", answer.partstat());
    if !name.trim().is_empty() {
        me = me.with("CN", name.trim());
    }
    r.props.push(me);
    if !comment.trim().is_empty() {
        r.props.push(Prop::new("COMMENT", &escape(comment.trim())));
    }
    let mut cal = Component::new("VCALENDAR");
    cal.props.push(Prop::new("PRODID", "-//MouseWerk//Arcalo Mail//EN"));
    cal.props.push(Prop::new("VERSION", "2.0"));
    cal.props.push(Prop::new("METHOD", "REPLY"));
    // The zones the times name.
    let (_, _, whole) = event_of(data)?;
    for z in whole.children.iter().filter(|c| c.name == "VTIMEZONE") {
        cal.children.push(z.clone());
    }
    cal.children.push(r);
    Some(cal.to_text())
}

/// What a mail with a calendar part says.
pub struct CalendarMail<'a> {
    pub to: &'a [String],
    pub subject: &'a str,
    pub text: &'a str,
    /// `REPLY`, `REQUEST`, `CANCEL`.
    pub method: &'a str,
    pub ics: &'a str,
}

/// A mail with a calendar part (iMIP): a short text and the `text/calendar` part with its
/// method, a new Message-ID, dated now.
pub fn calendar_mail(account: &Account, m: &CalendarMail<'_>) -> Result<Vec<u8>> {
    let (to, subject, text, method, ics_text) = (m.to, m.subject, m.text, m.method, m.ics);
    let message_id = crate::compose::new_message_id(&account.email);
    let date = now();
    use mail_builder::MessageBuilder;
    use mail_builder::headers::address::Address as MbAddress;
    use mail_builder::headers::content_type::ContentType;
    use mail_builder::mime::MimePart;
    let from = if account.sender_name.trim().is_empty() {
        MbAddress::from(account.email.clone())
    } else {
        MbAddress::from((account.sender_name.trim().to_string(), account.email.clone()))
    };
    let body = MimePart::new(
        "multipart/alternative",
        vec![
            MimePart::new("text/plain", text.to_string()),
            MimePart::new(
                ContentType::new("text/calendar").attribute("method", method.to_string()).attribute("charset", "utf-8"),
                ics_text.to_string(),
            ),
        ],
    );
    let to: Vec<MbAddress> = to.iter().map(|t| MbAddress::from(t.clone())).collect();
    MessageBuilder::new()
        .from(from)
        .to(to)
        .subject(subject.to_string())
        .message_id(message_id.trim_matches(['<', '>']).to_string())
        .date(mail_builder::headers::date::Date::new(date))
        .body(body)
        .write_to_vec()
        .map_err(|e| Error::Invalid(e.to_string()))
}

/// The answer as a mail to the organizer: message, envelope recipient.
pub fn reply_mail(
    account: &Account,
    inv: &Invitation,
    data: &str,
    answer: Answer,
    comment: &str,
) -> Result<(Vec<u8>, String)> {
    let organizer = attendee_email(&inv.organizer).ok_or_else(|| {
        Error::Invalid(
            tr!("Die Einladung nennt niemanden, der sie verschickt hat.", "The invitation names no organizer.").into(),
        )
    })?;
    let email = inv.me.clone().unwrap_or_else(|| account.email.clone());
    let name = if account.sender_name.trim().is_empty() { account.name.clone() } else { account.sender_name.clone() };
    let ics_text = reply_ics(data, &email, &name, answer, comment, Utc::now()).ok_or_else(|| {
        Error::Invalid(tr!("Die Einladung ist nicht lesbar.", "The invitation cannot be read.").into())
    })?;
    let text = match answer {
        Answer::Accepted => trf!("{} hat zugesagt.", "{} accepted.", email),
        Answer::Tentative => trf!("{} hat mit Vorbehalt zugesagt.", "{} accepted tentatively.", email),
        Answer::Declined => trf!("{} hat abgesagt.", "{} declined.", email),
    };
    let text = if comment.trim().is_empty() { text } else { format!("{text}\n\n{}", comment.trim()) };
    let raw = calendar_mail(
        account,
        &CalendarMail {
            to: std::slice::from_ref(&organizer),
            subject: &answer.subject(&inv.title),
            text: &text,
            method: "REPLY",
            ics: &ics_text,
        },
    )?;
    Ok((raw, organizer))
}

/// An update of a meeting the user organizes as a mail to its attendees (`METHOD:REQUEST`), for
/// servers that do not send it themselves. Returns the message and the recipients.
pub fn update_mail(account: &Account, data: &str) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let me = [account.email.to_ascii_lowercase()];
    let Some(inv) = parse(data, &me) else { return Ok(None) };
    let to: Vec<String> = inv
        .attendees
        .iter()
        .map(|a| a.email.clone())
        .filter(|e| !me.iter().any(|m| m.eq_ignore_ascii_case(e)))
        .collect();
    if to.is_empty() {
        return Ok(None);
    }
    let mut top = ics::parse_components(data.as_bytes());
    let Some(cal) = top.iter_mut().find(|c| c.name == "VCALENDAR") else { return Ok(None) };
    cal.set(Prop::new("METHOD", "REQUEST"));
    let when = crate::calendar::tz::Zone::Local.to_wall(inv.start).format("%d.%m.%Y %H:%M").to_string();
    let text = trf!("Geändert: {} am {}", "Changed: {} on {}", inv.title, when);
    let subject = trf!("Aktualisiert: {}", "Updated: {}", inv.title);
    let ics_text = cal.to_text();
    let raw = calendar_mail(
        account,
        &CalendarMail { to: &to, subject: &subject, text: &text, method: "REQUEST", ics: &ics_text },
    )?;
    Ok(Some((raw, to)))
}

// ---------------------------------------------------------------------------- the calendars

/// What the app knows about an invitation's appointment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card {
    pub invitation: Invitation,
    /// The answer the user gave here (`accepted`, `tentative`, `declined`).
    pub answer: Option<String>,
    /// A newer message changed or cancelled the meeting since.
    pub outdated: bool,
    /// The appointment is in one of the user's calendars (its key).
    pub event_key: Option<String>,
    /// Busy appointments at the same time.
    pub conflicts: Vec<calendar::CalendarEvent>,
}

/// The card of a message's invitation.
pub fn card(store: &Store, message_id: i64, data: &str) -> Result<Option<Card>> {
    let me = calendar::my_addresses(store)?;
    let Some(inv) = parse(data, &me) else { return Ok(None) };
    let answer: Option<String> = store
        .conn()
        .query_row("SELECT answer FROM itip_seen WHERE message_id = ?1", [message_id], |r| r.get(0))
        .optional()?
        .flatten();
    let newest: Option<i64> = store
        .conn()
        .query_row(
            "SELECT MAX(sequence) FROM itip_seen WHERE uid = ?1 AND message_id != ?2",
            params![inv.uid, message_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let outdated = newest.is_some_and(|n| n > inv.sequence as i64);
    let event_key = calendar::objects_by_uid(store, &inv.uid)?
        .into_iter()
        .find(|o| !o.deleted)
        .map(|o| format!("{}|{}|", o.calendar_id, o.id));
    let conflicts = if inv.method == "REQUEST" && !inv.all_day {
        calendar::conflicts(store, inv.start, inv.end, &inv.uid)?
    } else {
        vec![]
    };
    note(store, message_id, &inv)?;
    Ok(Some(Card { invitation: inv, answer, outdated, event_key, conflicts }))
}

/// Notes that a message's invitation was seen.
fn note(store: &Store, message_id: i64, inv: &Invitation) -> Result<()> {
    store.conn().execute(
        "INSERT INTO itip_seen (message_id, uid, method, sequence, at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (message_id) DO NOTHING",
        params![message_id, inv.uid, inv.method, inv.sequence, now()],
    )?;
    Ok(())
}

/// Notes the user's answer.
pub fn answered(store: &Store, message_id: i64, inv: &Invitation, answer: Answer) -> Result<()> {
    note(store, message_id, inv)?;
    let a = match answer {
        Answer::Accepted => "accepted",
        Answer::Tentative => "tentative",
        Answer::Declined => "declined",
    };
    store.conn().execute("UPDATE itip_seen SET answer = ?2 WHERE message_id = ?1", params![message_id, a])?;
    Ok(())
}

/// The invitation's event as the user's copy: without the method, the user's attendee line with
/// the answer (tentative ones also marked busy-tentative, so the calendar shows them hatched).
fn own_copy(data: &str, me: &str, answer: Answer) -> Option<String> {
    let mut top = ics::parse_components(data.as_bytes());
    let cal = top.iter_mut().find(|c| c.name == "VCALENDAR")?;
    cal.remove("METHOD");
    for ev in cal.children.iter_mut().filter(|c| c.name == "VEVENT") {
        for p in ev.props.iter_mut().filter(|p| p.name == "ATTENDEE") {
            let addr = p.value.trim().trim_start_matches("mailto:").trim_start_matches("MAILTO:");
            if addr.eq_ignore_ascii_case(me) {
                p.params.retain(|(k, _)| k != "PARTSTAT" && k != "RSVP");
                p.params.push(("PARTSTAT".into(), answer.partstat().into()));
            }
        }
        ev.remove("X-MICROSOFT-CDO-BUSYSTATUS");
        if answer == Answer::Tentative {
            ev.props.push(Prop::new("X-MICROSOFT-CDO-BUSYSTATUS", "TENTATIVE"));
        }
    }
    Some(cal.to_text())
}

/// Puts an answered invitation into the calendars: the appointment already there (a server that
/// schedules put it in) gets the answer, else an accepted or tentative one goes into
/// `calendar_id`; a declined one leaves. Returns the appointment's key, if it stays.
pub fn file(store: &Store, data: &str, inv: &Invitation, answer: Answer, calendar_id: i64) -> Result<Option<String>> {
    let me = inv.me.clone().or_else(|| calendar::my_addresses(store).ok()?.into_iter().next()).unwrap_or_default();
    let copy = own_copy(data, &me, answer).ok_or_else(|| {
        Error::Invalid(tr!("Die Einladung ist nicht lesbar.", "The invitation cannot be read.").into())
    })?;
    let existing: Vec<calendar::Object> = calendar::objects_by_uid(store, &inv.uid)?
        .into_iter()
        .filter(|o| o.format == "ics")
        .filter(|o| {
            calendar::calendar(store, o.calendar_id).is_ok_and(|c| c.writable && c.kind != calendar::SourceKind::Ics)
        })
        .collect();
    if let Some(o) = existing.first() {
        if answer == Answer::Declined {
            calendar::remove_internal(store, o.id)?;
            return Ok(None);
        }
        calendar::replace_internal(store, o.id, &copy)?;
        return Ok(Some(format!("{}|{}|", o.calendar_id, o.id)));
    }
    if answer == Answer::Declined {
        return Ok(None);
    }
    let id = calendar::insert_internal(store, calendar_id, &inv.uid, &copy)?;
    Ok(Some(format!("{calendar_id}|{id}|")))
}

/// The address of an `ORGANIZER` as [`Invitation::organizer`] holds it (`Name <address>`).
fn organizer_address(organizer: &str) -> String {
    let o = organizer.trim();
    let a = o.rsplit_once('<').map(|(_, a)| a.trim_end_matches('>')).unwrap_or(o);
    a.trim().trim_start_matches("mailto:").trim_start_matches("MAILTO:").trim().to_ascii_lowercase()
}

/// An update or a cancellation from the organizer, applied to the copy in the user's calendars
/// (not to a Microsoft calendar: Exchange does that itself). `true` when something changed.
/// Only the organizer may change a meeting (RFC 5546 3.2.2, 3.2.5): the message's sender
/// (`sender`, its From address) must be the organizer of the stored copy, and the update keeps
/// that organizer. Anything else stays a card the user can look at.
pub fn apply(store: &Store, data: &str, inv: &Invitation, sender: Option<&str>) -> Result<bool> {
    if inv.mine || !matches!(inv.method.as_str(), "REQUEST" | "CANCEL") {
        return Ok(false);
    }
    let organizer = organizer_address(&inv.organizer);
    let Some(sender) = sender.map(|s| s.trim().to_ascii_lowercase()) else { return Ok(false) };
    if organizer.is_empty() || sender != organizer {
        return Ok(false);
    }
    let mut changed = false;
    for o in calendar::objects_by_uid(store, &inv.uid)?.into_iter().filter(|o| o.format == "ics" && !o.deleted) {
        let Ok(cal) = calendar::calendar(store, o.calendar_id) else { continue };
        if !cal.writable || cal.kind == calendar::SourceKind::Ics {
            continue;
        }
        let me = calendar::my_addresses(store)?;
        let Some(current) = parse(&o.data, &me) else { continue };
        // The organizer stays who it was.
        if organizer_address(&current.organizer) != organizer {
            continue;
        }
        if inv.sequence < current.sequence || (inv.sequence == current.sequence && inv.method == "REQUEST") {
            continue;
        }
        if inv.method == "CANCEL" {
            calendar::remove_internal(store, o.id)?;
        } else {
            // The user's answer stays.
            let answer = match current.my_status.as_deref() {
                Some("TENTATIVE") => Answer::Tentative,
                _ => Answer::Accepted,
            };
            let me_addr = current.me.clone().unwrap_or_default();
            if let Some(copy) = own_copy(data, &me_addr, answer) {
                calendar::replace_internal(store, o.id, &copy)?;
            }
        }
        changed = true;
    }
    Ok(changed)
}

/// Messages with an invitation that the calendars have not seen yet: applies their updates and
/// cancellations. Called after a sync; returns whether a calendar changed.
pub fn scan(store: &Store) -> Result<bool> {
    let mut st = store.conn().prepare(
        "SELECT DISTINCT a.message_id FROM attachments a
          WHERE (a.mime_type IN ('text/calendar', 'application/ics') OR lower(a.filename) LIKE '%.ics')
            AND a.message_id NOT IN (SELECT message_id FROM itip_seen)
          ORDER BY a.message_id LIMIT 200",
    )?;
    let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let me = calendar::my_addresses(store)?;
    let mut changed = false;
    for id in ids {
        let Some(raw) = store.raw_message(id)? else { continue };
        let Some(data) = find_in_message(&raw) else {
            continue;
        };
        let Some(inv) = parse(&data, &me) else { continue };
        note(store, id, &inv)?;
        let detail = store.message(id)?;
        // Mail in Junk or Trash never changes the calendar.
        let role = store.mailbox(detail.summary.mailbox_id)?.role;
        if matches!(role, Some(crate::imap::Role::Junk | crate::imap::Role::Trash)) {
            continue;
        }
        let sender = detail.summary.from.as_ref().map(|a| a.email.as_str());
        changed |= apply(store, &data, &inv, sender)?;
    }
    Ok(changed)
}

/// A draft appointment from a date found in a mail (the assistant's „Termine“): title, start and
/// length, nothing written yet; the calendar's editor opens with it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Draft {
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub location: String,
    pub description: String,
}

/// The draft of an item the assistant found (`ai::extract::Item`), placed in the device's zone.
pub fn draft_from(item: &crate::ai::extract::Item, description: &str) -> Option<Draft> {
    let date = chrono::NaiveDate::parse_from_str(item.date.as_deref()?, "%Y-%m-%d").ok()?;
    let time = |t: Option<&str>| t.and_then(|t| chrono::NaiveTime::parse_from_str(t.trim(), "%H:%M").ok());
    let (start, end, all_day) = match time(item.time.as_deref()) {
        Some(t) => {
            let s = Zone::Local.to_utc(date.and_time(t));
            let e = time(item.end.as_deref()).map(|e| Zone::Local.to_utc(date.and_time(e))).filter(|e| *e > s);
            (s, e.unwrap_or(s + Duration::hours(1)), false)
        }
        None => {
            let s = Zone::Local.to_utc(date.and_hms_opt(0, 0, 0)?);
            (s, s + Duration::days(1), true)
        }
    };
    Some(Draft {
        title: item.title.trim().to_string(),
        start,
        end,
        all_day,
        location: item.place.clone().unwrap_or_default(),
        description: description.trim().to_string(),
    })
}

#[cfg(test)]
mod tests;
