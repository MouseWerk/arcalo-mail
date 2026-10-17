//! The real IMAP and SMTP clients of the core against the test server over TCP: the protocol
//! as it goes over the wire (literals, response codes, IDLE, STARTTLS, certificates), the sync
//! engine on top of it, and the account worker end to end.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mail_core::account::{Account, AccountDraft, Auth, Provider, Security, Server, Transport};
use mail_core::engine::{Command, NetConnector, Notice, Status, Worker, check_account};
use mail_core::error::Error;
use mail_core::imap::client::ImapSession;
use mail_core::imap::{Capabilities, IdleOutcome, ImapClient, Role, SEEN, Search, UidRange};
use mail_core::network::Problem;
use mail_core::outbox::{self, Op};
use mail_core::store::Store;
use mail_core::sync::{Strategy, fetch_attachment, fetch_body, sync_account, sync_mailbox};
use mail_testserver::{Options, Running, start};
use tokio::sync::Notify;

const USER: &str = "maria@example.org";
const PW: &str = "geheim ü";

fn msg(id: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Jonas Berger <jonas@example.org>\r\nTo: Maria <maria@example.org>\r\nSubject: {subject}\r\n\
Date: Fri, 16 Oct 2026 09:30:00 +0200\r\nMessage-ID: <{id}>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

async fn server(caps: Capabilities, tls: bool) -> Running {
    start(Options { user: USER.into(), password: PW.into(), caps, tls, ..Options::default() }).await.unwrap()
}

fn plain(port: u16) -> Server {
    Server::new("127.0.0.1", port, Security::Plain)
}

fn all() -> Capabilities {
    mail_core::imap::fake::FakeImap::default().caps
}

fn store_with_account(imap: Server, smtp: Server) -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    let id = s
        .add_account(&AccountDraft {
            provider: Provider::Imap,
            name: String::new(),
            email: USER.into(),
            sender_name: "Maria".into(),
            transport: Transport::Imap { imap, smtp, username: USER.into(), auth: Auth::Password },
        })
        .unwrap();
    (s, id)
}

#[tokio::test]
async fn signs_in_lists_and_syncs_over_the_wire() {
    let srv = server(all(), false).await;
    srv.shared.with(|f| {
        f.deliver("INBOX", &msg("a@x", "Angebot für Oktober", "Hallo"), 1_792_135_800);
        f.deliver("INBOX", &msg("b@x", "Rechnung", "Bitte zahlen"), 1_792_135_900);
        f.mailboxes.insert("Entwürfe & Notizen".into(), mail_core::imap::fake::FakeMailbox::new(&[]));
    });
    let (store, account) = store_with_account(plain(srv.imap), plain(srv.smtp));
    let mut c = ImapSession::connect(&plain(srv.imap), USER, PW).await.unwrap();
    let caps = c.capabilities().await.unwrap();
    assert!(caps.qresync && caps.idle && caps.moves && caps.uidplus && caps.special_use);
    let names: Vec<String> = c.list().await.unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"Entwürfe & Notizen".to_string()), "modified UTF-7 both ways: {names:?}");

    let reports = sync_account(&mut c, &store, account).await.unwrap();
    let inbox = store.mailbox_by_role(account, Role::Inbox).unwrap().unwrap();
    assert_eq!(reports.iter().find(|r| r.mailbox_id == inbox.id).unwrap().added, 2);
    let list = store.messages(inbox.id, 10, 0).unwrap();
    assert_eq!(list[0].subject, "Rechnung");
    assert_eq!(list[1].subject, "Angebot für Oktober");
    assert_eq!(store.mailbox_by_role(account, Role::Sent).unwrap().unwrap().name, "Sent", "SPECIAL-USE");

    // Another client changes things: QRESYNC brings them.
    srv.shared.with(|f| {
        f.set_flags("INBOX", 1, mail_core::imap::Flags { seen: true, ..Default::default() });
        f.expunge("INBOX", 2);
        f.deliver("INBOX", &msg("c@x", "Neu", "x"), 1_792_136_000);
    });
    let r = sync_mailbox(&mut c, &store, caps, inbox.id).await.unwrap();
    assert_eq!((r.strategy, r.added, r.updated, r.removed), (Strategy::Qresync, 1, 1, 1));
    assert_eq!(store.uids(inbox.id).unwrap(), [1, 3]);

    // The body on demand, with the internal date and size from the wire.
    let id = store.messages(inbox.id, 10, 0).unwrap().into_iter().find(|m| m.uid == 1).unwrap().id;
    fetch_body(&mut c, &store, id).await.unwrap();
    assert_eq!(store.message(id).unwrap().text.unwrap().trim(), "Hallo");
    c.logout().await.unwrap();
}

