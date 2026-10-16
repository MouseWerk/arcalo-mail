//! The IMAP client, the sync engine and SMTP against real servers: Dovecot (QRESYNC, CONDSTORE,
//! MOVE, IDLE) and GreenMail (a server without QRESYNC). Only in CI, where the conformance job
//! starts them as containers on the runner; locally no test talks to a real server, so these
//! are `#[ignore]` and run with `cargo test -p mail-testserver --test conformance -- --ignored`
//! and the server in the environment:
//!
//! ```text
//! ARCALO_MAIL_CONFORMANCE_IMAP=127.0.0.1:31143  (plain text, loopback only)
//! ARCALO_MAIL_CONFORMANCE_SMTP=127.0.0.1:3025   (optional)
//! ARCALO_MAIL_CONFORMANCE_USER=…  ARCALO_MAIL_CONFORMANCE_PASSWORD=…
//! ```

use std::time::{Duration, Instant};

use mail_core::account::{AccountDraft, Auth, Provider, Security, Server, Transport};
use mail_core::imap::client::ImapSession;
use mail_core::imap::{FLAGGED, IdleOutcome, ImapClient, Role, SEEN, Search};
use mail_core::store::Store;
use mail_core::sync::{fetch_body, sync_account, sync_mailbox};
use tokio::sync::Notify;

struct Target {
    imap: Server,
    smtp: Option<Server>,
    user: String,
    password: String,
}

fn server(var: &str) -> Option<Server> {
    let v = std::env::var(var).ok().filter(|v| !v.is_empty())?;
    let (host, port) = v.rsplit_once(':')?;
    Some(Server::new(host, port.parse().ok()?, Security::Plain))
}

fn target() -> Target {
    Target {
        imap: server("ARCALO_MAIL_CONFORMANCE_IMAP")
            .expect("ARCALO_MAIL_CONFORMANCE_IMAP=host:port (see the module comment)"),
        smtp: server("ARCALO_MAIL_CONFORMANCE_SMTP"),
        user: std::env::var("ARCALO_MAIL_CONFORMANCE_USER").expect("ARCALO_MAIL_CONFORMANCE_USER"),
        password: std::env::var("ARCALO_MAIL_CONFORMANCE_PASSWORD").expect("ARCALO_MAIL_CONFORMANCE_PASSWORD"),
    }
}

fn message(id: &str, subject: &str, to: &str) -> Vec<u8> {
    format!(
        "From: Jonas Berger <jonas@example.org>\r\nTo: {to}\r\nSubject: {subject}\r\n\
Date: Fri, 16 Oct 2026 09:30:00 +0200\r\nMessage-ID: <{id}>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nGrüße aus dem Konformitätstest {id}\r\n"
    )
    .into_bytes()
}

fn unique() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
    )
}

