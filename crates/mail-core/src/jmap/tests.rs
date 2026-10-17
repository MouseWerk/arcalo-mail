//! A JMAP account end to end: the account worker against the fake JMAP server on loopback,
//! syncing into the store, noticing new mail, replaying flags, moves, folders and sending.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::fake::{FakeJmapState, authorized};
use super::*;
use crate::account::{AccountDraft, Provider};
use crate::engine::{Command, Notice, Sink, Status, Worker};
use crate::imap::fake::FakeImap;
use crate::imap::{ImapClient, Role};
use crate::network::tests::testhttp::{self, Reply};
use crate::outbox::{self, Op};
use crate::parse::tests::sample;
use crate::store::{Store, now};

type Server = Arc<Mutex<(FakeImap, FakeJmapState)>>;

const USER: &str = "maria@example.org";
const PW: &str = "geheim";

async fn serve(server: Server) -> u16 {
    let base: Arc<Mutex<String>> = Arc::default();
    let b = base.clone();
    let port = testhttp::serve(Arc::new(move |method, target, headers, body| {
        let path = target.split('?').next().unwrap_or_default();
        if !path.starts_with("/jmap/") && path != "/.well-known/jmap" {
            return Reply::status(404, "{}");
        }
        if path != "/.well-known/jmap" && !authorized(headers, USER, PW) {
            return Reply::status(401, "{}");
        }
        let mut s = server.lock().unwrap();
        let (fake, jmap) = &mut *s;
        let r = jmap.handle(fake, method, target, body, &b.lock().unwrap());
        Reply { status: r.status, content_type: r.content_type, body: r.body, location: r.location, headers: vec![] }
    }))
    .await;
    *base.lock().unwrap() = format!("http://127.0.0.1:{port}");
    port
}

struct Setup {
    db: PathBuf,
    store: Store,
    account: i64,
    server: Server,
    port: u16,
    notices: Arc<Mutex<Vec<Notice>>>,
}