#[tokio::test]
async fn plain_and_condstore_servers() {
    for caps in [Capabilities::default(), Capabilities { qresync: false, ..all() }] {
        let srv = server(caps, false).await;
        srv.shared.with(|f| {
            for i in 1..=3 {
                f.deliver("INBOX", &msg(&format!("m{i}@x"), &format!("Nachricht {i}"), "x"), 1_792_135_800 + i);
            }
        });
        let (store, account) = store_with_account(plain(srv.imap), plain(srv.smtp));
        let mut c = ImapSession::connect(&plain(srv.imap), USER, PW).await.unwrap();
        let caps = c.capabilities().await.unwrap();
        sync_account(&mut c, &store, account).await.unwrap();
        let inbox = store.mailbox_by_role(account, Role::Inbox).unwrap().unwrap().id;
        srv.shared.with(|f| {
            f.expunge("INBOX", 2);
            f.set_flags("INBOX", 3, mail_core::imap::Flags { flagged: true, ..Default::default() });
        });
        let r = sync_mailbox(&mut c, &store, caps, inbox).await.unwrap();
        let want = if caps.condstore { Strategy::Condstore } else { Strategy::Plain };
        assert_eq!((r.strategy, r.removed, r.updated >= 1), (want, 1, true), "{caps:?}");
        assert_eq!(store.uids(inbox).unwrap(), [1, 3]);
        assert!(store.messages(inbox, 10, 0).unwrap().iter().any(|m| m.flagged));
    }
}

#[tokio::test]
async fn moves_copies_appends_and_folders() {
    for caps in [all(), Capabilities { moves: false, uidplus: false, ..all() }] {
        let srv = server(caps, false).await;
        srv.shared.with(|f| {
            f.deliver("INBOX", &msg("a@x", "Eins", "x"), 1);
            f.deliver("INBOX", &msg("b@x", "Zwei", "x"), 2);
        });
        let mut c = ImapSession::connect(&plain(srv.imap), USER, PW).await.unwrap();
        c.select("INBOX", None).await.unwrap();
        let map = c.move_to(&[1], "Archive").await.unwrap();
        assert_eq!(map.is_empty(), !caps.uidplus, "COPYUID with UIDPLUS: {map:?}");
        assert_eq!(srv.shared.with(|f| f.subjects("Archive")), ["Eins"]);
        assert_eq!(srv.shared.with(|f| f.subjects("INBOX")), ["Zwei"], "{caps:?}");
        c.store(&[2], &[SEEN], true).await.unwrap();
        assert!(srv.shared.with(|f| f.mailboxes["INBOX"].messages[&2].flags.seen));
        let uid = c.append("Drafts", &[mail_core::imap::DRAFT], &msg("d@x", "Entwurf mit Ümlaut", "x")).await.unwrap();
        assert_eq!(uid.is_some(), caps.uidplus);
        c.select("Drafts", None).await.unwrap();
        assert_eq!(c.search(&Search::MessageId("d@x".into())).await.unwrap(), [1]);
        assert_eq!(c.search(&Search::Text("ümlaut".into())).await.unwrap(), [1], "a literal in SEARCH");
        c.expunge(&[1]).await.unwrap();
        assert!(srv.shared.with(|f| f.subjects("Drafts")).is_empty());
        c.create("Projekte/Größe").await.unwrap();
        c.rename("Projekte/Größe", "Kunden").await.unwrap();
        assert!(srv.shared.with(|f| f.mailboxes.contains_key("Kunden")));
        c.delete("Kunden").await.unwrap();
        let err = c.delete("Kunden").await.unwrap_err();
        assert!(matches!(err, Error::Imap(_)), "{err:?}");
        c.select("INBOX", None).await.unwrap();
        c.copy(&[2], "Trash").await.unwrap();
        assert_eq!(
            srv.shared.with(|f| (f.subjects("Trash"), f.subjects("INBOX"))),
            (vec!["Zwei".to_string()], vec!["Zwei".to_string()])
        );
        srv.shared.with(|f| f.fail.push("MOVE".into()));
        if caps.moves {
            assert!(matches!(c.move_to(&[2], "Archive").await, Err(Error::Imap(_))), "a refused MOVE is an IMAP error");
        }
    }
}

#[tokio::test]
async fn idle_wakes_on_new_mail_and_on_the_app() {
    let srv = server(all(), false).await;
    let mut c = ImapSession::connect(&plain(srv.imap), USER, PW).await.unwrap();
    c.select("INBOX", None).await.unwrap();
    let wake = Arc::new(Notify::new());
    assert_eq!(c.idle(Duration::from_millis(200), &wake).await.unwrap(), IdleOutcome::Timeout);
    let shared = srv.shared.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        shared.with(|f| f.deliver("INBOX", &msg("n@x", "Neu", "x"), 3));
    });
    assert_eq!(c.idle(Duration::from_secs(10), &wake).await.unwrap(), IdleOutcome::Changed);
    let w = wake.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        w.notify_one();
    });
    assert_eq!(c.idle(Duration::from_secs(10), &wake).await.unwrap(), IdleOutcome::Interrupted);
    // The connection is usable after IDLE.
    let h = c.fetch_headers(UidRange { from: 1, to: None }).await.unwrap();
    assert_eq!(h.len(), 1);
    assert!(h[0].internal_date > 0 && h[0].size > 0);
}

