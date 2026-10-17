//! Invitations round trip: read from an Outlook-style mail, answered (the REPLY and its mail),
//! filed in the calendar, changed and cancelled by the organizer's later mails.

use super::*;
use crate::account::{AccountDraft, Provider, preset};
use crate::calendar::{Busy, Editability};
use crate::imap::MailboxEntry;
use crate::parse::parse as parse_mail;
use crate::store::tests::header;

fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
}

/// An invitation as Outlook sends it: text, the calendar part with its method, the same as an
/// attachment.
pub(crate) fn invite(method: &str, sequence: u32, start: &str, end: &str, extra: &str) -> String {
    let ics = format!(
        "BEGIN:VCALENDAR\r\nMETHOD:{method}\r\nPRODID:Microsoft Exchange Server 2010\r\nVERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\nBEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\nEND:STANDARD\r\nBEGIN:DAYLIGHT\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\nRRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nORGANIZER;CN=Jonas Berger:mailto:jonas@example.org\r\n\
ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Maria Muster:mailto:maria@example.org\r\n\
ATTENDEE;ROLE=OPT-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Lea Weber:mailto:lea@example.org\r\n\
ATTENDEE;CUTYPE=RESOURCE;CN=Raum Elbe:mailto:elbe@example.org\r\n\
DESCRIPTION:Agenda folgt.\\n\\nMicrosoft Teams-Besprechung\\nhttps://teams.microsoft.com/l/meetup-join/19%3ameeting\r\n\
UID:040000008200E00074C5B7101A82E0080000000001\r\nSUMMARY;LANGUAGE=de-DE:Projektstart Website\r\n\
DTSTART;TZID=W. Europe Standard Time:{start}\r\nDTEND;TZID=W. Europe Standard Time:{end}\r\nLOCATION:Raum Elbe\r\n\
SEQUENCE:{sequence}\r\nDTSTAMP:20261016T080000Z\r\n{extra}END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    format!(
        "From: Jonas Berger <jonas@example.org>\r\nTo: Maria Muster <maria@example.org>\r\nSubject: Projektstart Website\r\n\
Date: Fri, 16 Oct 2026 10:00:00 +0200\r\nMessage-ID: <inv-{method}-{sequence}@example.org>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"M\"\r\n\r\n--M\r\nContent-Type: multipart/alternative; boundary=\"A\"\r\n\r\n\
--A\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nWann: 3. November 2026 10:00-11:00\r\n\
--A\r\nContent-Type: text/calendar; charset=utf-8; method={method}\r\n\r\n{ics}\r\n--A--\r\n\
--M\r\nContent-Type: application/ics; name=\"invite.ics\"\r\nContent-Disposition: attachment; filename=\"invite.ics\"\r\n\r\n{ics}\r\n--M--\r\n"
    )
}

fn me() -> Vec<String> {
    vec!["maria@example.org".into()]
}

fn store() -> (Store, i64, i64) {
    let s = Store::open_in_memory().unwrap();
    let account = s
        .add_account(&AccountDraft {
            provider: Provider::Imap,
            name: "Maria".into(),
            email: "maria@example.org".into(),
            sender_name: "Maria Muster".into(),
            transport: preset(Provider::Imap, "maria@example.org").unwrap(),
        })
        .unwrap();
    let inbox = s
        .upsert_mailbox(account, &MailboxEntry { name: "INBOX".into(), delimiter: Some('/'), attributes: vec![] })
        .unwrap();
    (s, account, inbox)
}

fn deliver(s: &Store, inbox: i64, uid: u32, raw: &str) -> i64 {
    let p = parse_mail(raw.as_bytes()).unwrap();
    let id = s.insert_message(inbox, &header(uid, raw.as_bytes()), &p).unwrap();
    s.set_body_raw(id, &p, Some(raw.as_bytes())).unwrap();
    id
}