/// Folders, APPEND, the sync engine (first sync and the cheapest follow-up the server allows),
/// bodies, flags, SEARCH, MOVE (or COPY and EXPUNGE), RENAME and DELETE, IDLE and SMTP.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to a real server: the conformance job of CI"]
async fn a_real_server() {
    let t = target();
    let run = unique();
    let mut c = ImapSession::connect(&t.imap, &t.user, &t.password).await.expect("sign in");
    let caps = c.capabilities().await.unwrap();
    println!("capabilities: {caps:?}");
    let folder = format!("Conformance-{run}");
    let target_folder = format!("{folder}-Ziel ü");
    c.create(&folder).await.unwrap();
    c.create(&target_folder).await.unwrap();
    for i in 1..=3 {
        c.append(
            &folder,
            if i == 1 { &[SEEN] } else { &[] },
            &message(&format!("m{i}-{run}@example.org"), &format!("Nachricht {i}"), &t.user),
        )
        .await
        .unwrap();
    }

    // The sync engine on a store of its own.
    let store = Store::open_in_memory().unwrap();
    let account = store
        .add_account(&AccountDraft {
            provider: Provider::Imap,
            name: String::new(),
            email: t.user.clone(),
            sender_name: String::new(),
            transport: Transport::Imap {
                imap: t.imap.clone(),
                smtp: t.smtp.clone().unwrap_or_else(|| t.imap.clone()),
                username: t.user.clone(),
                auth: Auth::Password,
            },
        })
        .unwrap();
    sync_account(&mut c, &store, account).await.unwrap();
    let mb = store.mailbox_by_name(account, &folder).unwrap().expect("folder synced");
    let list = store.messages(mb.id, 10, 0).unwrap();
    assert_eq!(list.len(), 3);
    assert!(store.mailbox_by_role(account, Role::Inbox).unwrap().is_some());
    let first = list.iter().find(|m| m.subject == "Nachricht 1").unwrap();
    assert!(first.seen, "flags of APPEND");
    fetch_body(&mut c, &store, first.id).await.unwrap();
    assert!(store.message(first.id).unwrap().has_body);

    // A change from another connection, found by the next sync.
    let mut other = ImapSession::connect(&t.imap, &t.user, &t.password).await.unwrap();
    other.select(&folder, None).await.unwrap();
    let uids = other.uids().await.unwrap();
    other.store(&uids[1..2], &[FLAGGED], true).await.unwrap();
    other.append(&folder, &[], &message(&format!("m4-{run}@example.org"), "Nachricht 4", &t.user)).await.unwrap();
    let r = sync_mailbox(&mut c, &store, caps, mb.id).await.unwrap();
    println!("follow-up sync: {:?}", r.strategy);
    assert_eq!(r.added, 1);
    assert!(store.messages(mb.id, 10, 0).unwrap().iter().any(|m| m.flagged), "flag from the other connection");

    // Search, move, expunge.
    c.select(&folder, None).await.unwrap();
    let hits = c.search(&Search::MessageId(format!("m2-{run}@example.org"))).await.unwrap();
    assert_eq!(hits.len(), 1);
    c.move_to(&hits, &target_folder).await.unwrap();
    c.select(&target_folder, None).await.unwrap();
    assert_eq!(c.uids().await.unwrap().len(), 1);
    c.select(&folder, None).await.unwrap();
    let rest = c.uids().await.unwrap();
    assert_eq!(rest.len(), 3);
    c.expunge(&rest[..1]).await.unwrap();
    assert_eq!(c.uids().await.unwrap().len(), 2);
    let r = sync_mailbox(&mut c, &store, caps, mb.id).await.unwrap();
    assert_eq!(r.removed, 2, "moved and expunged");

    // IDLE ends when another connection delivers.
    if caps.idle {
        c.select(&folder, None).await.unwrap();
        let (folder2, run2) = (folder.clone(), run.clone());
        let (imap, user, password) = (t.imap.clone(), t.user.clone(), t.password.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let mut o = ImapSession::connect(&imap, &user, &password).await.unwrap();
            o.append(&folder2, &[], &message(&format!("m5-{run2}@example.org"), "Nachricht 5", &user)).await.unwrap();
        });
        let start = Instant::now();
        let outcome = c.idle(Duration::from_secs(30), &Notify::new()).await.unwrap();
        assert_eq!(outcome, IdleOutcome::Changed);
        assert!(start.elapsed() < Duration::from_secs(20));
    }

    // Rename and delete.
    let renamed = format!("{folder}-umbenannt");
    c.select("INBOX", None).await.unwrap();
    c.rename(&folder, &renamed).await.unwrap();
    let names: Vec<String> = c.list().await.unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&renamed) && !names.contains(&folder), "{names:?}");
    c.delete(&renamed).await.unwrap();
    c.delete(&target_folder).await.unwrap();
    let names: Vec<String> = c.list().await.unwrap().into_iter().map(|e| e.name).collect();
    assert!(!names.contains(&renamed) && !names.contains(&target_folder), "{names:?}");

    // Sending, where the server takes mail.
    if let Some(smtp) = &t.smtp {
        let raw = message(&format!("s-{run}@example.org"), &format!("Gesendet {run}"), &t.user);
        mail_core::smtp::send(smtp, &t.user, &t.password, &t.user, &[t.user.clone()], &raw).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            c.select("INBOX", None).await.unwrap();
            if !c.search(&Search::MessageId(format!("s-{run}@example.org"))).await.unwrap().is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "the message did not arrive");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    let _ = c.logout().await;
}
