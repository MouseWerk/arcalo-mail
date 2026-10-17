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
    /// The organizer moved the meeting since the user answered: the answer is asked for again.
    pub reanswer: bool,
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
    // A change of the whole series makes every earlier message outdated, a change of one
    // instance only the earlier messages about that instance.
    let newest: Option<i64> = store
        .conn()
        .query_row(
            "SELECT MAX(sequence) FROM itip_seen WHERE uid = ?1 AND message_id != ?2 AND method != ''
               AND (recurrence = '' OR recurrence = ?3)",
            params![inv.uid, message_id, inv.recurrence_id.as_deref().unwrap_or_default()],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let outdated = newest.is_some_and(|n| n > inv.sequence as i64);
    let stored = calendar::objects_by_uid(store, &inv.uid)?.into_iter().find(|o| !o.deleted);
    let event_key = stored.as_ref().map(|o| format!("{}|{}|", o.calendar_id, o.id));
    let reanswer = stored.is_some_and(|o| o.reanswer) && answer.is_none();
    let conflicts = if inv.method == "REQUEST" && !inv.all_day {
        calendar::conflicts(store, inv.start, inv.end, &inv.uid)?
    } else {
        vec![]
    };
    note(store, message_id, &inv)?;
    Ok(Some(Card { invitation: inv, answer, outdated, event_key, conflicts, reanswer }))
}

