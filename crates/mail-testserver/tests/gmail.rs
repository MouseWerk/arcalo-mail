//! The core against a Gmail account over the wire (`mail_testserver::gmail`): what a real Gmail
//! account showed on a phone, checked where the core decides it. The localized `[Gmail]/…`
//! folders and the labels keep their exact names and one folder per role; a first sync that is
//! cut short already holds the newest mail; a message in INBOX, „Alle Nachrichten“ and a label
//! is one row in the inbox and one hit in search; receipts and newsletters stay out of
//! „Personen“; the newest messages get their first lines; pictures inside HTML are shown.

use mail_core::account::{AccountDraft, Auth, Provider, Security, Server, Transport};
use mail_core::imap::client::ImapSession;
use mail_core::imap::{ImapClient, Role};
use mail_core::settings::{Prefetch, RemoteContent};
use mail_core::store::Store;
use mail_core::sync::{fetch_body, prefetch_bodies, sync_account, sync_folders, sync_mailbox};
use mail_core::triage::{Category, InboxQuery};
use mail_testserver::gmail::{self, Summary};
use mail_testserver::{Options, Running, start};

const USER: &str = "maria.muster@gmail.com";
const PW: &str = "app-passwort";

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

async fn gmail_server(count: usize) -> (Running, Summary) {
    let srv = start(Options { user: USER.into(), password: PW.into(), ..Options::default() }).await.unwrap();
    let summary = srv.shared.with(|f| gmail::fill(f, count, now(), USER));
    (srv, summary)
}

fn plain(port: u16) -> Server {
    Server::new("127.0.0.1", port, Security::Plain)
}

fn store_for(srv: &Running) -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    let id = s
        .add_account(&AccountDraft {
            provider: Provider::Imap,
            name: String::new(),
            email: USER.into(),
            sender_name: "Maria Muster".into(),
            transport: Transport::Imap {
                imap: plain(srv.imap),
                smtp: plain(srv.smtp),
                username: USER.into(),
                auth: Auth::Password,
            },
        })
        .unwrap();
    (s, id)
}

async fn connect(srv: &Running) -> ImapSession {
    ImapSession::connect(&plain(srv.imap), USER, PW).await.unwrap()
}

fn query(account: i64, tab: Category) -> InboxQuery {
    InboxQuery { account: Some(account), tab, focus: false, threads: false, bundles: false }
}

/// The folder list as Gmail sends it: names exactly as they are (brackets, umlauts, nesting),
/// one folder per role, the old drafts label an ordinary folder.
#[tokio::test]
async fn gmail_folders_keep_their_names_and_one_role_each() {
    let (srv, summary) = gmail_server(200).await;
    let (store, account) = store_for(&srv);
    let mut c = connect(&srv).await;
    assert!(c.capabilities().await.unwrap().gmail, "X-GM-EXT-1");
    sync_folders(&mut c, &store, account).await.unwrap();
    let boxes = store.mailboxes(account).unwrap();
    let mut names: Vec<String> = boxes.iter().map(|m| m.name.clone()).collect();
    names.sort();
    let mut expected = summary.folders.clone();
    expected.sort();
    assert_eq!(names, expected, "every folder once, its name exactly as the server sent it");
    for role in [Role::Inbox, Role::Drafts, Role::Sent, Role::Trash, Role::Junk, Role::All] {
        let with: Vec<&str> = boxes.iter().filter(|m| m.role == Some(role)).map(|m| m.name.as_str()).collect();
        assert_eq!(with.len(), 1, "one folder for {role:?}: {with:?}");
    }
    let drafts = store.mailbox_by_role(account, Role::Drafts).unwrap().unwrap();
    assert_eq!(drafts.name, "[Gmail]/Entwürfe");
    let label = store.mailbox_by_name(account, "Entwürfe").unwrap().unwrap();
    assert_eq!(label.role, None, "a label named like the drafts is no second drafts folder");
    let notion = store.mailbox_by_name(account, "[Notion]/Inbox").unwrap().unwrap();
    assert_eq!((notion.delimiter.as_deref(), notion.role), (Some("/"), None));
    assert!(!store.mailbox_by_name(account, "[Gmail]").unwrap().unwrap().selectable);
}

/// A first sync of a large inbox that is cut short after its first batch of headers already
/// shows the newest mail (Gmail with years of mail: the newest came last, 2015 first).
#[tokio::test]
async fn a_first_sync_cut_short_already_holds_the_newest_mail() {
    let (srv, summary) = gmail_server(3000).await;
    let (store, account) = store_for(&srv);
    let mut c = connect(&srv).await;
    let caps = c.capabilities().await.unwrap();
    sync_folders(&mut c, &store, account).await.unwrap();
    let inbox = store.mailbox_by_role(account, Role::Inbox).unwrap().unwrap();
    *srv.shared.cut_after_header_batches.lock().unwrap() = Some(1);
    let r = sync_mailbox(&mut c, &store, caps, inbox.id).await;
    assert!(r.is_err(), "the connection dropped after one batch");
    let rows = store.messages(inbox.id, 10, 0).unwrap();
    let shown: Vec<&str> = rows.iter().map(|m| m.subject.as_str()).collect();
    assert!(!shown.is_empty(), "the first batch is kept");
    assert_eq!(
        shown.first().copied(),
        Some(summary.newest[0].as_str()),
        "the newest message is on top after the first batch, not years-old mail: {shown:?}"
    );
    assert_eq!(&shown[..5], &summary.newest[..5].iter().map(String::as_str).collect::<Vec<_>>()[..]);
}

