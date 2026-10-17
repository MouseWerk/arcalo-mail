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
    assert!(!apply(&s, &older, &parse(&older, &me()).unwrap(), Some("jonas@example.org")).unwrap());
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

#[test]
fn only_the_organizer_moves_or_cancels_a_meeting() {
    let (s, account, inbox) = store();
    let cal = calendar::local_calendar(&s).unwrap();
    let first = deliver(&s, inbox, 1, &invite("REQUEST", 0, "20261103T100000", "20261103T110000", ""));
    let data = find_in_message(&s.raw_message(first).unwrap().unwrap()).unwrap();
    let inv = parse(&data, &me()).unwrap();
    let key = file(&s, &data, &inv, Answer::Accepted, cal).unwrap().unwrap();
    assert!(!scan(&s).unwrap(), "the invitation itself changes nothing");
    let from_mallory =
        |raw: String| raw.replace("From: Jonas Berger <jonas@example.org>", "From: Mallory <mallory@evil.example>");
    let mallory_organizes = |raw: String| {
        raw.replace(
            "ORGANIZER;CN=Jonas Berger:mailto:jonas@example.org",
            "ORGANIZER;CN=Mallory:mailto:mallory@evil.example",
        )
    };
    // Someone else who knows the UID moves it, takes it over, cancels it: nothing happens.
    deliver(&s, inbox, 2, &from_mallory(invite("REQUEST", 1, "20261103T150000", "20261103T160000", "")));
    deliver(
        &s,
        inbox,
        3,
        &from_mallory(mallory_organizes(invite("REQUEST", 2, "20261103T150000", "20261103T160000", ""))),
    );
    deliver(
        &s,
        inbox,
        4,
        &from_mallory(invite("CANCEL", 3, "20261103T100000", "20261103T110000", "STATUS:CANCELLED\r\n")),
    );
    assert!(!scan(&s).unwrap());
    let ev = calendar::event(&s, &key, &me()).unwrap();
    assert_eq!(ev.start, utc(2026, 11, 3, 9, 0));
    assert!(calendar::object(&s, ev.object_id).unwrap().data.contains("mailto:jonas@example.org"));
    // Not even the organizer's address can hand the meeting to someone else.
    deliver(&s, inbox, 5, &mallory_organizes(invite("REQUEST", 4, "20261103T150000", "20261103T160000", "")));
    assert!(!scan(&s).unwrap());
    // A cancellation in Junk is not applied either.
    let junk = s
        .upsert_mailbox(
            account,
            &MailboxEntry { name: "Junk".into(), delimiter: Some('/'), attributes: vec!["\\Junk".into()] },
        )
        .unwrap();
    deliver(&s, junk, 1, &invite("CANCEL", 5, "20261103T100000", "20261103T110000", "STATUS:CANCELLED\r\n"));
    assert!(!scan(&s).unwrap());
    assert!(calendar::event(&s, &key, &me()).is_ok());
    // The organizer himself: applied.
    deliver(&s, inbox, 6, &invite("REQUEST", 6, "20261103T110000", "20261103T120000", ""));
    assert!(scan(&s).unwrap());
    assert_eq!(calendar::event(&s, &key, &me()).unwrap().start, utc(2026, 11, 3, 10, 0));
}

const SERIES: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:series-1\r\nSEQUENCE:0\r\n\
DTSTAMP:20261001T080000Z\r\nDTSTART:20261104T090000Z\r\nDTEND:20261104T100000Z\r\nRRULE:FREQ=WEEKLY;COUNT=10\r\n\
SUMMARY:Jour fixe\r\nORGANIZER:mailto:jonas@example.org\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:maria@example.org\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";

fn series_starts(s: &Store) -> Vec<DateTime<Utc>> {
    calendar::events(s, utc(2026, 11, 1, 0, 0), utc(2027, 2, 1, 0, 0), &me(), true)
        .unwrap()
        .into_iter()
        .filter(|e| e.uid == "series-1")
        .map(|e| e.start)
        .collect()
}