/// Notes that a message's invitation was seen.
fn note(store: &Store, message_id: i64, inv: &Invitation) -> Result<()> {
    store.conn().execute(
        "INSERT INTO itip_seen (message_id, uid, method, sequence, at, recurrence) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (message_id) DO NOTHING",
        params![message_id, inv.uid, inv.method, inv.sequence, now(), inv.recurrence_id.as_deref().unwrap_or_default()],
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

/// The address of an `ATTENDEE` line.
fn line_address(p: &Prop) -> &str {
    p.value.trim().trim_start_matches("mailto:").trim_start_matches("MAILTO:").trim()
}

/// The user's answer in an event (the `PARTSTAT` of the user's attendee line).
fn my_partstat(ev: &Component, me: &[String]) -> Option<String> {
    ev.all("ATTENDEE")
        .find(|p| me.iter().any(|m| m.eq_ignore_ascii_case(line_address(p))))
        .map(|p| p.param("PARTSTAT").unwrap_or("NEEDS-ACTION").to_ascii_uppercase())
}

/// Sets the user's answer in an event (tentative ones also marked busy-tentative, so the
/// calendar shows them hatched).
fn set_answer(ev: &mut Component, me: &[String], partstat: &str) {
    for p in ev.props.iter_mut().filter(|p| p.name == "ATTENDEE") {
        if me.iter().any(|m| m.eq_ignore_ascii_case(line_address(p))) {
            p.params.retain(|(k, _)| k != "PARTSTAT" && k != "RSVP");
            p.params.push(("PARTSTAT".into(), partstat.into()));
        }
    }
    ev.remove("X-MICROSOFT-CDO-BUSYSTATUS");
    if partstat == "TENTATIVE" {
        ev.props.push(Prop::new("X-MICROSOFT-CDO-BUSYSTATUS", "TENTATIVE"));
    }
}

/// The invitation's event as the user's copy: without the method, the user's attendee line with
/// the answer.
fn own_copy(data: &str, me: &str, answer: Answer) -> Option<String> {
    let mut top = ics::parse_components(data.as_bytes());
    let cal = top.iter_mut().find(|c| c.name == "VCALENDAR")?;
    cal.remove("METHOD");
    let me = [me.to_string()];
    for ev in cal.children.iter_mut().filter(|c| c.name == "VEVENT") {
        set_answer(ev, &me, answer.partstat());
    }
    Some(cal.to_text())
}

fn sequence_of(ev: &Component) -> u32 {
    ev.text("SEQUENCE").and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// Whether an organizer's version of an event (`new`) is newer than the stored one: a higher
/// `SEQUENCE`, or the same one with a later `DTSTAMP` (RFC 5546 2.1.5; servers send changes of
/// the place or the text without counting up).
fn newer(new: &Component, old: &Component) -> bool {
    let stamp = |c: &Component| c.text("DTSTAMP").unwrap_or_default().replace(['-', ':'], "").to_ascii_uppercase();
    let (a, b) = (sequence_of(new), sequence_of(old));
    a > b || (a == b && stamp(new) > stamp(old))
}

/// The times of an event, to see whether the organizer moved it.
type Times = (Option<(DateTime<Utc>, DateTime<Utc>)>, Option<String>, Option<String>);

fn times(file: &Component, ev: &Component) -> Times {
    (ics::span_of(file, ev), ev.text("RRULE"), ev.text("RDATE"))
}

/// Who changes the stored copy.
#[derive(Debug, Clone, Copy, PartialEq)]
enum By<'a> {
    /// The organizer's update or cancellation (only a newer version counts).
    Organizer,
    /// The user's answer (`PARTSTAT`); a declined instance of a series leaves it.
    User(&'a str),
}

/// What a message does to a stored copy.
#[derive(Debug, PartialEq)]
enum Merge {
    Keep,
    Remove,
    /// The new data, and whether the organizer moved a meeting the user had answered.
    Replace(String, bool),
}

/// Merges an invitation's message (`data`) into the stored copy of its appointment. A message
/// about one instance of a series (`RECURRENCE-ID`) changes only that instance: a cancellation
/// leaves it out (`EXDATE`), an update replaces or adds its own version. An update of the whole
/// series keeps the instances it does not name and what was left out. The user's answer stays as
/// it is (an update never answers for the user).
fn merge(stored: &str, data: &str, inv: &Invitation, me: &[String], by: By<'_>) -> Option<Merge> {
    let mut top = ics::parse_components(stored.as_bytes());
    let cal = top.iter_mut().find(|c| c.name == "VCALENDAR")?;
    let inc_top = ics::parse_components(data.as_bytes());
    let inc = inc_top.iter().find(|c| c.name == "VCALENDAR")?;
    let is_event = |c: &&Component| c.name == "VEVENT";
    let master_i = cal.children.iter().position(|c| c.name == "VEVENT" && c.prop("RECURRENCE-ID").is_none());
    let all_day = match master_i {
        Some(i) => ics::is_all_day(&cal.children[i]),
        None => inc.children.iter().filter(is_event).any(ics::is_all_day),
    };
    let key_of =
        |file: &Component, ev: &Component| ev.prop("RECURRENCE-ID").and_then(|p| ics::instance_of(file, p, all_day));
    let organizer = by == By::Organizer;
    let answered = |a: &Option<String>| matches!(a.as_deref(), Some("ACCEPTED" | "TENTATIVE" | "DECLINED"));
    // Zones the new parts name and the stored copy lacks.
    let add_zones = |cal: &mut Component| {
        for z in inc.children.iter().filter(|c| c.name == "VTIMEZONE") {
            let id = z.text("TZID");
            if !cal.children.iter().any(|c| c.name == "VTIMEZONE" && c.text("TZID") == id) {
                let at = cal.children.iter().position(|c| c.name == "VEVENT").unwrap_or(cal.children.len());
                cal.children.insert(at, z.clone());
            }
        }
    };
    if inv.recurrence_id.is_none() {
        let current = master_i.map(|i| &cal.children[i]).or_else(|| cal.children.iter().find(is_event))?;
        let inc_master = inc
            .children
            .iter()
            .find(|c| c.name == "VEVENT" && c.prop("RECURRENCE-ID").is_none())
            .or_else(|| inc.children.iter().find(is_event))?;
        if organizer && inv.method == "CANCEL" {
            return Some(if sequence_of(inc_master) >= sequence_of(current) { Merge::Remove } else { Merge::Keep });
        }
        if organizer && !newer(inc_master, current) {
            return Some(Merge::Keep);
        }
        let answer = match by {
            By::User(p) => Some(p.to_string()),
            By::Organizer => my_partstat(current, me),
        };
        let moved = organizer && answered(&answer) && times(inc, inc_master) != times(cal, current);
        let mut next = inc.clone();
        next.remove("METHOD");
        let named: Vec<String> = next.children.iter().filter(is_event).filter_map(|e| key_of(inc, e)).collect();
        // Instances the message does not name stay as they were (an answer to the whole
        // series holds for them too).
        let mut kept: Vec<Component> = cal
            .children
            .iter()
            .filter(is_event)
            .filter(|o| key_of(cal, o).is_some_and(|k| !named.contains(&k)))
            .cloned()
            .collect();
        if let By::User(p) = by {
            for o in kept.iter_mut() {
                set_answer(o, me, p);
            }
        }
        let stored_answers: Vec<(String, Option<String>)> =
            cal.children.iter().filter(is_event).filter_map(|o| Some((key_of(cal, o)?, my_partstat(o, me)))).collect();
        let gone: Vec<String> = match master_i {
            Some(i) => {
                let have = ics::excluded(inc, inc_master, all_day);
                ics::excluded(cal, &cal.children[i], all_day).into_iter().filter(|k| !have.contains(k)).collect()
            }
            None => vec![],
        };
        for ev in next.children.iter_mut().filter(|c| c.name == "VEVENT") {
            let own = match (&by, ev.prop("RECURRENCE-ID").and_then(|p| ics::instance_of(inc, p, all_day))) {
                (By::Organizer, Some(k)) => {
                    stored_answers.iter().find(|(sk, _)| *sk == k).and_then(|(_, a)| a.clone()).or(answer.clone())
                }
                _ => answer.clone(),
            };
            if let Some(a) = own {
                set_answer(ev, me, &a);
            }
            if ev.prop("RECURRENCE-ID").is_none() {
                for k in &gone {
                    if let Some(p) = ics::exdate_for(k) {
                        ev.props.push(p);
                    }
                }
            }
        }
        for z in cal.children.iter().filter(|c| c.name == "VTIMEZONE") {
            let id = z.text("TZID");
            if !kept.is_empty() && !next.children.iter().any(|c| c.name == "VTIMEZONE" && c.text("TZID") == id) {
                next.children.insert(0, z.clone());
            }
        }
        next.children.extend(kept);
        return Some(Merge::Replace(next.to_text(), moved));
    }
    // One instance of a series.
    let inc_ev = inc
        .children
        .iter()
        .find(|c| c.name == "VEVENT" && c.prop("RECURRENCE-ID").is_some())
        .or_else(|| inc.children.iter().find(is_event))?;
    let k = key_of(inc, inc_ev)?;
    let existing_i =
        cal.children.iter().position(|c| c.name == "VEVENT" && key_of(cal, c).as_deref() == Some(k.as_str()));
    let reference_i = existing_i.or(master_i)?;
    let cancel = (organizer && inv.method == "CANCEL") || by == By::User("DECLINED");
    if organizer {
        let current = &cal.children[reference_i];
        let fresh = match existing_i {
            Some(_) if cancel => sequence_of(inc_ev) >= sequence_of(current),
            Some(_) => newer(inc_ev, current),
            // The first change of this instance: its SEQUENCE counts from the series'.
            None => cancel || sequence_of(inc_ev) >= sequence_of(current),
        };
        if !fresh {
            return Some(Merge::Keep);
        }
    }
    if cancel {
        let already = master_i.map(|i| ics::excluded(cal, &cal.children[i], all_day)).unwrap_or_default();
        if let Some(i) = existing_i {
            cal.children.remove(i);
        }
        let any_left = cal.children.iter().any(|c| c.name == "VEVENT");
        let master = cal.children.iter_mut().find(|c| c.name == "VEVENT" && c.prop("RECURRENCE-ID").is_none());
        match master {
            Some(m) => {
                if !already.contains(&k)
                    && let Some(p) = ics::exdate_for(&k)
                {
                    m.props.push(p);
                }
            }
            None if !any_left => return Some(Merge::Remove),
            None => {}
        }
        return Some(Merge::Replace(cal.to_text(), false));
    }
    let answer = match by {
        By::User(p) => Some(p.to_string()),
        By::Organizer => my_partstat(&cal.children[reference_i], me),
    };
    let moved = organizer
        && answered(&answer)
        && match existing_i {
            Some(i) => times(inc, inc_ev) != times(cal, &cal.children[i]),
            None => {
                inc_ev.prop("DTSTART").and_then(|p| ics::instance_of(inc, p, all_day)).as_deref() != Some(k.as_str())
            }
        };
    let mut ev = inc_ev.clone();
    if let Some(a) = &answer {
        set_answer(&mut ev, me, a);
    }
    match existing_i {
        Some(i) => cal.children[i] = ev,
        None => cal.children.push(ev),
    }
    add_zones(cal);
    Some(Merge::Replace(cal.to_text(), moved))
}

/// Puts an answered invitation into the calendars: the appointment already there (a server that
/// schedules put it in) gets the answer, else an accepted or tentative one goes into
/// `calendar_id`; a declined one leaves (a declined instance of a series leaves the series).
/// Returns the appointment's key, if it stays.
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
        if answer == Answer::Declined && inv.recurrence_id.is_none() {
            calendar::remove_internal(store, o.id)?;
            return Ok(None);
        }
        let mine = [me.clone()];
        match merge(&o.data, data, inv, &mine, By::User(answer.partstat())) {
            Some(Merge::Replace(next, _)) => calendar::replace_internal(store, o.id, &next)?,
            Some(Merge::Remove) => {
                calendar::remove_internal(store, o.id)?;
                return Ok(None);
            }
            _ => calendar::replace_internal(store, o.id, &copy)?,
        }
        calendar::set_reanswer(store, o.id, false)?;
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
/// that organizer. Anything else stays a card the user can look at. The user's answer is never
/// changed by an update: when the organizer moves a meeting the user had answered, the
/// appointment asks for a new answer ([`calendar::Object::reanswer`]).
pub fn apply(store: &Store, data: &str, inv: &Invitation, sender: Option<&str>) -> Result<bool> {
    if inv.mine || !matches!(inv.method.as_str(), "REQUEST" | "CANCEL") {
        return Ok(false);
    }
    let organizer = organizer_address(&inv.organizer);
    let Some(sender) = sender.map(|s| s.trim().to_ascii_lowercase()) else { return Ok(false) };
    if organizer.is_empty() || sender != organizer {
        return Ok(false);
    }
    let me = calendar::my_addresses(store)?;
    let mut changed = false;
    for o in calendar::objects_by_uid(store, &inv.uid)?.into_iter().filter(|o| o.format == "ics" && !o.deleted) {
        let Ok(cal) = calendar::calendar(store, o.calendar_id) else { continue };
        if !cal.writable || cal.kind == calendar::SourceKind::Ics {
            continue;
        }
        let Some(current) = parse(&o.data, &me) else { continue };
        // The organizer stays who it was.
        if organizer_address(&current.organizer) != organizer {
            continue;
        }
        match merge(&o.data, data, inv, &me, By::Organizer) {
            None | Some(Merge::Keep) => continue,
            Some(Merge::Remove) => calendar::remove_internal(store, o.id)?,
            Some(Merge::Replace(next, moved)) => {
                calendar::replace_internal(store, o.id, &next)?;
                if moved {
                    calendar::set_reanswer(store, o.id, true)?;
                }
            }
        }
        changed = true;
    }
    Ok(changed)
}

/// The calendar part of a message: from the message when it was kept whole, else (a large
/// message) the part fetched with its body ([`crate::sync`]). `None` until it is loaded.
pub fn calendar_data(store: &Store, message_id: i64) -> Result<Option<String>> {
    if let Some(raw) = store.raw_message(message_id)? {
        return Ok(find_in_message(&raw));
    }
    let mut st = store.conn().prepare(
        "SELECT data FROM attachments WHERE message_id = ?1 AND data IS NOT NULL
           AND (lower(mime_type) IN ('text/calendar', 'application/ics') OR lower(filename) LIKE '%.ics')
         ORDER BY lower(mime_type) = 'text/calendar' DESC, part_index",
    )?;
    let parts: Vec<Vec<u8>> = st.query_map([message_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut fallback = None;
    for bytes in parts {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if !text.contains("BEGIN:VCALENDAR") {
            continue;
        }
        if text.contains("METHOD:") {
            return Ok(Some(text));
        }
        fallback.get_or_insert(text);
    }
    Ok(fallback)
}

/// Notes a message whose calendar part cannot be read, so it is not looked at again.
fn note_unreadable(store: &Store, message_id: i64) -> Result<()> {
    store.conn().execute(
        "INSERT INTO itip_seen (message_id, uid, method, sequence, at) VALUES (?1, '', '', 0, ?2)
         ON CONFLICT (message_id) DO NOTHING",
        params![message_id, now()],
    )?;
    Ok(())
}

/// Messages with an invitation that the calendars have not seen yet: applies their updates and
/// cancellations. Called after a sync; returns whether a calendar changed. A message whose
/// calendar part is not loaded yet waits until it is.
pub fn scan(store: &Store) -> Result<bool> {
    let mut st = store.conn().prepare(
        "SELECT DISTINCT a.message_id FROM attachments a
          WHERE (a.mime_type IN ('text/calendar', 'application/ics') OR lower(a.filename) LIKE '%.ics')
            AND a.message_id NOT IN (SELECT message_id FROM itip_seen)
            AND (a.data IS NOT NULL
              OR EXISTS (SELECT 1 FROM message_bodies b WHERE b.message_id = a.message_id AND b.raw IS NOT NULL))
          ORDER BY a.message_id LIMIT 200",
    )?;
    let ids: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let me = calendar::my_addresses(store)?;
    let mut changed = false;
    for id in ids {
        let Some(inv) = calendar_data(store, id)?.and_then(|data| Some((parse(&data, &me)?, data))) else {
            note_unreadable(store, id)?;
            continue;
        };
        let (inv, data) = inv;
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