#[tokio::test]
async fn large_messages_and_their_parts_over_the_wire() {
    let srv = server(all(), false).await;
    let big = "QUJD".repeat(800_000);
    let raw = format!(
        "From: lea@example.org\r\nTo: maria@example.org\r\nSubject: Pläne\r\nMessage-ID: <big@x>\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHier die Pläne.\r\n\
--B\r\nContent-Type: application/zip; name=\"plaene.zip\"\r\nContent-Disposition: attachment; filename=\"plaene.zip\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{big}\r\n--B--\r\n"
    );
    srv.shared.with(|f| f.deliver("INBOX", raw.as_bytes(), 1));
    let (store, account) = store_with_account(plain(srv.imap), plain(srv.smtp));
    let mut c = ImapSession::connect(&plain(srv.imap), USER, PW).await.unwrap();
    sync_account(&mut c, &store, account).await.unwrap();
    let inbox = store.mailbox_by_role(account, Role::Inbox).unwrap().unwrap().id;
    let id = store.messages(inbox, 1, 0).unwrap()[0].id;
    fetch_body(&mut c, &store, id).await.unwrap();
    let d = store.message(id).unwrap();
    assert_eq!(d.text.as_deref().map(str::trim), Some("Hier die Pläne."));
    assert!(store.raw_message(id).unwrap().is_none(), "not kept whole");
    let a = d.attachments.iter().find(|a| a.filename.as_deref() == Some("plaene.zip")).unwrap();
    assert_eq!(a.section.as_deref(), Some("2"));
    let data = fetch_attachment(&mut c, &store, a.id).await.unwrap();
    assert_eq!(data.len(), 2_400_000);
}

#[tokio::test]
async fn refused_logins_and_closed_ports_are_told_apart() {
    let srv = server(all(), false).await;
    let err = ImapSession::connect(&plain(srv.imap), USER, "falsch").await.unwrap_err();
    assert!(matches!(err, Error::Connection(Problem::Auth { .. })), "{err:?}");
    let r = check_account(
        &Transport::Imap { imap: plain(srv.imap), smtp: plain(srv.smtp), username: USER.into(), auth: Auth::Password },
        "falsch",
    )
    .await;
    assert!(matches!(r.imap, Some(Problem::Auth { .. })) && matches!(r.smtp, Some(Problem::Auth { .. })), "{r:?}");
    let ok = check_account(
        &Transport::Imap { imap: plain(srv.imap), smtp: plain(srv.smtp), username: USER.into(), auth: Auth::Password },
        PW,
    )
    .await;
    assert!(ok.ok(), "{ok:?}");
    assert_eq!(ok.folders, 6);
    srv.shared.set_offline(true);
    let err = ImapSession::connect(&plain(srv.imap), USER, PW).await.unwrap_err();
    assert!(matches!(err, Error::Connection(p) if p.is_transient()), "dropped at once");
}

#[tokio::test]
async fn tls_starttls_and_trusting_a_certificate() {
    let srv = server(all(), true).await;
    let fp = srv.fingerprint.clone().unwrap();
    // Self-signed: refused with the fingerprint to show the user.
    let tls = Server::new("localhost", srv.imaps.unwrap(), Security::Tls);
    let err = ImapSession::connect(&tls, USER, PW).await.unwrap_err();
    let Error::Connection(Problem::Certificate { fingerprint, .. }) = err else { panic!("{err:?}") };
    assert_eq!(fingerprint, fp);
    // Trusted by its fingerprint: implicit TLS and STARTTLS, IMAP and SMTP.
    let trusted = |port: u16, security: Security| Server {
        trusted_cert: Some(fp.clone()),
        ..Server::new("localhost", port, security)
    };
    let mut c = ImapSession::connect(&trusted(srv.imaps.unwrap(), Security::Tls), USER, PW).await.unwrap();
    assert!(!c.list().await.unwrap().is_empty());
    let mut c = ImapSession::connect(&trusted(srv.imap, Security::StartTls), USER, PW).await.unwrap();
    assert!(!c.list().await.unwrap().is_empty());
    mail_core::smtp::send(
        &trusted(srv.smtps.unwrap(), Security::Tls),
        USER,
        PW,
        USER,
        &["jonas@example.org".into()],
        &msg("t@x", "TLS", "x"),
    )
    .await
    .unwrap();
    mail_core::smtp::send(
        &trusted(srv.smtp, Security::StartTls),
        USER,
        PW,
        USER,
        &["jonas@example.org".into()],
        &msg("s@x", "STARTTLS", "x"),
    )
    .await
    .unwrap();
    let sent: Vec<String> = srv.shared.sent.lock().unwrap().iter().map(|m| m.subject.clone()).collect();
    assert_eq!(sent, ["TLS", "STARTTLS"]);
    // Another certificate than the trusted one is refused again.
    let wrong =
        Server { trusted_cert: Some("00:11".into()), ..Server::new("localhost", srv.imaps.unwrap(), Security::Tls) };
    assert!(matches!(
        ImapSession::connect(&wrong, USER, PW).await,
        Err(Error::Connection(Problem::Certificate { .. }))
    ));
}