fn instance_mail(method: &str, sequence: u32, rid: &str, start: &str, end: &str, extra: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nMETHOD:{method}\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:series-1\r\nSEQUENCE:{sequence}\r\n\
DTSTAMP:20261015T080000Z\r\nRECURRENCE-ID:{rid}\r\nDTSTART:{start}\r\nDTEND:{end}\r\nSUMMARY:Jour fixe\r\n\
ORGANIZER:mailto:jonas@example.org\r\nATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:maria@example.org\r\n{extra}\
END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn one_instance_of_a_series_is_cancelled_or_moved_alone() {
    let (s, _, _) = store();
    let cal = calendar::local_calendar(&s).unwrap();
    calendar::insert_internal(&s, cal, "series-1", SERIES).unwrap();
    assert_eq!(series_starts(&s).len(), 10);
    // One meeting cancelled: the other nine stay.
    let cancel =
        instance_mail("CANCEL", 1, "20261111T090000Z", "20261111T090000Z", "20261111T100000Z", "STATUS:CANCELLED\r\n");
    let inv = parse(&cancel, &me()).unwrap();
    assert!(apply(&s, &cancel, &inv, Some("jonas@example.org")).unwrap());
    let starts = series_starts(&s);
    assert_eq!(starts.len(), 9);
    assert!(!starts.contains(&utc(2026, 11, 11, 9, 0)));
    // Another one moved to the afternoon: ten minus one, one of them later.
    let moved = instance_mail("REQUEST", 1, "20261118T090000Z", "20261118T130000Z", "20261118T140000Z", "");
    let inv = parse(&moved, &me()).unwrap();
    assert!(apply(&s, &moved, &inv, Some("jonas@example.org")).unwrap());
    let starts = series_starts(&s);
    assert_eq!(starts.len(), 9);
    assert!(starts.contains(&utc(2026, 11, 18, 13, 0)) && !starts.contains(&utc(2026, 11, 18, 9, 0)));
    let o = &calendar::objects_by_uid(&s, "series-1").unwrap()[0];
    assert_eq!(o.data.matches("PARTSTAT=ACCEPTED").count(), 2, "the user's answer holds for the moved one: {}", o.data);
    assert!(o.reanswer, "moved since the user accepted");
    // The same message again changes nothing.
    assert!(!apply(&s, &moved, &inv, Some("jonas@example.org")).unwrap());
    // The organizer updates the whole series (a new room): the cancelled and the moved meeting stay so.
    let update = SERIES
        .replace("BEGIN:VCALENDAR\r\n", "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\n")
        .replace("SEQUENCE:0", "SEQUENCE:2")
        .replace("SUMMARY:Jour fixe", "SUMMARY:Jour fixe\r\nLOCATION:Raum B")
        .replace("PARTSTAT=ACCEPTED", "PARTSTAT=NEEDS-ACTION");
    let inv = parse(&update, &me()).unwrap();
    assert!(apply(&s, &update, &inv, Some("jonas@example.org")).unwrap());
    let starts = series_starts(&s);
    assert_eq!(starts.len(), 9, "{starts:?}");
    assert!(!starts.contains(&utc(2026, 11, 11, 9, 0)) && starts.contains(&utc(2026, 11, 18, 13, 0)));
    let all = calendar::events(&s, utc(2026, 11, 1, 0, 0), utc(2027, 2, 1, 0, 0), &me(), true).unwrap();
    assert!(
        all.iter()
            .filter(|e| e.instance != "2026-11-18T09:00:00Z")
            .all(|e| e.location == "Raum B" || e.start == utc(2026, 11, 18, 13, 0))
    );
    let o = &calendar::objects_by_uid(&s, "series-1").unwrap()[0];
    assert!(!o.data.contains("PARTSTAT=NEEDS-ACTION"), "the answer stays: {}", o.data);
    // The whole series cancelled: gone.
    let cancel_all = update.replace("METHOD:REQUEST", "METHOD:CANCEL").replace("SEQUENCE:2", "SEQUENCE:3");
    let inv = parse(&cancel_all, &me()).unwrap();
    assert!(apply(&s, &cancel_all, &inv, Some("jonas@example.org")).unwrap());
    assert!(series_starts(&s).is_empty());
}

#[test]
fn an_update_never_answers_for_the_user() {
    for status in ["NEEDS-ACTION", "DECLINED", "TENTATIVE", "ACCEPTED"] {
        let (s, _, _) = store();
        let src =
            calendar::add_source(&s, calendar::SourceKind::Caldav, "Dav", "https://dav.example.org/", "maria", None)
                .unwrap();
        let cal = calendar::upsert_calendar(&s, src, "https://dav.example.org/cal/home/", "Home", None, true).unwrap();
        let server_copy = SERIES
            .replace("RRULE:FREQ=WEEKLY;COUNT=10\r\n", "")
            .replace("PARTSTAT=ACCEPTED", &format!("PARTSTAT={status}"));
        calendar::put_object(
            &s,
            cal,
            "https://dav.example.org/cal/home/series-1.ics",
            Some("\"e1\""),
            "ics",
            &server_copy,
        )
        .unwrap();
        // A new room: nothing to answer again.
        let update = server_copy
            .replace("BEGIN:VCALENDAR\r\n", "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\n")
            .replace("SEQUENCE:0", "SEQUENCE:1")
            .replace("SUMMARY:Jour fixe", "SUMMARY:Jour fixe (Raum B)")
            .replace(&format!("PARTSTAT={status}"), "PARTSTAT=NEEDS-ACTION;RSVP=TRUE");
        let inv = parse(&update, &me()).unwrap();
        assert!(apply(&s, &update, &inv, Some("jonas@example.org")).unwrap());
        let o = &calendar::objects_by_uid(&s, "series-1").unwrap()[0];
        let line = o.data.lines().find(|l| l.starts_with("ATTENDEE")).unwrap().to_string();
        assert!(line.contains(&format!("PARTSTAT={status}")), "{status}: {line}");
        assert!(o.data.contains("Raum B") && !o.reanswer);
        // Moved: the answer still stays, the appointment asks for a new one if there was one.
        let moved = update
            .replace("SEQUENCE:1", "SEQUENCE:2")
            .replace("DTSTART:20261104T090000Z", "DTSTART:20261104T140000Z")
            .replace("DTEND:20261104T100000Z", "DTEND:20261104T150000Z");
        let inv = parse(&moved, &me()).unwrap();
        assert!(apply(&s, &moved, &inv, Some("jonas@example.org")).unwrap());
        let o = &calendar::objects_by_uid(&s, "series-1").unwrap()[0];
        assert!(o.data.contains(&format!("PARTSTAT={status}")), "{status}: {}", o.data);
        assert_eq!(o.reanswer, status != "NEEDS-ACTION", "{status}");
        let ev = calendar::event(&s, &format!("{cal}|{}|", o.id), &me()).unwrap();
        assert_eq!(ev.reanswer, o.reanswer);
        // Answering again clears it.
        file(&s, &moved, &inv, Answer::Accepted, cal).unwrap();
        assert!(!calendar::object(&s, o.id).unwrap().reanswer);
    }
}

#[test]
fn a_change_without_a_new_sequence_counts_by_its_stamp() {
    let (s, _, inbox) = store();
    let cal = calendar::local_calendar(&s).unwrap();
    let first = deliver(&s, inbox, 1, &invite("REQUEST", 0, "20261103T100000", "20261103T110000", ""));
    let data = find_in_message(&s.raw_message(first).unwrap().unwrap()).unwrap();
    let inv = parse(&data, &me()).unwrap();
    let key = file(&s, &data, &inv, Answer::Accepted, cal).unwrap().unwrap();
    // The same SEQUENCE, a later DTSTAMP: another room.
    let later = invite("REQUEST", 0, "20261103T100000", "20261103T110000", "")
        .replace("DTSTAMP:20261016T080000Z", "DTSTAMP:20261017T090000Z")
        .replace("LOCATION:Raum Elbe", "LOCATION:Raum Spree")
        .replace("Message-ID: <inv-REQUEST-0@example.org>", "Message-ID: <inv-later@example.org>");
    deliver(&s, inbox, 2, &later);
    assert!(scan(&s).unwrap());
    assert_eq!(calendar::event(&s, &key, &me()).unwrap().location, "Raum Spree");
    // An older stamp does not undo it.
    let older = invite("REQUEST", 0, "20261103T100000", "20261103T110000", "")
        .replace("DTSTAMP:20261016T080000Z", "DTSTAMP:20261015T090000Z")
        .replace("LOCATION:Raum Elbe", "LOCATION:Raum Alt");
    let d = find_in_message(older.as_bytes()).unwrap();
    assert!(!apply(&s, &d, &parse(&d, &me()).unwrap(), Some("jonas@example.org")).unwrap());
    assert_eq!(calendar::event(&s, &key, &me()).unwrap().location, "Raum Spree");
}

#[test]
fn invitations_in_large_messages_are_read_from_their_part() {
    let (s, _, inbox) = store();
    let cal = calendar::local_calendar(&s).unwrap();
    let raw = invite("REQUEST", 0, "20261103T100000", "20261103T110000", "");
    let id = deliver(&s, inbox, 1, &raw);
    let key = file(
        &s,
        &find_in_message(raw.as_bytes()).unwrap(),
        &parse(&find_in_message(raw.as_bytes()).unwrap(), &me()).unwrap(),
        Answer::Accepted,
        cal,
    )
    .unwrap()
    .unwrap();
    let _ = id;
    // A large update: only the text and the parts came, not the whole message.
    let update = invite("REQUEST", 1, "20261103T140000", "20261103T150000", "");
    let p = parse_mail(update.as_bytes()).unwrap();
    let big = s.insert_message(inbox, &header(2, update.as_bytes()), &p).unwrap();
    let parts = crate::store::PartialBody {
        text: Some("Wann: später".into()),
        snippet: "Wann: später".into(),
        attachments: p.attachments.iter().map(|a| (a.clone(), format!("2.{}", a.index + 1))).collect(),
        ..Default::default()
    };
    s.set_body_parts(big, &p, &parts).unwrap();
    assert!(calendar_data(&s, big).unwrap().is_none(), "not loaded yet");
    assert!(!scan(&s).unwrap(), "waits");
    let ics = find_in_message(update.as_bytes()).unwrap();
    for a in s
        .attachments(big)
        .unwrap()
        .iter()
        .filter(|a| a.mime_type == "text/calendar" || a.mime_type == "application/ics")
    {
        s.set_attachment_data(big, a.section.as_deref().unwrap(), ics.as_bytes()).unwrap();
    }
    assert!(calendar_data(&s, big).unwrap().is_some_and(|d| d.contains("SEQUENCE:1")));
    assert!(scan(&s).unwrap(), "applied once loaded");
    assert_eq!(calendar::event(&s, &key, &me()).unwrap().start, utc(2026, 11, 3, 13, 0));
    let c = card(&s, big, &calendar_data(&s, big).unwrap().unwrap()).unwrap().unwrap();
    assert_eq!(c.invitation.sequence, 1);
    // A message whose calendar part cannot be read is looked at once.
    let broken = invite("REQUEST", 2, "20261103T140000", "20261103T150000", "").replace("UID:", "X-NO-UID:");
    deliver(&s, inbox, 3, &broken);
    assert!(!scan(&s).unwrap());
    let seen: i64 = s.conn().query_row("SELECT COUNT(*) FROM itip_seen", [], |r| r.get(0)).unwrap();
    assert!(!scan(&s).unwrap());
    assert_eq!(s.conn().query_row("SELECT COUNT(*) FROM itip_seen", [], |r| r.get::<_, i64>(0)).unwrap(), seen);
}

#[test]
fn a_changed_instance_leaves_the_series_invitation_open() {
    let (s, _, inbox) = store();
    let weekly = "RRULE:FREQ=WEEKLY;COUNT=4\r\n";
    let series = deliver(&s, inbox, 1, &invite("REQUEST", 0, "20261103T100000", "20261103T110000", weekly));
    let one = invite(
        "REQUEST",
        1,
        "20261110T140000",
        "20261110T150000",
        "RECURRENCE-ID;TZID=W. Europe Standard Time:20261110T100000\r\n",
    )
    .replace("Message-ID: <inv-REQUEST-1@example.org>", "Message-ID: <inv-one@example.org>");
    let instance = deliver(&s, inbox, 2, &one);
    scan(&s).unwrap();
    let data = |id| find_in_message(&s.raw_message(id).unwrap().unwrap()).unwrap();
    assert!(!card(&s, series, &data(series)).unwrap().unwrap().outdated, "the series is still to be answered");
    assert!(!card(&s, instance, &data(instance)).unwrap().unwrap().outdated);
    // The organizer changes the whole series: both are outdated.
    let all = deliver(&s, inbox, 3, &invite("REQUEST", 2, "20261103T090000", "20261103T100000", weekly));
    scan(&s).unwrap();
    assert!(card(&s, series, &data(series)).unwrap().unwrap().outdated);
    assert!(card(&s, instance, &data(instance)).unwrap().unwrap().outdated);
    assert!(!card(&s, all, &data(all)).unwrap().unwrap().outdated);
}

#[test]
fn declining_one_instance_keeps_the_series() {
    let (s, _, _) = store();
    let cal = calendar::local_calendar(&s).unwrap();
    calendar::insert_internal(&s, cal, "series-1", SERIES).unwrap();
    let one = instance_mail("REQUEST", 1, "20261125T090000Z", "20261125T090000Z", "20261125T100000Z", "");
    let inv = parse(&one, &me()).unwrap();
    assert!(file(&s, &one, &inv, Answer::Declined, cal).unwrap().is_some(), "the series stays");
    let starts = series_starts(&s);
    assert_eq!(starts.len(), 9);
    assert!(!starts.contains(&utc(2026, 11, 25, 9, 0)));
    // Accepting another one tentatively: that instance carries the answer, the rest keeps its own.
    let other = instance_mail("REQUEST", 1, "20261202T090000Z", "20261202T090000Z", "20261202T100000Z", "");
    file(&s, &other, &parse(&other, &me()).unwrap(), Answer::Tentative, cal).unwrap();
    let evs = calendar::events(&s, utc(2026, 11, 1, 0, 0), utc(2027, 2, 1, 0, 0), &me(), true).unwrap();
    assert_eq!(evs.len(), 9);
    let busy: Vec<Busy> = evs.iter().map(|e| e.busy).collect();
    assert_eq!(busy.iter().filter(|b| **b == Busy::Tentative).count(), 1, "{busy:?}");
}