#[test]
fn reads_an_outlook_invitation() {
    let raw = invite("REQUEST", 0, "20261103T100000", "20261103T110000", "");
    let data = find_in_message(raw.as_bytes()).unwrap();
    assert!(data.contains("METHOD:REQUEST"));
    let inv = parse(&data, &me()).unwrap();
    assert_eq!((inv.method.as_str(), inv.title.as_str(), inv.sequence), ("REQUEST", "Projektstart Website", 0));
    assert_eq!((inv.start, inv.end), (utc(2026, 11, 3, 9, 0), utc(2026, 11, 3, 10, 0)), "10:00 Berlin in winter time");
    assert_eq!(inv.organizer, "Jonas Berger <jonas@example.org>");
    assert_eq!(inv.attendees.len(), 2, "the room is no attendee");
    assert_eq!(inv.attendees[1].role, "OPT-PARTICIPANT");
    assert_eq!((inv.me.as_deref(), inv.my_status.as_deref()), (Some("maria@example.org"), Some("NEEDS-ACTION")));
    assert_eq!(inv.link_kind.as_deref(), Some("Teams"));
    assert!(!inv.mine);
    assert!(find_in_message(crate::parse::tests::sample("x@y", "Hallo", None, "kein Termin").as_slice()).is_none());
}

#[test]
fn the_answer_goes_to_the_organizer() {
    let (s, account, _) = store();
    let acc = s.account(account).unwrap();
    let data = find_in_message(invite("REQUEST", 2, "20261103T100000", "20261103T110000", "").as_bytes()).unwrap();
    let inv = parse(&data, &me()).unwrap();
    let reply = reply_ics(
        &data,
        "maria@example.org",
        "Maria Muster",
        Answer::Tentative,
        "Komme etwas später",
        utc(2026, 10, 17, 8, 0),
    )
    .unwrap();
    let back = parse(&reply, &me()).unwrap();
    assert_eq!((back.method.as_str(), back.uid.as_str(), back.sequence), ("REPLY", inv.uid.as_str(), 2));
    assert_eq!(back.attendees.len(), 1, "only the user's own line");
    assert_eq!(back.my_status.as_deref(), Some("TENTATIVE"));
    assert!(
        reply.contains("ORGANIZER;CN=Jonas Berger:mailto:jonas@example.org")
            && reply.contains("COMMENT:Komme etwas später")
    );
    assert!(reply.contains("BEGIN:VTIMEZONE"), "the zone its times name");
    let (mail, to) = reply_mail(&acc, &inv, &data, Answer::Accepted, "").unwrap();
    assert_eq!(to, "jonas@example.org");
    let text = String::from_utf8_lossy(&mail);
    assert!(
        text.contains("Subject: Accepted: Projektstart Website")
            || text.contains("Subject: Zugesagt: Projektstart Website"),
        "{text}"
    );
    assert!(text.contains("method=REPLY") || text.contains("method=\"REPLY\""), "{text}");
    let parsed = find_in_message(&mail).unwrap();
    assert_eq!(parse(&parsed, &me()).unwrap().my_status.as_deref(), Some("ACCEPTED"));
}