/// The PlayStation receipt is in INBOX, „Belege“ and „Alle Nachrichten“: one row in the inbox
/// (every tab and the unified inbox), one hit in search.
#[tokio::test]
async fn one_message_in_three_folders_is_one_row() {
    let (srv, summary) = gmail_server(400).await;
    let (store, account) = store_for(&srv);
    let mut c = connect(&srv).await;
    sync_account(&mut c, &store, account).await.unwrap();
    let mut ids: Vec<String> = vec![];
    for all in [Some(account), None] {
        ids.clear();
        for tab in Category::ALL {
            let (_, rows) =
                mail_core::triage::inbox(&store, &InboxQuery { account: all, ..query(account, tab) }, 5000, 0).unwrap();
            for r in rows {
                let key =
                    format!("{}|{}|{}", r.subject, r.from.as_ref().map(|a| a.email.as_str()).unwrap_or(""), r.date);
                ids.push(key);
            }
        }
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "no message twice in the inbox (account {all:?})");
    }
    let ps: Vec<String> = ids.iter().filter(|k| k.starts_with(&summary.playstation)).cloned().collect();
    assert!(!ps.is_empty() && ps.windows(2).all(|w| w[0] != w[1]), "{ps:?}");
    let hits = mail_core::search::search(&store, "PlayStation", Some(account), 500).unwrap();
    let mut keys: Vec<(String, i64)> = hits.iter().map(|h| (h.subject.clone(), h.date)).collect();
    let n = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), n, "search shows each PlayStation mail once: {hits:#?}");
}

/// Receipts and newsletters as their senders write them are not people.
#[tokio::test]
async fn receipts_and_newsletters_stay_out_of_people() {
    let (srv, summary) = gmail_server(200).await;
    let (store, account) = store_for(&srv);
    let mut c = connect(&srv).await;
    sync_account(&mut c, &store, account).await.unwrap();
    let (_, people) = mail_core::triage::inbox(&store, &query(account, Category::People), 5000, 0).unwrap();
    let wrong: Vec<String> = summary
        .expected
        .iter()
        .filter(|e| e.category != "people")
        .filter(|e| people.iter().any(|p| p.subject == e.subject))
        .map(|e| format!("{} ({}, expected {})", e.subject, e.from, e.category))
        .collect();
    assert!(wrong.is_empty(), "in Personen: {wrong:#?}");
    for e in summary.expected.iter().filter(|e| e.category == "receipts") {
        let (_, rows) = mail_core::triage::inbox(&store, &query(account, Category::Receipts), 5000, 0).unwrap();
        assert!(rows.iter().any(|r| r.subject == e.subject), "{} ({}) not under Belege", e.subject, e.from);
    }
}

/// The newest messages get their first lines (HTML-only shop mail too), and a picture inside
/// an HTML message is shown from the message itself.
#[tokio::test]
async fn newest_mail_has_first_lines_and_pictures_inside_show() {
    let (srv, summary) = gmail_server(300).await;
    let (store, account) = store_for(&srv);
    let mut c = connect(&srv).await;
    sync_account(&mut c, &store, account).await.unwrap();
    let inbox = store.mailbox_by_role(account, Role::Inbox).unwrap().unwrap();
    prefetch_bodies(&mut c, &store, inbox.id, Prefetch::Recent).await.unwrap();
    let rows = store.messages(inbox.id, 20, 0).unwrap();
    let empty: Vec<&str> = rows.iter().filter(|m| m.snippet.trim().is_empty()).map(|m| m.subject.as_str()).collect();
    assert!(empty.is_empty(), "no first lines for {empty:?}");
    let ps = rows.iter().find(|m| m.subject == summary.playstation).unwrap();
    assert!(ps.snippet.contains("Vielen Dank"), "{:?}", ps.snippet);

    let pic = rows.iter().find(|m| m.subject == summary.cid).unwrap();
    fetch_body(&mut c, &store, pic.id).await.unwrap();
    let view = mail_core::actions::message_view(&store, pic.id, RemoteContent::Ask).unwrap();
    assert!(!view.body.html.contains("cid:"), "{}", view.body.html);
    assert!(view.body.html.contains("data:image/png"), "the picture inside is shown: {}", view.body.html);

    let nl = rows.iter().find(|m| m.subject == summary.remote).unwrap();
    fetch_body(&mut c, &store, nl.id).await.unwrap();
    let view = mail_core::actions::message_view(&store, nl.id, RemoteContent::Ask).unwrap();
    assert_eq!(view.body.blocked_remote, 1, "the picture from the web waits");
    assert!(!view.body.html.contains("bilder.zeit.example"), "{}", view.body.html);
    let view = mail_core::actions::message_view(&store, nl.id, RemoteContent::Always).unwrap();
    assert!(view.body.html.contains("https://bilder.zeit.example/"), "loaded when allowed: {}", view.body.html);
}