#[tokio::test]
async fn smtp_sends_to_everyone_and_delivers_to_the_account() {
    let srv = server(all(), false).await;
    let raw = msg("x@x", "Hallo Team", "Grüße");
    mail_core::smtp::send(&plain(srv.smtp), USER, PW, USER, &[USER.into(), "jonas@example.org".into()], &raw)
        .await
        .unwrap();
    let sent = srv.shared.sent.lock().unwrap().clone();
    assert_eq!(sent[0].to, [USER, "jonas@example.org"]);
    assert_eq!(srv.shared.with(|f| f.subjects("INBOX")), ["Hallo Team"]);
    let err = mail_core::smtp::send(&plain(srv.smtp), USER, "falsch", USER, &[USER.into()], &raw).await.unwrap_err();
    assert!(matches!(err, Error::Connection(Problem::Auth { .. })), "{err:?}");
}

/// GreenMail offers neither 8BITMIME nor takes 8-bit data: the message goes out 7-bit, its
/// text and names intact; an address with umlauts is refused with a clear error.
#[tokio::test]
async fn smtp_without_8bitmime_gets_a_7bit_message() {
    let srv = server(all(), false).await;
    *srv.shared.smtp_8bitmime.lock().unwrap() = false;
    let raw = msg("8@x", "Grüße aus Köln", "Schöne Grüße");
    mail_core::smtp::send(&plain(srv.smtp), USER, PW, USER, &[USER.into()], &raw).await.unwrap();
    let sent = srv.shared.sent.lock().unwrap().clone();
    assert!(sent[0].raw.is_ascii(), "{}", String::from_utf8_lossy(&sent[0].raw));
    let parsed = mail_parser::MessageParser::default().parse(&sent[0].raw).unwrap();
    assert_eq!(parsed.subject(), Some("Grüße aus Köln"));
    assert_eq!(parsed.from().unwrap().first().unwrap().name(), Some("Jonas Berger"));
    assert_eq!(parsed.body_text(0).unwrap().trim(), "Schöne Grüße");
    let err =
        mail_core::smtp::send(&plain(srv.smtp), USER, PW, USER, &["jörg@example.org".into()], &raw).await.unwrap_err();
    assert!(err.to_string().contains("SMTPUTF8"), "{err}");
}

