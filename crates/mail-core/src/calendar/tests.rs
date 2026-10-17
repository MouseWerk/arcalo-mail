//! The calendar end to end: the store's view of objects and series, changes with undo, and one
//! round of each source against its fake server on loopback (CalDAV with sync tokens and ETags,
//! Graph with delta links and change keys, JMAP), conflicts and their two ways out.

use std::sync::{Arc, Mutex};

use chrono::{Duration, TimeZone, Utc};
use serde_json::json;

use super::sync::{Access, sync_source};
use super::*;
use crate::network::tests::testhttp::{Reply, serve};

fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
}

fn week() -> (DateTime<Utc>, DateTime<Utc>) {
    (utc(2026, 11, 2, 0, 0), utc(2026, 11, 9, 0, 0))
}

fn me() -> Vec<String> {
    vec!["maria@example.org".to_string()]
}

fn fields(title: &str, start: DateTime<Utc>, minutes: i64) -> Fields {
    Fields {
        title: Some(title.into()),
        start: Some(start),
        end: Some(start + Duration::minutes(minutes)),
        ..Fields::default()
    }
}

fn ics_event(uid: &str, title: &str, start: &str, end: &str, extra: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261001T000000Z\r\nDTSTART:{start}\r\nDTEND:{end}\r\nSUMMARY:{title}\r\n{extra}END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[test]
fn meeting_links_and_their_names() {
    let body = "Einwahl: https://eu01web.zoom.us/j/123?pwd=x. Oder https://example.org/agenda";
    assert_eq!(meeting_link([body]).as_deref(), Some("https://eu01web.zoom.us/j/123?pwd=x"));
    assert_eq!(meeting_kind("https://eu01web.zoom.us/j/1"), "Zoom");
    for (url, kind) in [
        ("https://teams.microsoft.com/l/meetup-join/19%3a1", "Teams"),
        ("https://meet.google.com/abc-defg-hij", "Google Meet"),
        ("https://firma.webex.com/meet/anna", "Webex"),
        ("https://meet.jit.si/ArcaloTeam", "Jitsi"),
        ("https://jitsi.example.org/Wochenrunde", "Jitsi"),
    ] {
        assert_eq!(meeting_link([url]).as_deref(), Some(url), "{url}");
        assert_eq!(meeting_kind(url), kind, "{url}");
    }
    assert!(meeting_link(["https://jitsi.example.org/"]).is_none(), "a server's start page is no meeting");
    let safe = "https://eur01.safelinks.protection.outlook.com/?url=https%3A%2F%2Fteams.microsoft.com%2Fl%2Fmeetup-join%2Fx&data=1";
    assert_eq!(meeting_link([safe]).as_deref(), Some("https://teams.microsoft.com/l/meetup-join/x"));
    assert!(meeting_link(["nichts hier", "https://example.org"]).is_none());
    assert_eq!(attendee_email("Anna <ANNA@Firma.de>").as_deref(), Some("anna@firma.de"));
    assert_eq!(attendee_email("mailto:jo@x.de").as_deref(), Some("jo@x.de"));
    assert_eq!(attendee_name("Anna <anna@firma.de>"), "Anna");
}

#[test]
fn this_device_creates_moves_deletes_and_undoes() {
    let s = Store::open_in_memory().unwrap();
    let cal = default_calendar(&s, None).unwrap();
    assert_eq!(calendar(&s, cal).unwrap().kind, SourceKind::Local, "without accounts: this device");
    let (ev, created) = create(&s, cal, &fields("Zahnarzt", utc(2026, 11, 3, 9, 0), 45), &me()).unwrap();
    assert_eq!((ev.title.as_str(), ev.editable), ("Zahnarzt", Editability::Yes));
    assert!(ev.write.is_none(), "this device has no server");
    assert_eq!(events(&s, week().0, week().1, &me(), false).unwrap().len(), 1);
    // Moved to the next day an hour later: the length stays.
    let (moved, before) =
        update(&s, &ev.key, &Fields { start: Some(utc(2026, 11, 4, 10, 0)), ..Fields::default() }, false, &me())
            .unwrap();
    assert_eq!((moved.start, moved.end), (utc(2026, 11, 4, 10, 0), utc(2026, 11, 4, 10, 45)));
    let back = undo(&s, &before, &me()).unwrap().unwrap();
    assert_eq!(back.start, utc(2026, 11, 3, 9, 0));
    // Renamed, deleted, brought back.
    update(&s, &ev.key, &Fields { title: Some("Zahnarzt Dr. Weiß".into()), ..Fields::default() }, false, &me())
        .unwrap();
    let gone = delete(&s, &ev.key, &me()).unwrap();
    assert!(events(&s, week().0, week().1, &me(), false).unwrap().is_empty());
    let again = undo(&s, &gone, &me()).unwrap().unwrap();
    assert_eq!(again.title, "Zahnarzt Dr. Weiß");
    // Taking back the creation removes it.
    assert!(undo(&s, &created, &me()).unwrap().is_none());
    assert!(events(&s, week().0, week().1, &me(), false).unwrap().is_empty());
    // Hidden calendars do not show, except for conflicts.
    create(&s, cal, &fields("Sport", utc(2026, 11, 5, 18, 0), 60), &me()).unwrap();
    set_calendar(&s, cal, &CalendarPatch { hidden: Some(true), color: Some("#DB2777".into()), name: None }).unwrap();
    assert!(events(&s, week().0, week().1, &me(), false).unwrap().is_empty());
    assert_eq!(events(&s, week().0, week().1, &me(), true).unwrap().len(), 1);
    assert_eq!(calendar(&s, cal).unwrap().color, "#db2777");
    assert!(update(&s, "9|9|", &Fields::default(), false, &me()).is_err());
}

#[test]
fn series_meetings_and_what_may_be_changed() {
    let s = Store::open_in_memory().unwrap();
    let cal = local_calendar(&s).unwrap();
    // A weekly series in Berlin time across the change to winter time, with an exception.
    let series = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:jf\r\nDTSTART;TZID=Europe/Berlin:20261021T100000\r\n\
        DTEND;TZID=Europe/Berlin:20261021T103000\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\nEXDATE;TZID=Europe/Berlin:20261104T100000\r\nSUMMARY:Jour fixe\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    put_object(&s, cal, "jf", None, "ics", series).unwrap();
    let invited = ics_event(
        "inv",
        "Planung",
        "20261103T130000Z",
        "20261103T140000Z",
        "ORGANIZER;CN=Jonas:mailto:jonas@example.org\r\nATTENDEE;CN=Maria:mailto:maria@example.org\r\n",
    );
    put_object(&s, cal, "inv", None, "ics", &invited).unwrap();
    let mine = ics_event(
        "own",
        "Kickoff",
        "20261105T080000Z",
        "20261105T090000Z",
        "ORGANIZER:mailto:maria@example.org\r\nATTENDEE;CN=Lea:mailto:lea@example.org\r\n",
    );
    put_object(&s, cal, "own", None, "ics", &mine).unwrap();
    let list = events(&s, utc(2026, 10, 19, 0, 0), utc(2026, 11, 16, 0, 0), &me(), false).unwrap();
    let jf: Vec<_> = list.iter().filter(|e| e.uid == "jf").map(|e| e.start).collect();
    assert_eq!(
        jf,
        [utc(2026, 10, 21, 8, 0), utc(2026, 10, 28, 9, 0), utc(2026, 11, 11, 9, 0)],
        "10:00 Berlin on both sides of 25 October"
    );
    let by = |uid: &str| list.iter().find(|e| e.uid == uid).unwrap().clone();
    assert_eq!(by("jf").editable, Editability::Series);
    assert_eq!(by("inv").editable, Editability::Received);
    assert_eq!(by("own").editable, Editability::Confirm);
    let key = by("own").key;
    let title = |t: &str| Fields { title: Some(t.into()), ..Fields::default() };
    assert!(update(&s, &key, &title("x"), false, &me()).is_err(), "only after the confirmation");
    assert!(update(&s, &key, &title("Kickoff neu"), true, &me()).is_ok());
    assert!(delete(&s, &by("inv").key, &me()).is_err());
    assert!(update(&s, &by("jf").key, &title("x"), false, &me()).is_err());
    assert_eq!(event(&s, &by("jf").key, &me()).unwrap().start, utc(2026, 10, 21, 8, 0));
    // Conflicts of an invitation: busy appointments at the same time, not the invitation itself.
    let c = conflicts(&s, utc(2026, 11, 3, 13, 30), utc(2026, 11, 3, 15, 0), "x").unwrap();
    assert_eq!(c.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Planung"]);
    assert!(conflicts(&s, utc(2026, 11, 3, 13, 30), utc(2026, 11, 3, 15, 0), "inv").unwrap().is_empty());
}

type DavServer = Arc<Mutex<caldav::fake::FakeCalDav>>;

async fn dav_server() -> (DavServer, String) {
    let fake: DavServer = Arc::new(Mutex::new(caldav::fake::FakeCalDav::new("maria", "app-pw")));
    let f = fake.clone();
    let port = serve(Arc::new(move |method: &str, path: &str, headers: &[(String, String)], body: &[u8]| {
        let auth = headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.clone());
        let (status, extra, text) =
            f.lock().unwrap().handle(method, path, auth.as_deref(), headers, &String::from_utf8_lossy(body));
        Reply {
            status,
            content_type: "application/xml".into(),
            body: text.into_bytes(),
            location: None,
            headers: extra,
        }
    }))
    .await;
    (fake, format!("http://127.0.0.1:{port}/caldav"))
}

fn basic() -> Access {
    Access::Basic { user: "maria".into(), password: "app-pw".into() }
}

#[tokio::test]
async fn caldav_discovers_syncs_by_token_and_writes_with_etags() {
    let (fake, base) = dav_server().await;
    let wrong = caldav::CalDav::new("maria", "falsch").unwrap();
    assert!(wrong.discover(&base).await.is_err());
    let dav = caldav::CalDav::new("maria", "app-pw").unwrap();
    let found = dav.discover(&base).await.unwrap();
    assert_eq!(found.calendars.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Privat", "Arbeit"]);
    assert_eq!(found.calendars[1].color.as_deref(), Some("#0D9488FF"));

    fake.lock().unwrap().put(
        caldav::fake::WORK,
        "a.ics",
        &ics_event("a", "Planung", "20261103T090000Z", "20261103T100000Z", ""),
    );
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let source = add_source(&store.lock().unwrap(), SourceKind::Caldav, "Fastmail", &base, "maria", None).unwrap();
    assert!(sync_source(&store, source, &basic()).await.unwrap().changed);
    let work = calendars(&store.lock().unwrap()).unwrap().into_iter().find(|c| c.name == "Arbeit").unwrap();
    assert_eq!(work.color, "#0d9488", "the server's color, alpha dropped");
    let titles = |store: &Mutex<Store>| -> Vec<String> {
        events(&store.lock().unwrap(), week().0, week().1, &me(), false).unwrap().into_iter().map(|e| e.title).collect()
    };
    assert_eq!(titles(&store), ["Planung"]);

    // Nothing changed: nothing comes.
    assert!(!sync_source(&store, source, &basic()).await.unwrap().changed);
    // Another client adds one and removes the first: only that comes (sync-collection).
    {
        let mut f = fake.lock().unwrap();
        f.put(caldav::fake::WORK, "b.ics", &ics_event("b", "Review", "20261104T090000Z", "20261104T100000Z", ""));
        f.remove(&format!("{}a.ics", caldav::fake::WORK));
    }
    assert!(sync_source(&store, source, &basic()).await.unwrap().changed);
    assert_eq!(titles(&store), ["Review"]);
    assert!(fake.lock().unwrap().writes.is_empty(), "reading writes nothing");

    // Made here: shown at once as waiting, on the server after the round, with its ETag.
    let (ev, _) =
        create(&store.lock().unwrap(), work.id, &fields("Workshop", utc(2026, 11, 5, 13, 0), 120), &me()).unwrap();
    assert_eq!(ev.write.as_ref().map(|w| (w.op.as_str(), w.state.as_str())), Some(("create", "pending")));
    sync_source(&store, source, &basic()).await.unwrap();
    let ev = event(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    assert!(ev.write.is_none(), "taken by the server");
    let stored = object(&store.lock().unwrap(), ev.object_id).unwrap();
    assert!(stored.etag.is_some());
    let path = url::Url::parse(&stored.href).unwrap().path().to_string();
    assert!(path.starts_with(caldav::fake::WORK), "{path}");
    assert!(fake.lock().unwrap().calendars[caldav::fake::WORK].objects[&path].1.contains("SUMMARY:Workshop"));

    // Moved here while another client renamed it: a conflict.
    let rename = |fake: &DavServer, from: &str, to: &str| {
        let mut f = fake.lock().unwrap();
        let data = f.calendars[caldav::fake::WORK].objects[&path].1.replace(from, to);
        f.put(caldav::fake::WORK, path.rsplit('/').next().unwrap(), &data);
    };
    rename(&fake, "SUMMARY:Workshop", "SUMMARY:Workshop (Raum 2)");
    update(
        &store.lock().unwrap(),
        &ev.key,
        &Fields { start: Some(utc(2026, 11, 5, 14, 0)), ..Fields::default() },
        false,
        &me(),
    )
    .unwrap();
    sync_source(&store, source, &basic()).await.unwrap();
    let ev = event(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    let w = ev.write.clone().unwrap();
    assert_eq!(w.state, "conflict");
    assert_eq!(w.server.as_ref().and_then(|f| f.title.clone()).as_deref(), Some("Workshop (Raum 2)"));
    assert_eq!(w.mine.as_ref().and_then(|f| f.start), Some(utc(2026, 11, 5, 14, 0)));
    assert_eq!(ev.start, utc(2026, 11, 5, 14, 0), "the change made here shows meanwhile");
    // Keep mine: written over the server's copy, the server's rename stays.
    resolve(&store.lock().unwrap(), &ev.key, true).unwrap();
    sync_source(&store, source, &basic()).await.unwrap();
    let ev = event(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    assert!(ev.write.is_none());
    assert_eq!((ev.title.as_str(), ev.start), ("Workshop (Raum 2)", utc(2026, 11, 5, 14, 0)));

    // Another conflict, settled by taking the server's version.
    rename(&fake, "SUMMARY:Workshop (Raum 2)", "SUMMARY:Abgesagt?");
    update(
        &store.lock().unwrap(),
        &ev.key,
        &Fields { title: Some("Mein Titel".into()), ..Fields::default() },
        false,
        &me(),
    )
    .unwrap();
    sync_source(&store, source, &basic()).await.unwrap();
    resolve(&store.lock().unwrap(), &ev.key, false).unwrap();
    let ev = event(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    assert_eq!(ev.title, "Abgesagt?");
    assert!(ev.write.is_none());

    // Deleted here and taken back before the round: the server never hears of it.
    let undo_delete = delete(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    undo(&store.lock().unwrap(), &undo_delete, &me()).unwrap();
    let writes = fake.lock().unwrap().writes.len();
    sync_source(&store, source, &basic()).await.unwrap();
    assert_eq!(fake.lock().unwrap().writes.len(), writes);
    // Deleted for real.
    let deleted = delete(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    sync_source(&store, source, &basic()).await.unwrap();
    assert!(!fake.lock().unwrap().calendars[caldav::fake::WORK].objects.contains_key(&path));
    assert!(object(&store.lock().unwrap(), ev.object_id).is_err());
    // „Rückgängig“ after the server took the delete: made there again, with the same UID.
    let back = undo(&store.lock().unwrap(), &deleted, &me()).unwrap().expect("back");
    assert_eq!((back.title.as_str(), back.uid.as_str()), ("Abgesagt?", ev.uid.as_str()));
    assert_eq!(back.write.as_ref().map(|w| w.op.as_str()), Some("create"));
    sync_source(&store, source, &basic()).await.unwrap();
    assert!(
        fake.lock().unwrap().calendars[caldav::fake::WORK]
            .objects
            .values()
            .any(|(_, d)| d.contains("SUMMARY:Abgesagt?")),
        "on the server again"
    );
    let back = event(&store.lock().unwrap(), &back.key, &me()).unwrap();
    assert!(back.write.is_none());
    delete(&store.lock().unwrap(), &back.key, &me()).unwrap();
    sync_source(&store, source, &basic()).await.unwrap();
    // A refused sign-in: the change waits with the error, the source says why.
    let refused = Access::Basic { user: "maria".into(), password: "falsch".into() };
    let (ev2, _) =
        create(&store.lock().unwrap(), work.id, &fields("Später", utc(2026, 11, 6, 9, 0), 30), &me()).unwrap();
    assert!(sync_source(&store, source, &refused).await.is_err());
    let s = store.lock().unwrap();
    assert!(super::source(&s, source).unwrap().error.is_some());
    let w = event(&s, &ev2.key, &me()).unwrap().write.unwrap();
    assert_eq!((w.state.as_str(), w.error.is_some()), ("pending", true));
}

type GraphServer = Arc<Mutex<graph::fake::FakeGraphCalendar>>;

async fn graph_server() -> (GraphServer, crate::graph::Graph) {
    let fake: GraphServer = Arc::default();
    let f = fake.clone();
    let base: Arc<Mutex<String>> = Arc::default();
    let b = base.clone();
    let port = serve(Arc::new(move |method: &str, target: &str, headers: &[(String, String)], body: &[u8]| {
        assert!(headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer tok"));
        let base = b.lock().unwrap().clone();
        let (status, text) = f.lock().unwrap().handle(method, target, headers, body, &base);
        Reply::status(status, &text)
    }))
    .await;
    let url = format!("http://127.0.0.1:{port}/v1.0");
    *base.lock().unwrap() = url.clone();
    (fake, crate::graph::Graph::at(&url, "tok", crate::network::Service::Graph).unwrap())
}

#[tokio::test]
async fn graph_calendars_sync_by_delta_and_write_with_change_keys() {
    let (fake, g) = graph_server().await;
    let teams = fake.lock().unwrap().put(
        "C1",
        json!({
            "subject": "Sprint Review",
            "start": {"dateTime": "2026-11-03T13:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-03T14:00:00.0000000", "timeZone": "UTC"},
            "isOrganizer": false,
            "organizer": {"emailAddress": {"name": "Jonas", "address": "jonas@example.org"}},
            "attendees": [{"emailAddress": {"name": "Maria", "address": "maria@example.org"}, "type": "required"}],
            "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/19%3aabc"},
            "showAs": "tentative"
        }),
    );
    fake.lock().unwrap().put(
        "C1",
        json!({
            "subject": "Urlaub",
            "start": {"dateTime": "2026-11-06T00:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-07T00:00:00.0000000", "timeZone": "UTC"},
            "isAllDay": true, "showAs": "oof"
        }),
    );
    fake.lock().unwrap().put(
        "C1",
        json!({
            "subject": "Daily", "type": "occurrence", "seriesMasterId": "S1",
            "start": {"dateTime": "2026-11-04T08:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-04T08:15:00.0000000", "timeZone": "UTC"}
        }),
    );
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let source = add_source(&store.lock().unwrap(), SourceKind::Graph, "Microsoft", "", "", None).unwrap();
    let access = Access::Graph(g);
    sync_source(&store, source, &access).await.unwrap();
    let cals = calendars(&store.lock().unwrap()).unwrap();
    assert_eq!(
        cals.iter().map(|c| (c.name.as_str(), c.writable)).collect::<Vec<_>>(),
        [("Kalender", true), ("Feiertage", false)]
    );
    let list = events(&store.lock().unwrap(), week().0, week().1, &me(), false).unwrap();
    let by = |t: &str| list.iter().find(|e| e.title == t).unwrap().clone();
    let review = by("Sprint Review");
    assert_eq!(review.editable, Editability::Received);
    assert_eq!((review.link_kind.as_deref(), review.busy), (Some("Teams"), Busy::Tentative));
    assert_eq!(review.organizer, "Jonas <jonas@example.org>");
    assert!(by("Urlaub").all_day);
    assert_eq!(by("Daily").editable, Editability::Series);

    // Made here: Graph gives it its id and change key.
    let c1 = cals[0].id;
    let (ev, _) = create(&store.lock().unwrap(), c1, &fields("Fokuszeit", utc(2026, 11, 5, 9, 0), 90), &me()).unwrap();
    sync_source(&store, source, &access).await.unwrap();
    let o = object(&store.lock().unwrap(), ev.object_id).unwrap();
    assert!(o.href.starts_with('E') && o.etag.as_deref().is_some_and(|e| e.starts_with("W/")), "{o:?}");
    let ev = event(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    assert_eq!((ev.title.as_str(), ev.start, ev.editable), ("Fokuszeit", utc(2026, 11, 5, 9, 0), Editability::Yes));
    // Moved here: PATCH with If-Match.
    update(
        &store.lock().unwrap(),
        &ev.key,
        &Fields { start: Some(utc(2026, 11, 5, 10, 0)), ..Fields::default() },
        false,
        &me(),
    )
    .unwrap();
    sync_source(&store, source, &access).await.unwrap();
    assert_eq!(fake.lock().unwrap().events[&o.href].1["start"]["dateTime"], "2026-11-05T10:00:00");
    // Changed in Outlook meanwhile: a conflict.
    {
        let mut f = fake.lock().unwrap();
        let mut e = f.events[&o.href].1.clone();
        e["subject"] = json!("Fokuszeit (Outlook)");
        f.put("C1", e);
    }
    update(&store.lock().unwrap(), &ev.key, &Fields { title: Some("Fokus".into()), ..Fields::default() }, false, &me())
        .unwrap();
    sync_source(&store, source, &access).await.unwrap();
    let w = event(&store.lock().unwrap(), &ev.key, &me()).unwrap().write.unwrap();
    assert_eq!(w.state, "conflict");
    assert_eq!(w.server.and_then(|f| f.title).as_deref(), Some("Fokuszeit (Outlook)"));
    resolve(&store.lock().unwrap(), &ev.key, true).unwrap();
    sync_source(&store, source, &access).await.unwrap();
    assert_eq!(fake.lock().unwrap().events[&o.href].1["subject"], "Fokus");
    // Removed in Outlook: gone here after the next delta.
    fake.lock().unwrap().remove(&teams);
    sync_source(&store, source, &access).await.unwrap();
    let list = events(&store.lock().unwrap(), week().0, week().1, &me(), false).unwrap();
    assert!(list.iter().all(|e| e.title != "Sprint Review"));
    // Deleted here.
    delete(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    sync_source(&store, source, &access).await.unwrap();
    assert!(!fake.lock().unwrap().events.contains_key(&o.href));
    // Answering an invitation found by its UID.
    let Access::Graph(g) = &access else { unreachable!() };
    let inv = fake.lock().unwrap().put(
        "C1",
        json!({"subject": "Einladung", "iCalUId": "040000-abc", "isOrganizer": false,
            "start": {"dateTime": "2026-11-09T10:00:00", "timeZone": "UTC"}, "end": {"dateTime": "2026-11-09T11:00:00", "timeZone": "UTC"}}),
    );
    let found = graph::find_by_uid(g, "040000-abc").await.unwrap().unwrap();
    assert_eq!(found["id"], json!(inv));
    graph::respond(g, &inv, "tentativelyAccept", "").await.unwrap();
    assert_eq!(fake.lock().unwrap().responses, [(inv, "tentativelyAccept".to_string())]);
}

#[test]
fn graph_events_in_their_own_zone() {
    let v = json!({"subject": "NY", "start": {"dateTime": "2026-11-03T09:00:00", "timeZone": "Eastern Standard Time"},
        "end": {"dateTime": "2026-11-03T10:00:00", "timeZone": "Eastern Standard Time"}, "isCancelled": false});
    let o = graph::occurrence(&v.to_string()).unwrap();
    assert_eq!(o.start, utc(2026, 11, 3, 14, 0), "EST after the change on 1 November");
    let edited =
        graph::edit_json(&v.to_string(), &Fields { start: Some(utc(2026, 11, 3, 15, 0)), ..Fields::default() })
            .unwrap();
    let o = graph::occurrence(&edited).unwrap();
    assert_eq!((o.start, o.end), (utc(2026, 11, 3, 15, 0), utc(2026, 11, 3, 16, 0)));
    let cancelled = json!({"subject": "x", "isCancelled": true, "start": v["start"], "end": v["end"]});
    assert!(graph::occurrence(&cancelled.to_string()).is_none());
}

#[test]
fn jscalendar_becomes_icalendar() {
    let e = json!({
        "@type": "Event", "id": "ev1", "uid": "abc@jmap", "title": "Yoga", "start": "2026-10-21T18:00:00",
        "timeZone": "Europe/Berlin", "duration": "PT1H",
        "locations": {"l": {"name": "Studio"}},
        "virtualLocations": {"v": {"uri": "https://meet.jit.si/yoga"}},
        "participants": {"o": {"email": "lea@example.org", "name": "Lea", "roles": {"owner": true}},
                          "a": {"email": "maria@example.org", "roles": {"attendee": true}, "participationStatus": "accepted"}},
        "recurrenceRules": [{"frequency": "weekly", "byDay": [{"day": "we"}], "count": 3}],
        "recurrenceOverrides": {"2026-10-28T18:00:00": {"excluded": true}}
    });
    let ics = jmap::to_ics(&e).unwrap();
    let list = ics::parse(ics.as_bytes(), (utc(2026, 10, 1, 0, 0), utc(2026, 12, 1, 0, 0)), &tz::Zone::Utc).unwrap();
    assert_eq!(list.iter().map(|o| o.start).collect::<Vec<_>>(), [utc(2026, 10, 21, 16, 0), utc(2026, 11, 4, 17, 0)]);
    assert_eq!(list[0].location, "Studio");
    assert_eq!(list[0].link.as_deref(), Some("https://meet.jit.si/yoga"));
    assert_eq!(list[0].organizer, "Lea <lea@example.org>");
    let patch = jmap::patch_from_ics(&ics).unwrap();
    assert_eq!(
        (patch["start"].as_str(), patch["timeZone"].as_str(), patch["duration"].as_str()),
        (Some("2026-10-21T18:00:00"), Some("Europe/Berlin"), Some("PT1H0M0S"))
    );
}

#[tokio::test]
async fn jmap_calendars_where_the_server_offers_them() {
    use crate::imap::fake::FakeImap;
    use crate::jmap::fake::{FakeJmapState, authorized};
    let mut state = FakeJmapState::new("maria@example.org", false);
    let mut cals = jmap::fake::FakeJmapCalendars::default();
    cals.put(
        json!({"@type": "Event", "calendarIds": {"cal1": true}, "title": "Lauftreff", "start": "2026-11-04T18:00:00",
        "timeZone": "Europe/Berlin", "duration": "PT1H"}),
    );
    state.calendars = Some(cals);
    let server = Arc::new(Mutex::new((FakeImap::default().with_all_folders(), state)));
    let s2 = server.clone();
    let base: Arc<Mutex<String>> = Arc::default();
    let b = base.clone();
    let port = serve(Arc::new(move |method: &str, target: &str, headers: &[(String, String)], body: &[u8]| {
        if !authorized(headers, "maria@example.org", "pw") {
            return Reply::status(401, "{}");
        }
        let base = b.lock().unwrap().clone();
        let mut g = s2.lock().unwrap();
        let (fake, jmap) = &mut *g;
        let r = jmap.handle(fake, method, target, body, &base);
        Reply { status: r.status, content_type: r.content_type, body: r.body, location: r.location, headers: vec![] }
    }))
    .await;
    *base.lock().unwrap() = format!("http://127.0.0.1:{port}");
    let auth = crate::jmap::JmapAuth::Basic { user: "maria@example.org".into(), password: "pw".into() };
    let client =
        crate::jmap::JmapClient::connect(&format!("http://127.0.0.1:{port}/jmap/session"), &auth).await.unwrap();
    assert!(jmap::account_of(&client).is_some());
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let source = add_source(&store.lock().unwrap(), SourceKind::Jmap, "Fastmail", "", "", None).unwrap();
    let access = Access::Jmap(Box::new(client));
    sync_source(&store, source, &access).await.unwrap();
    let list = events(&store.lock().unwrap(), week().0, week().1, &me(), false).unwrap();
    assert_eq!(
        list.iter().map(|e| (e.title.as_str(), e.start)).collect::<Vec<_>>(),
        [("Lauftreff", utc(2026, 11, 4, 17, 0))]
    );
    let all = calendars(&store.lock().unwrap()).unwrap();
    assert!(!all.iter().find(|c| c.name == "Team").unwrap().writable);
    let personal = all.iter().find(|c| c.name == "Persönlich").unwrap().id;
    let (ev, _) =
        create(&store.lock().unwrap(), personal, &fields("Abendessen", utc(2026, 11, 6, 18, 0), 120), &me()).unwrap();
    sync_source(&store, source, &access).await.unwrap();
    let titles_there = |server: &Arc<Mutex<(FakeImap, FakeJmapState)>>| -> Vec<String> {
        let s = server.lock().unwrap();
        s.1.calendars.as_ref().unwrap().events.values().filter_map(|e| e["title"].as_str().map(str::to_owned)).collect()
    };
    assert!(titles_there(&server).contains(&"Abendessen".to_string()));
    update(
        &store.lock().unwrap(),
        &ev.key,
        &Fields { title: Some("Abendessen bei Lea".into()), ..Fields::default() },
        false,
        &me(),
    )
    .unwrap();
    sync_source(&store, source, &access).await.unwrap();
    assert!(titles_there(&server).contains(&"Abendessen bei Lea".to_string()));
    delete(&store.lock().unwrap(), &ev.key, &me()).unwrap();
    sync_source(&store, source, &access).await.unwrap();
    assert!(!titles_there(&server).iter().any(|t| t.starts_with("Abendessen")));
    // Changed on the server: comes with the state.
    server.lock().unwrap().1.calendars.as_mut().unwrap().put(json!({"@type": "Event", "calendarIds": {"cal2": true},
        "title": "Teamessen", "start": "2026-11-05T12:00:00", "timeZone": "Europe/Berlin", "duration": "PT1H"}));
    sync_source(&store, source, &access).await.unwrap();
    let titles: Vec<String> = events(&store.lock().unwrap(), week().0, week().1, &me(), false)
        .unwrap()
        .into_iter()
        .map(|e| e.title)
        .collect();
    assert_eq!(titles, ["Lauftreff", "Teamessen"]);
}

#[tokio::test]
async fn subscriptions_are_read_only_and_follow_their_etag() {
    let port = serve(Arc::new(move |_m: &str, _p: &str, headers: &[(String, String)], _b: &[u8]| {
        if headers.iter().any(|(k, v)| k == "if-none-match" && v == "\"f1\"") {
            return Reply::status(304, "");
        }
        let body = ics_event("ft", "Feiertag", "20261103T000000Z", "20261104T000000Z", "");
        Reply {
            status: 200,
            content_type: "text/calendar".into(),
            body: body.into_bytes(),
            location: None,
            headers: vec![("ETag".into(), "\"f1\"".into())],
        }
    }))
    .await;
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let source =
        add_source(&store.lock().unwrap(), SourceKind::Ics, "Feiertage", "http://127.0.0.1/…", "", None).unwrap();
    let access = Access::Url(format!("http://127.0.0.1:{port}/feiertage.ics?key=geheim"));
    assert!(sync_source(&store, source, &access).await.unwrap().changed);
    assert!(!sync_source(&store, source, &access).await.unwrap().changed, "304: nothing new");
    let list = events(&store.lock().unwrap(), week().0, week().1, &me(), false).unwrap();
    assert_eq!((list[0].title.as_str(), list[0].editable), ("Feiertag", Editability::Ics));
    let s = store.lock().unwrap();
    assert!(update(&s, &list[0].key, &Fields { title: Some("x".into()), ..Fields::default() }, false, &me()).is_err());
    let cal = list[0].calendar_id;
    assert!(create(&s, cal, &fields("x", utc(2026, 11, 3, 9, 0), 30), &me()).is_err());
    assert_ne!(default_calendar(&s, Some(cal)).unwrap(), cal, "never the default");
}