#[test]
fn accepted_invitations_are_filed_updated_and_cancelled() {
    let (s, _, inbox) = store();
    let cal = calendar::local_calendar(&s).unwrap();
    // Something else at the same time: a conflict on the card.
    calendar::create(
        &s,
        cal,
        &calendar::Fields {
            title: Some("Zahnarzt".into()),
            start: Some(utc(2026, 11, 3, 9, 30)),
            end: Some(utc(2026, 11, 3, 10, 30)),
            ..Default::default()
        },
        &me(),
    )
    .unwrap();
    let first = deliver(&s, inbox, 1, &invite("REQUEST", 0, "20261103T100000", "20261103T110000", ""));
    let data = find_in_message(&s.raw_message(first).unwrap().unwrap()).unwrap();
    let c = card(&s, first, &data).unwrap().unwrap();
    assert_eq!(c.conflicts.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Zahnarzt"]);
    assert!(c.answer.is_none() && c.event_key.is_none() && !c.outdated);

    // Tentative: in the calendar, hatched, the user's answer in it.
    let key = file(&s, &data, &c.invitation, Answer::Tentative, cal).unwrap().unwrap();
    answered(&s, first, &c.invitation, Answer::Tentative).unwrap();
    let ev = calendar::event(&s, &key, &me()).unwrap();
    assert_eq!(
        (ev.title.as_str(), ev.busy, ev.editable),
        ("Projektstart Website", Busy::Tentative, Editability::Received)
    );
    assert_eq!(ev.link_kind.as_deref(), Some("Teams"));
    let c = card(&s, first, &data).unwrap().unwrap();
    assert_eq!((c.answer.as_deref(), c.event_key.as_deref()), (Some("tentative"), Some(key.as_str())));
    // Accepted after all: the same appointment.
    assert_eq!(file(&s, &data, &c.invitation, Answer::Accepted, cal).unwrap().as_deref(), Some(key.as_str()));
    assert_eq!(calendar::event(&s, &key, &me()).unwrap().busy, Busy::Busy);

    // The organizer moves it an hour later: applied when the mail is scanned, the answer stays.
    let moved = deliver(&s, inbox, 2, &invite("REQUEST", 1, "20261103T110000", "20261103T120000", ""));
    assert!(scan(&s).unwrap());
    let ev = calendar::event(&s, &key, &me()).unwrap();
    assert_eq!(ev.start, utc(2026, 11, 3, 10, 0));
    let o = calendar::object(&s, ev.object_id).unwrap();
    assert!(o.data.contains("PARTSTAT=ACCEPTED"), "{}", o.data);
    // The first card is outdated now; an old update does nothing.
    assert!(card(&s, first, &data).unwrap().unwrap().outdated);
    let older = find_in_message(invite("REQUEST", 0, "20261103T150000", "20261103T160000", "").as_bytes()).unwrap();
    assert!(!apply(&s, &older, &parse(&older, &me()).unwrap()).unwrap());
    let _ = moved;
    // Cancelled: gone.
    deliver(&s, inbox, 3, &invite("CANCEL", 2, "20261103T110000", "20261103T120000", "STATUS:CANCELLED\r\n"));
    assert!(scan(&s).unwrap());
    assert!(calendar::event(&s, &key, &me()).is_err());
    assert!(!scan(&s).unwrap(), "each message once");
}

#[test]
fn declining_keeps_the_calendar_free() {
    let (s, _, inbox) = store();
    let cal = calendar::local_calendar(&s).unwrap();
    let id = deliver(&s, inbox, 1, &invite("REQUEST", 0, "20261104T100000", "20261104T110000", ""));
    let data = find_in_message(&s.raw_message(id).unwrap().unwrap()).unwrap();
    let inv = parse(&data, &me()).unwrap();
    assert!(file(&s, &data, &inv, Answer::Declined, cal).unwrap().is_none());
    let key = file(&s, &data, &inv, Answer::Accepted, cal).unwrap().unwrap();
    assert!(file(&s, &data, &inv, Answer::Declined, cal).unwrap().is_none());
    assert!(calendar::event(&s, &key, &me()).is_err(), "declined later: removed");
}

#[test]
fn an_update_to_attendees_of_the_users_own_meeting() {
    let (s, account, _) = store();
    let acc = s.account(account).unwrap();
    let own = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:own-1\r\nDTSTART:20261105T080000Z\r\nDTEND:20261105T090000Z\r\n\
SUMMARY:Kickoff\r\nORGANIZER:mailto:maria@example.org\r\nATTENDEE;CN=Lea:mailto:lea@example.org\r\nATTENDEE:mailto:maria@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let (mail, to) = update_mail(&acc, own).unwrap().unwrap();
    assert_eq!(to, ["lea@example.org"]);
    let text = String::from_utf8_lossy(&mail);
    assert!(text.contains("method=REQUEST") || text.contains("method=\"REQUEST\""));
    let data = find_in_message(&mail).unwrap();
    let inv = parse(&data, &["lea@example.org".to_string()]).unwrap();
    assert_eq!((inv.method.as_str(), inv.title.as_str()), ("REQUEST", "Kickoff"));
    assert!(
        update_mail(&acc, &own.replace("ATTENDEE;CN=Lea:mailto:lea@example.org\r\n", "")).unwrap().is_none(),
        "nobody to tell"
    );
}

#[test]
fn a_date_from_a_mail_becomes_a_draft() {
    let item = crate::ai::extract::Item {
        kind: crate::ai::extract::Kind::Event,
        title: "Abstimmung Angebot".into(),
        date: Some("2026-11-12".into()),
        time: Some("14:30".into()),
        end: Some("15:15".into()),
        place: Some("Telefon".into()),
        at: None,
    };
    let d = draft_from(&item, "Aus der Mail von Jonas").unwrap();
    assert_eq!(d.end - d.start, Duration::minutes(45));
    assert!(!d.all_day);
    assert_eq!((d.title.as_str(), d.location.as_str()), ("Abstimmung Angebot", "Telefon"));
    let day = draft_from(&crate::ai::extract::Item { time: None, end: None, ..item }, "").unwrap();
    assert!(day.all_day && day.end - day.start == Duration::days(1));
}