async fn setup(name: &str) -> Setup {
    let dir = std::env::temp_dir().join(format!("mail-jmap-{name}-{}-{}", std::process::id(), now()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("mail.db");
    let mut fake = FakeImap::default().with_all_folders();
    fake.deliver("INBOX", &sample("a@x", "Willkommen", None, "Hallo über JMAP"), 1_760_000_000);
    fake.deliver("INBOX", &sample("b@x", "Termin", None, "Morgen um zehn"), 1_760_000_100);
    let server: Server = Arc::new(Mutex::new((fake, FakeJmapState::new(USER, false))));
    let port = serve(server.clone()).await;
    let store = Store::open(&db).unwrap();
    let account = store
        .add_account(&AccountDraft {
            provider: Provider::Jmap,
            name: String::new(),
            email: USER.into(),
            sender_name: "Maria".into(),
            transport: Transport::Jmap {
                session_url: format!("http://127.0.0.1:{port}/jmap/session"),
                username: USER.into(),
                auth: Auth::Password,
            },
        })
        .unwrap();
    Setup { db, store, account, server, port, notices: Arc::default() }
}

impl Setup {
    fn start(&self) -> Worker {
        let notices = self.notices.clone();
        let sink: Sink = Arc::new(move |n| notices.lock().unwrap().push(n));
        let mut c = JmapConnector::new(&self.store.account(self.account).unwrap(), PW.into(), self.db.clone()).unwrap();
        c.poll = Duration::from_millis(100);
        Worker::spawn(self.account, self.db.clone(), c, sink).unwrap()
    }

    async fn until(&self, what: &str, f: impl Fn(&Setup) -> bool) {
        let start = Instant::now();
        while !f(self) {
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "timed out: {what}; notices: {:?}",
                self.notices.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn status(&self) -> Option<Status> {
        self.notices.lock().unwrap().iter().rev().find_map(|n| match n {
            Notice::Status { status, .. } => Some(status.clone()),
            _ => None,
        })
    }

    fn inbox(&self) -> i64 {
        self.store.mailbox_by_role(self.account, Role::Inbox).unwrap().unwrap().id
    }

    fn with<R>(&self, f: impl FnOnce(&mut FakeImap) -> R) -> R {
        f(&mut self.server.lock().unwrap().0)
    }

    fn subjects(&self, role: Role) -> Vec<String> {
        let mb = self.store.mailbox_by_role(self.account, role).unwrap().unwrap();
        let mut v: Vec<String> = self.store.messages(mb.id, 50, 0).unwrap().into_iter().map(|m| m.subject).collect();
        v.sort();
        v
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn syncs_a_jmap_account_and_follows_changes() {
    let s = setup("sync").await;
    let w = s.start();
    s.until("first sync", |s| s.status() == Some(Status::Ready)).await;
    assert_eq!(s.store.mailboxes(s.account).unwrap().len(), 6);
    assert_eq!(s.subjects(Role::Inbox), ["Termin", "Willkommen"]);
    s.until("bodies ahead", |s| s.store.messages(s.inbox(), 10, 0).unwrap().iter().all(|m| !m.snippet.is_empty()))
        .await;
    let first = s.store.messages(s.inbox(), 10, 0).unwrap();
    let termin = first.iter().find(|m| m.subject == "Termin").unwrap().id;
    assert!(s.store.message(termin).unwrap().has_body);

    // New mail and a flag set by another client, noticed by polling.
    s.with(|f| f.deliver("INBOX", &sample("c@x", "Neu da", None, "x"), 1_760_000_200));
    s.until("new mail", |s| {
        s.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::NewMail { messages, .. } if messages.len() == 1))
    })
    .await;
    s.with(|f| {
        let uid = *f.mailboxes["INBOX"].messages.keys().next().unwrap();
        let mut flags = f.mailboxes["INBOX"].messages[&uid].flags.clone();
        flags.flagged = true;
        f.set_flags("INBOX", uid, flags);
    });
    s.until("flag synced", |s| s.store.messages(s.inbox(), 10, 0).unwrap().iter().any(|m| m.flagged)).await;

    // Local changes reach the server: seen, a move to Archive, a new folder.
    let id = s.store.messages(s.inbox(), 10, 0).unwrap().iter().find(|m| m.subject == "Willkommen").unwrap().id;
    let uid = s.store.message(id).unwrap().summary.uid;
    outbox::enqueue(
        &s.store,
        s.account,
        &Op::Flags { mailbox: "INBOX".into(), uids: vec![uid], seen: Some(true), flagged: None },
    )
    .unwrap();
    s.store.set_hidden(&[id], true).unwrap();
    outbox::enqueue(
        &s.store,
        s.account,
        &Op::Move { from: "INBOX".into(), to: "Archive".into(), uids: vec![uid], ids: vec![id] },
    )
    .unwrap();
    outbox::enqueue(&s.store, s.account, &Op::CreateFolder { name: "Projekte".into() }).unwrap();
    outbox::enqueue(&s.store, s.account, &Op::RenameFolder { from: "Projekte".into(), to: "Kunden".into() }).unwrap();
    w.send(Command::Kick);
    s.until("replayed", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
    assert_eq!(s.with(|f| f.subjects("Archive")), ["Willkommen"]);
    assert!(s.with(|f| f.mailboxes["Archive"].messages.values().next().unwrap().flags.seen), "seen before the move");
    assert!(s.with(|f| f.mailboxes.contains_key("Kunden") && !f.mailboxes.contains_key("Projekte")));
    w.send(Command::SyncNow);
    s.until("archive synced", |s| s.subjects(Role::Archive) == ["Willkommen"]).await;
    s.until("folder synced", |s| s.store.mailbox_by_name(s.account, "Kunden").unwrap().is_some()).await;
    assert_eq!(s.subjects(Role::Inbox), ["Neu da", "Termin"]);

    // Sending: import, submission, filed in Sent by the server.
    let raw = sample("out@example.org", "Antwort über JMAP", None, "Danke!");
    let b = base64::engine::general_purpose::STANDARD.encode(&raw);
    outbox::enqueue(
        &s.store,
        s.account,
        &Op::Send {
            raw_b64: b,
            from: USER.into(),
            recipients: vec!["jonas@example.org".into(), "bcc@example.org".into()],
            sent_b64: None,
            sent_mailbox: Some("Sent".into()),
            draft: None,
            answered: None,
            copies: vec![],
        },
    )
    .unwrap();
    w.send(Command::Kick);
    s.until("sent", |s| outbox::pending(&s.store, s.account).unwrap() == 0).await;
    {
        let srv = s.server.lock().unwrap();
        assert_eq!(srv.1.submitted.len(), 1);
        assert_eq!(srv.1.submitted[0].to, ["jonas@example.org", "bcc@example.org"]);
        assert_eq!(srv.0.subjects("Sent"), ["Antwort über JMAP"]);
        assert!(srv.0.subjects("Drafts").is_empty(), "moved out of Drafts once sent");
    }
    s.until("sent synced once", |s| s.subjects(Role::Sent) == ["Antwort über JMAP"]).await;
    drop(w);
}

/// A server that forgot the old state (`cannotCalculateChanges`): everything is compared
/// again, UIDs of known messages stay, removed ones vanish.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_state_resyncs_without_new_uids() {
    let s = setup("resync").await;
    let client = JmapClient::connect(
        &format!("http://127.0.0.1:{}/jmap/session", s.port),
        &JmapAuth::Basic { user: USER.into(), password: PW.into() },
    )
    .await
    .unwrap();
    let mut j = JmapSession::new(client, Store::open(&s.db).unwrap(), s.account, Duration::from_millis(50));
    let first = j.select("INBOX", None).await.unwrap();
    assert_eq!((first.info.exists, first.info.uid_next), (2, 3));
    let modseq = first.info.highest_modseq.unwrap();
    // A flag set elsewhere comes as a change with QRESYNC.
    s.with(|f| {
        let mut flags = f.mailboxes["INBOX"].messages[&2].flags.clone();
        flags.flagged = true;
        f.set_flags("INBOX", 2, flags);
    });
    let q = crate::imap::QresyncParams { uid_validity: first.info.uid_validity, modseq };
    let flagged = j.select("INBOX", Some(q)).await.unwrap();
    assert_eq!(flagged.changed.len(), 1, "{flagged:?}");
    assert!(flagged.changed[0].flags.flagged && flagged.changed[0].uid == 2);
    let modseq = flagged.info.highest_modseq.unwrap();
    // The server forgets its states; one message goes, one comes.
    {
        let mut srv = s.server.lock().unwrap();
        let (fake, jmap) = &mut *srv;
        let uid = *fake.mailboxes["INBOX"].messages.keys().next().unwrap();
        fake.expunge("INBOX", uid);
        fake.deliver("INBOX", &sample("d@x", "Später", None, "x"), 1_760_000_300);
        *jmap = FakeJmapState::new(USER, false);
    }
    let q = crate::imap::QresyncParams { uid_validity: first.info.uid_validity, modseq };
    let again = j.select("INBOX", Some(q)).await.unwrap();
    assert_eq!(again.info.uid_validity, first.info.uid_validity);
    assert_eq!(again.info.exists, 2);
    assert_eq!(again.vanished, [1], "the removed one");
    assert_eq!(again.info.uid_next, 4, "only the new message got a UID");
    let uids = j.uids().await.unwrap();
    assert_eq!(uids, [2, 3]);
    let headers = j.fetch_headers(crate::imap::UidRange { from: 3, to: None }).await.unwrap();
    assert_eq!(crate::parse::parse(&headers[0].header).unwrap().subject, "Später");
    // Search on the server.
    assert_eq!(j.search(&crate::imap::Search::Text("Morgen".into())).await.unwrap(), [2]);
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_passwords_and_discovery() {
    let s = setup("auth").await;
    let url = format!("http://127.0.0.1:{}/jmap/session", s.port);
    let e = JmapClient::connect(&url, &JmapAuth::Basic { user: USER.into(), password: "falsch".into() })
        .await
        .err()
        .unwrap();
    assert!(matches!(&e, Error::Connection(p) if p.needs_sign_in()), "{e:?}");
    assert_eq!(check(&url, &JmapAuth::Bearer(PW.into())).await.unwrap(), 6);
    let found = discover("maria@example.org", &[format!("http://127.0.0.1:{}/.well-known/jmap", s.port)]).await;
    assert!(found.is_some_and(|u| u.ends_with("/.well-known/jmap")));
    assert_eq!(discover("maria@fastmail.com", &[]).await.as_deref(), Some("https://api.fastmail.com/jmap/session"));
    assert!(discover("maria@example.org", &[format!("http://127.0.0.1:{}/nothing", s.port)]).await.is_none());
}

#[test]
fn reads_event_source_states() {
    let mut buf = String::from(
        "event: state\ndata: {\"@type\":\"StateChange\",\"changed\":{\"a1\":{\"Email\":\"5\",\"Mailbox\":\"2\"}}}\n\n: ping\n\nevent: state\ndata: {\"changed\":{\"other\":{\"Email\":\"9\"}}}\n\nevent: sta",
    );
    let got = parse_events(&mut buf, "a1");
    assert_eq!(got, [StateChange { email: Some("5".into()), mailbox: Some("2".into()) }]);
    assert_eq!(buf, "event: sta");
}