/// The account worker with the real clients: sync, a sent message filed in Sent, and changes
/// made while the server was gone, replayed in order when it is back.
#[tokio::test(flavor = "multi_thread")]
async fn the_worker_end_to_end() {
    let srv = server(all(), false).await;
    srv.shared.with(|f| f.deliver("INBOX", &msg("w@x", "Willkommen", "Hallo Maria"), 1_792_135_800));
    let dir = std::env::temp_dir().join(format!("mail-testserver-worker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("mail.db");
    let _ = std::fs::remove_file(&db);
    let store = Store::open(&db).unwrap();
    let transport =
        Transport::Imap { imap: plain(srv.imap), smtp: plain(srv.smtp), username: USER.into(), auth: Auth::Password };
    let account_id = store
        .add_account(&AccountDraft {
            provider: Provider::Imap,
            name: String::new(),
            email: USER.into(),
            sender_name: "Maria".into(),
            transport: transport.clone(),
        })
        .unwrap();
    let account: Account = store.account(account_id).unwrap();
    let notices: Arc<Mutex<Vec<Notice>>> = Arc::default();
    let n = notices.clone();
    let worker = Worker::spawn(
        account_id,
        db.clone(),
        NetConnector::new(&account, PW.into()).unwrap(),
        Arc::new(move |x| n.lock().unwrap().push(x)),
    )
    .unwrap();
    let status = || {
        notices.lock().unwrap().iter().rev().find_map(|x| match x {
            Notice::Status { status, .. } => Some(status.clone()),
            _ => None,
        })
    };
    let until = |what: &str, f: &dyn Fn() -> bool| {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(15), "timed out: {what}: {:?}", notices.lock().unwrap());
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    until("ready", &|| status() == Some(Status::Ready));
    let inbox = store.mailbox_by_role(account_id, Role::Inbox).unwrap().unwrap().id;
    assert_eq!(store.messages(inbox, 10, 0).unwrap()[0].subject, "Willkommen");

    // New mail while the worker waits in IDLE.
    srv.shared.with(|f| f.deliver("INBOX", &msg("n@x", "Frisch", "x"), 1_792_136_000));
    until("new mail", &|| notices.lock().unwrap().iter().any(|x| matches!(x, Notice::NewMail { .. })));

    // Sending: SMTP, then the copy in Sent.
    let raw = msg("out@example.org", "Antwort", "Danke!");
    let b64 = |b: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b);
    outbox::enqueue(
        &store,
        account_id,
        &Op::Send {
            raw_b64: b64(&raw),
            from: USER.into(),
            recipients: vec!["jonas@example.org".into()],
            sent_b64: None,
            sent_mailbox: Some("Sent".into()),
            draft: None,
            answered: Some(("INBOX".into(), 1)),
            copies: vec![],
        },
    )
    .unwrap();
    worker.send(Command::Kick);
    until("sent and filed", &|| srv.shared.with(|f| f.subjects("Sent")) == ["Antwort"]);
    assert!(srv.shared.with(|f| f.mailboxes["INBOX"].messages[&1].flags.answered));
    assert_eq!(srv.shared.sent.lock().unwrap().len(), 1);

    // Offline: the change waits, the worker retries, the change arrives in order.
    srv.shared.set_offline(true);
    until("connection lost", &|| matches!(status(), Some(Status::Retrying { .. })));
    let id = store.messages(inbox, 10, 0).unwrap().into_iter().find(|m| m.subject == "Willkommen").unwrap().id;
    store.mark_seen(id, true).unwrap();
    outbox::enqueue(
        &store,
        account_id,
        &Op::Flags { mailbox: "INBOX".into(), uids: vec![1], seen: Some(true), flagged: None },
    )
    .unwrap();
    store.set_hidden(&[id], true).unwrap();
    outbox::enqueue(
        &store,
        account_id,
        &Op::Move { from: "INBOX".into(), to: "Archive".into(), uids: vec![1], ids: vec![id] },
    )
    .unwrap();
    worker.send(Command::Kick);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(outbox::pending(&store, account_id).unwrap(), 2);
    srv.shared.set_offline(false);
    worker.send(Command::SyncNow);
    until("replayed", &|| outbox::pending(&store, account_id).unwrap() == 0);
    assert_eq!(srv.shared.with(|f| f.subjects("Archive")), ["Willkommen"]);
    assert!(srv.shared.with(|f| f.mailboxes["Archive"].messages.values().next().unwrap().flags.seen));
    drop(worker);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Microsoft's way in: OAuth tokens from the test server's identity provider, XOAUTH2 on IMAP
/// and SMTP, a refresh when the server refuses the access token, and Exchange Online's refusal
/// when SMTP AUTH is off for the mailbox.
#[tokio::test(flavor = "multi_thread")]
async fn xoauth2_with_tokens_from_the_identity_provider() {
    use mail_core::account::{Credential, OAuthProvider};
    use mail_core::engine::Connector;
    use mail_core::oauth::{OAuthConfig, TokenSource, sign_in};
    let srv = server(all(), false).await;
    let cfg = OAuthConfig {
        provider: OAuthProvider::Microsoft,
        client_id: "test".into(),
        authorize_url: format!("http://127.0.0.1:{}/oauth/authorize", srv.control),
        token_url: format!("http://127.0.0.1:{}/oauth/token", srv.control),
        scopes: mail_core::oauth::MS_SCOPES.iter().map(|s| s.to_string()).collect(),
        extra_consent: vec![],
    };
    // The browser: follows the authorize address to the app's loopback redirect.
    let tokens = sign_in(&cfg, USER, |url| {
        let url = url.to_string();
        tokio::spawn(async move {
            let client = mail_core::network::client_for(mail_core::network::Service::OAuthMicrosoft).unwrap();
            let _ = client.get(&url).send().await.unwrap().text().await;
        });
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(tokens.email.as_deref(), Some(USER));

    let cred = Credential::XOAuth2(tokens.access_token.clone());
    let mut c = ImapSession::connect_with(&plain(srv.imap), USER, &cred).await.unwrap();
    assert!(c.list().await.unwrap().len() >= 3);
    let err =
        ImapSession::connect_with(&plain(srv.imap), USER, &Credential::XOAuth2("falsch".into())).await.unwrap_err();
    assert!(matches!(err, Error::Connection(Problem::Auth { ref detail }) if detail.contains("401")), "{err:?}");
    mail_core::smtp::send_with(&plain(srv.smtp), USER, &cred, USER, &[USER.into()], &msg("o@x", "Per OAuth", "x"))
        .await
        .unwrap();
    assert_eq!(srv.shared.sent.lock().unwrap().len(), 1);

    // The connector refreshes when the server stops taking the access token.
    let account = Account {
        id: 1,
        provider: Provider::Microsoft,
        name: String::new(),
        email: USER.into(),
        sender_name: String::new(),
        transport: Transport::Imap {
            imap: plain(srv.imap),
            smtp: plain(srv.smtp),
            username: USER.into(),
            auth: Auth::OAuth2(OAuthProvider::Microsoft),
        },
        signature: String::new(),
    };
    let rotated = Arc::new(Mutex::new(0));
    let r2 = rotated.clone();
    let source = TokenSource::new(
        cfg.clone(),
        tokens.refresh_token.clone().unwrap(),
        Some(Arc::new(move |_: &str| *r2.lock().unwrap() += 1)),
    );
    let conn = NetConnector::with_oauth(&account, source).unwrap();
    assert!(conn.files_sent(), "Microsoft files sent mail itself");
    conn.imap().await.unwrap();
    *srv.shared.smtp_auth_disabled.lock().unwrap() = true;
    let check = mail_core::engine::check_account_with(&account.transport, &cred).await;
    assert!(check.imap.is_none() && matches!(check.smtp, Some(Problem::SmtpAuthDisabled { .. })), "{check:?}");
    *srv.shared.smtp_auth_disabled.lock().unwrap() = false;
    srv.shared.oauth.lock().unwrap().expire_access();
    conn.imap().await.unwrap();
    assert_eq!(*rotated.lock().unwrap(), 2, "a refresh for the first connection and one after the refusal");

    // SMTP AUTH switched off for the mailbox: a problem the UI explains, not a wrong password.
    *srv.shared.smtp_auth_disabled.lock().unwrap() = true;
    let err = conn.send(USER, &[USER.into()], &msg("p@x", "Nochmal", "x")).await.unwrap_err();
    assert!(matches!(err, Error::Connection(Problem::SmtpAuthDisabled { .. })), "{err:?}");

    // A revoked grant: sign in again.
    srv.shared.oauth.lock().unwrap().revoke_all();
    let err = conn.imap().await.err().unwrap();
    assert!(matches!(&err, Error::Connection(p) if p.needs_sign_in()), "{err:?}");
}

/// A JMAP account against the test server: push over EventSource ends the wait as soon as
/// another client changes the mailbox.
#[tokio::test(flavor = "multi_thread")]
async fn jmap_push_over_the_wire() {
    use mail_core::jmap::{JmapAuth, JmapClient, JmapSession};
    let srv = server(all(), false).await;
    srv.shared.with(|f| f.deliver("INBOX", &msg("j@x", "Über JMAP", "Hallo"), 1_792_135_800));
    let dir = std::env::temp_dir().join(format!("mail-testserver-jmap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("mail.db");
    let store = Store::open(&db).unwrap();
    let account = store
        .add_account(&AccountDraft {
            provider: Provider::Jmap,
            name: String::new(),
            email: USER.into(),
            sender_name: String::new(),
            transport: Transport::Jmap {
                session_url: format!("http://127.0.0.1:{}/jmap/session", srv.control),
                username: USER.into(),
                auth: Auth::Password,
            },
        })
        .unwrap();
    let auth = JmapAuth::Basic { user: USER.into(), password: PW.into() };
    let url = format!("http://127.0.0.1:{}/jmap/session", srv.control);
    let client = JmapClient::connect(&url, &auth).await.unwrap();
    assert!(client.has_push());
    let mut j = JmapSession::new(client, Store::open(&db).unwrap(), account, Duration::from_secs(60));
    let reports = sync_account(&mut j, &store, account).await.unwrap();
    assert!(reports.iter().any(|r| r.added == 1));
    let inbox = store.mailbox_by_role(account, Role::Inbox).unwrap().unwrap();
    assert_eq!(store.messages(inbox.id, 10, 0).unwrap()[0].subject, "Über JMAP");
    j.select("INBOX", None).await.unwrap();
    let shared = srv.shared.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        shared.with(|f| f.deliver("INBOX", &msg("k@x", "Gleich da", "x"), 1_792_135_900));
    });
    let start = Instant::now();
    let outcome = j.idle(Duration::from_secs(20), &Notify::new()).await.unwrap();
    assert_eq!(outcome, IdleOutcome::Changed);
    assert!(start.elapsed() < Duration::from_secs(5), "pushed, not polled");
    let caps = j.capabilities().await.unwrap();
    let r = sync_mailbox(&mut j, &store, caps, inbox.id).await.unwrap();
    assert_eq!((r.strategy, r.added), (Strategy::Qresync, 1));
    // The discovery finds the server at its well-known address.
    let found = mail_core::jmap::discover(USER, &[format!("http://127.0.0.1:{}/.well-known/jmap", srv.control)]).await;
    assert!(found.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn pop3_downloads_the_inbox_and_removes_at_quit() {
    use mail_core::pop3::{Policy, visit};
    let srv = server(all(), false).await;
    srv.shared.with(|f| {
        f.deliver("INBOX", &msg("p1@x", "Eins", "Hallo"), 1_760_000_000);
        f.deliver("INBOX", &msg("p2@x", "Zwei", ".Punkt"), 1_760_000_001);
    });
    let (store, account) = store_with_account(plain(srv.imap), plain(srv.smtp));
    let store = Mutex::new(store);
    let keep = Policy { leave_on_server: true, delete_after_days: None };
    let v = visit(&store, account, &plain(srv.pop3), USER, PW, keep).await.unwrap();
    assert_eq!((v.downloaded, v.removed), (2, 0));
    assert_eq!(srv.shared.with(|f| f.subjects("INBOX")).len(), 2, "left on the server");
    let take = Policy { leave_on_server: false, delete_after_days: None };
    let v = visit(&store, account, &plain(srv.pop3), USER, PW, take).await.unwrap();
    assert_eq!((v.downloaded, v.removed), (0, 2));
    assert!(srv.shared.with(|f| f.subjects("INBOX")).is_empty(), "gone at QUIT");
    let wrong = visit(&store, account, &plain(srv.pop3), USER, "falsch", keep).await.unwrap_err();
    assert!(matches!(wrong, Error::Connection(Problem::Auth { .. })), "{wrong:?}");
}

#[tokio::test]
async fn sieve_rules_and_the_ai_provider_over_the_wire() {
    use mail_core::rules::{Action, Condition, Field, Op as RuleOp, Place, Rule, RuleBody};
    let srv = server(all(), false).await;
    let rule = Rule {
        id: 1,
        account_id: Some(1),
        name: "Shop".into(),
        place: Place::Server,
        enabled: true,
        position: 1,
        body: RuleBody {
            match_all: true,
            conditions: vec![Condition { field: Field::From, op: RuleOp::Contains, value: "shop".into() }],
            actions: vec![Action::Move { folder: "Shop".into() }],
            stop: false,
        },
        server_ref: None,
    };
    let (requires, block) = mail_core::sieve::script(&[rule], "Trash", "Archive");
    mail_core::sieve::upload(&plain(srv.sieve), USER, PW, &requires, &block).await.unwrap();
    {
        let s = srv.shared.sieve.lock().unwrap();
        assert_eq!(s.active.as_deref(), Some(mail_core::sieve::SCRIPT_NAME));
        assert!(s.scripts[mail_core::sieve::SCRIPT_NAME].contains("fileinto \"Shop\""));
    }
    // The AI provider of the control port, streamed.
    let p = mail_core::ai::AiProvider {
        id: "test".into(),
        name: "Test".into(),
        kind: mail_core::ai::ProviderKind::Compatible,
        base_url: format!("http://127.0.0.1:{}/ai/v1", srv.control),
        model: "llama3.2".into(),
        api_version: String::new(),
    };
    let c = mail_core::ai::AiClient::new(p, None).unwrap();
    let msgs = mail_core::ai::prompts::rewrite(
        "Kannst du mir das schicken?",
        mail_core::ai::prompts::Style::Formal,
        mail_core::i18n::Lang::De,
    );
    let mut pieces = 0;
    let a = c.chat(&msgs, 0.2, &std::sync::atomic::AtomicBool::new(false), |_| pieces += 1).await.unwrap();
    assert_eq!(a.text, "Können Sie mir das schicken?");
    assert!(pieces > 1);
    assert_eq!(srv.shared.ai.lock().unwrap().requests.len(), 1);
}

#[tokio::test]
async fn calendars_over_caldav_on_the_control_port() {
    use mail_core::calendar::{
        self, Fields, SourceKind,
        sync::{Access, sync_source},
    };
    let srv = server(all(), false).await;
    let base = format!("http://127.0.0.1:{}/caldav", srv.control);
    srv.shared.caldav.lock().unwrap().put(
        mail_core::calendar::caldav::fake::WORK,
        "a.ics",
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261103T090000Z\r\nDTEND:20261103T100000Z\r\nSUMMARY:Planung\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    let store = Mutex::new(Store::open_in_memory().unwrap());
    let source = calendar::add_source(&store.lock().unwrap(), SourceKind::Caldav, "Test", &base, USER, None).unwrap();
    let access = Access::Basic { user: USER.into(), password: PW.into() };
    sync_source(&store, source, &access).await.unwrap();
    let me = vec![USER.to_string()];
    let from = chrono::DateTime::parse_from_rfc3339("2026-11-02T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
    let to = from + chrono::Duration::days(7);
    let list = calendar::events(&store.lock().unwrap(), from, to, &me, false).unwrap();
    assert_eq!(list.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Planung"]);
    let (ev, _) = calendar::create(
        &store.lock().unwrap(),
        list[0].calendar_id,
        &Fields { title: Some("Review".into()), start: Some(from + chrono::Duration::hours(36)), ..Fields::default() },
        &me,
    )
    .unwrap();
    sync_source(&store, source, &access).await.unwrap();
    assert!(calendar::event(&store.lock().unwrap(), &ev.key, &me).unwrap().write.is_none());
    let writes = srv.shared.caldav.lock().unwrap().writes.clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0, "PUT");
}

#[tokio::test]
async fn jonas_reads_and_writes_encrypted_mail() {
    use mail_core::crypto::{self, Method, Protect, pgp};
    let srv = server(all(), false).await;
    let public = srv.shared.peer.lock().unwrap().public().to_string();
    let jonas = pgp::read_public(public.as_bytes()).unwrap();
    // The app: an own key, Jonas's key, a message to him.
    let (store, id) = store_with_account(plain(srv.imap), plain(srv.smtp));
    let account = store.account(id).unwrap();
    let (secret, own) = pgp::generate("Maria", USER).unwrap();
    let fp = crypto::save_pgp(&store, &own, true, "generated", true).unwrap().fingerprint;
    crypto::save_pgp(&store, &jonas, false, "imported", false).unwrap();
    struct One(String, String);
    impl crypto::Keys for One {
        fn pgp_secret(&self, fp: &str) -> Option<String> {
            (fp == self.0).then(|| self.1.clone())
        }
        fn smime_secret(&self, _: &str) -> Option<(Vec<u8>, String)> {
            None
        }
    }
    let keys = One(fp, secret);
    let d = mail_core::compose::Draft {
        account_id: id,
        to: vec!["jonas@example.org".into()],
        subject: "Geheim".into(),
        text: "Nur für Jonas.".into(),
        ..Default::default()
    };
    let raw = mail_core::compose::build(&account, &d, &[], "g1@example.org", 1_792_000_000, false).unwrap();
    let raw = crypto::add_header(&raw, "Autocrypt", &crypto::autocrypt_for(&store, USER).unwrap().unwrap());
    let sealed = crypto::protect(
        &store,
        USER,
        &raw,
        &["jonas@example.org".into()],
        &Protect { method: Method::Pgp, sign: true, encrypt: true },
        &keys,
    )
    .unwrap();
    let read = srv.shared.peer.lock().unwrap().read(&sealed, USER);
    assert_eq!(read["decrypted"], true);
    assert_eq!(read["signature"], "valid");
    assert!(read["text"].as_str().unwrap().contains("Nur für Jonas."));
    // Jonas answers, encrypted to the key he learned.
    let answer = srv.shared.peer.lock().unwrap().letter(USER, "Re: Geheim", "Angekommen.", 1_792_000_100).unwrap();
    let o = crypto::open(&store, &answer, Some("jonas@example.org"), &keys).unwrap();
    assert!(o.protection.decrypted);
    assert_eq!(o.protection.signature.as_deref(), Some("valid"));
}

/// Without UIDPLUS a plain EXPUNGE removes every message marked `\Deleted`: messages another
/// client only marked (its „mark as deleted“ model, undo still possible there) must stay.
#[tokio::test]
async fn without_uidplus_a_move_leaves_messages_others_marked_deleted() {
    let caps = Capabilities { uidplus: false, moves: false, ..all() };
    let srv = server(caps, false).await;
    srv.shared.with(|f| {
        f.deliver("INBOX", &msg("a@x", "Marked by another client", "x"), 1_792_135_800);
        f.deliver("INBOX", &msg("b@x", "Archived here", "x"), 1_792_135_900);
        f.deliver("INBOX", &msg("c@x", "Untouched", "x"), 1_792_136_000);
        f.set_flags("INBOX", 1, mail_core::imap::Flags { deleted: true, ..Default::default() });
    });
    let mut c = ImapSession::connect(&plain(srv.imap), USER, PW).await.unwrap();
    c.select("INBOX", None).await.unwrap();
    c.move_to(&[2], "Trash").await.unwrap();
    let (left, marked) = srv.shared.with(|f| {
        let m = &f.mailboxes["INBOX"].messages;
        (f.subjects("INBOX"), m.get(&1).is_some_and(|x| x.flags.deleted))
    });
    assert_eq!(left, ["Marked by another client", "Untouched"]);
    assert!(marked, "still marked for the other client");
    assert_eq!(srv.shared.with(|f| f.subjects("Trash")), ["Archived here"]);
    // Deleting for good works the same way.
    c.expunge(&[3]).await.unwrap();
    assert_eq!(srv.shared.with(|f| f.subjects("INBOX")), ["Marked by another client"]);
}
