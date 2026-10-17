//! New mail while the phone app is not in front. Neither platform lets an app keep a connection
//! open in the background, and there is no server of ours to push (docs/plan.md, open decision
//! 5): Android runs [`run`] about every 15 minutes through WorkManager (`MailCheckWorker.kt`),
//! iOS when the system grants background app refresh (`mobile/ios.rs`). Each run checks every
//! account once (`mail_core::engine::check_once`: the offline queue, the inbox, its first
//! lines) within a time budget and answers what to tell the user: a notification per new
//! message and the unread count for the badge. The platform code shows them. Before that it does
//! what the time tools have due (`mail_core::later::tick`): messages to send later go to the
//! queue the check replays, snoozed messages come back, reminders become notifications.
//!
//! The texts are the desktop's notifications' ([`new_mail_text`]). Everything stays on the
//! device: the check talks only to the user's mail servers.

// Built and tested on the desktop too; only the phones run it.
#![cfg_attr(desktop, allow(dead_code))]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mail_core::account::Account;
use mail_core::store::{MessageSummary, Store};
use serde::Serialize;

/// The system wants its time back (iOS: the task's expiration): [`run`] answers with what it has.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// At most this many notifications per account and run; more new messages make one summary.
pub const NOTES_PER_ACCOUNT: usize = 4;

/// One notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    /// Stable per message (or per account for a summary), so a later run replaces it.
    pub key: String,
    /// The account's address: notifications of one account are grouped.
    pub group: String,
    pub title: String,
    pub body: String,
    /// The message of a note about one message: its buttons (archive, mark read, reply) act on
    /// it. None for a summary or a reminder.
    pub id: Option<i64>,
}

/// What a run found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Nothing checked: the app is in front (its own connections are open) or the user switched
    /// the background check off.
    pub skipped: bool,
    pub notes: Vec<Note>,
    /// Unread messages of all inboxes, for the badge.
    pub unread: u32,
}

/// What a notification about new mail says: for one message the sender and the subject, for
/// more how many and the account. `None` without messages.
pub fn new_mail_text(list: &[MessageSummary], account: Option<&Account>) -> Option<(String, String)> {
    let first = list.first()?;
    Some(if list.len() == 1 {
        let from = first.from.as_ref().map(|a| a.name.clone().unwrap_or_else(|| a.email.clone())).unwrap_or_default();
        let subject = if first.subject.trim().is_empty() {
            mail_core::tr!("(Kein Betreff)", "(No subject)").to_string()
        } else {
            first.subject.clone()
        };
        (from, subject)
    } else {
        (
            mail_core::trf!("{n} neue Nachrichten", "{n} new messages", n = list.len()),
            account.map(|a| if a.name.is_empty() { a.email.clone() } else { a.name.clone() }).unwrap_or_default(),
        )
    })
}

/// What a notification about a reminder without an answer says.
pub fn reminder_text(r: &mail_core::later::Reminder) -> (String, String) {
    let subject = if r.subject.trim().is_empty() {
        mail_core::tr!("(Kein Betreff)", "(No subject)").to_string()
    } else {
        r.subject.clone()
    };
    (mail_core::tr!("Keine Antwort bisher", "No answer yet").to_string(), subject)
}

/// The notifications for the new messages of one account: one per message (newest first), or
/// one summary when there are more than [`NOTES_PER_ACCOUNT`].
pub fn notes_for(account: &Account, mut list: Vec<MessageSummary>) -> Vec<Note> {
    list.sort_by_key(|m| std::cmp::Reverse(m.date));
    let group = account.email.clone();
    if list.len() > NOTES_PER_ACCOUNT {
        return new_mail_text(&list, Some(account))
            .map(|(title, body)| Note { key: format!("account-{}", account.id), group, title, body, id: None })
            .into_iter()
            .collect();
    }
    list.iter()
        .filter_map(|m| {
            let (title, body) = new_mail_text(std::slice::from_ref(m), Some(account))?;
            Some(Note { key: format!("message-{}", m.id), group: group.clone(), title, body, id: Some(m.id) })
        })
        .collect()
}

/// The app lock is on: one notification per account with the count only (no sender, no subject,
/// no buttons), the group a neutral one, so not even the account shows.
pub fn private_note(account: &Account, count: usize) -> Option<Note> {
    (count > 0).then(|| {
        let (title, body) = mail_core::applock::private_notification(count);
        Note { key: format!("account-{}", account.id), group: "arcalo-mail".into(), title, body, id: None }
    })
}

/// Asks a running [`run`] to stop now.
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// Checks every account once within `budget` and returns what to show. Runs without the app's
/// state (Android starts only this part when the app is closed), so it opens the store and the
/// credential store of `data_dir` itself.
pub fn run(data_dir: &Path, budget: Duration) -> Report {
    CANCEL.store(false, Ordering::SeqCst);
    let db = data_dir.join(mail_core::cipher::DB_FILE);
    if !db.exists() {
        return Report { skipped: true, ..Report::default() };
    }
    // An encrypted database opens with its key from the credential store (the Android Keystore,
    // the iOS Keychain after the first unlock), as in the app.
    crate::secrets::init(data_dir);
    if crate::security::prepare(data_dir).blocked.is_some() {
        return Report { skipped: true, ..Report::default() };
    }
    let Ok(store) = Store::open(&db) else { return Report { skipped: true, ..Report::default() } };
    let settings = mail_core::settings::load(&store).unwrap_or_default();
    if !settings.background_check {
        return Report { skipped: true, unread: store.unread_in_inboxes().unwrap_or(0), notes: vec![] };
    }
    crate::apply_language(settings.language);
    crate::apply_network(&settings.network);
    let accounts = store.accounts().unwrap_or_default();
    let due = mail_core::later::tick(&store, mail_core::store::now()).unwrap_or_default();

    // One thread per account with a runtime of its own (the check is not `Send`).
    let (tx, rx) = mpsc::channel::<(i64, Vec<i64>)>();
    let mut waiting = 0;
    for account in &accounts {
        let connection = match crate::connection_for(account, &db) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let (id, db, tx) = (account.id, db.clone(), tx.clone());
        let spawned =
            std::thread::Builder::new().name(format!("check-{id}")).stack_size(16 * 1024 * 1024).spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
                let Ok(own) = Store::open(&db) else { return };
                let check = async {
                    match connection {
                        crate::Connection::Net(c) => mail_core::engine::check_once(id, own, c).await,
                        crate::Connection::Jmap(c) => mail_core::engine::check_once(id, own, c).await,
                        crate::Connection::Pop3(c) => mail_core::engine::check_once(id, own, c).await,
                    }
                };
                let found = match rt.block_on(async { tokio::time::timeout(budget, check).await }) {
                    Ok(Ok(new)) => new,
                    Ok(Err(e)) => {
                        mail_core::diag::log("background", &format!("account {id}: {e}"));
                        vec![]
                    }
                    Err(_) => vec![],
                };
                let _ = tx.send((id, found));
            });
        if spawned.is_ok() {
            waiting += 1;
        }
    }
    drop(tx);

    let deadline = Instant::now() + budget + Duration::from_secs(2);
    let mut found: Vec<(i64, Vec<i64>)> = vec![];
    while waiting > 0 && !CANCEL.load(Ordering::SeqCst) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(r) => {
                waiting -= 1;
                found.push(r);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let mut notes = vec![];
    // With the app lock on, the notifications say how many and nothing else.
    let private = crate::security::private(&store);
    if settings.notifications {
        for (id, new) in found {
            let Some(account) = accounts.iter().find(|a| a.id == id) else { continue };
            // Only what is still unread (read on another device in the meantime: no note), and
            // with Focus or the Screener only what they let through.
            let new = mail_core::triage::worth_telling(&store, id, &new).unwrap_or(new);
            let list: Vec<_> = store.summaries(&new).unwrap_or_default().into_iter().filter(|m| !m.seen).collect();
            if private {
                notes.extend(private_note(account, list.len()));
            } else {
                notes.extend(notes_for(account, list));
            }
        }
        for (i, r) in due.reminders.iter().enumerate() {
            let group = accounts.iter().find(|a| a.id == r.account_id).map(|a| a.email.clone()).unwrap_or_default();
            let (title, body) = if private { mail_core::applock::private_reminder() } else { reminder_text(r) };
            notes.push(Note { key: format!("reminder-{}-{i}", r.account_id), group, title, body, id: None });
        }
    }
    // The home-screen widget shows what came in.
    crate::write_widget(&store, data_dir);
    Report { skipped: false, notes, unread: store.unread_in_inboxes().unwrap_or(0) }
}

/// A notification's button pressed while the app is not running (Android): archived or marked
/// read in the store of `data_dir`; the next check carries it to the server. Returns the unread
/// count for the badge.
pub fn quick_action(data_dir: &Path, message_id: i64, action: &str) -> Option<u32> {
    crate::secrets::init(data_dir);
    if crate::security::prepare(data_dir).blocked.is_some() {
        return None;
    }
    let store = Store::open(&data_dir.join(mail_core::cipher::DB_FILE)).ok()?;
    if let Err(e) = mail_core::actions::quick_action(&store, message_id, action) {
        mail_core::diag::log("background", &format!("notification action: {e}"));
    }
    crate::write_widget(&store, data_dir);
    store.unread_in_inboxes().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_core::account::{AccountDraft, Provider, preset};
    use mail_core::parse::Address;

    fn account(store: &Store) -> Account {
        let id = store
            .add_account(&AccountDraft {
                provider: Provider::Imap,
                name: String::new(),
                email: "maria@example.org".into(),
                sender_name: "Maria".into(),
                transport: preset(Provider::Imap, "maria@example.org").unwrap(),
            })
            .unwrap();
        store.account(id).unwrap()
    }

    fn summary(id: i64, from: &str, subject: &str, date: i64) -> MessageSummary {
        MessageSummary {
            id,
            mailbox_id: 1,
            uid: id as u32,
            thread_id: id,
            subject: subject.into(),
            from: Some(Address { name: Some(from.into()), email: format!("{}@example.org", from.to_lowercase()) }),
            date,
            snippet: String::new(),
            seen: false,
            flagged: false,
            answered: false,
            has_attachments: false,
            thread_count: 1,
            ..MessageSummary::default()
        }
    }

    #[test]
    fn a_note_per_message_or_one_for_many() {
        mail_core::i18n::set_lang(mail_core::i18n::Lang::En);
        let store = Store::open_in_memory().unwrap();
        let a = account(&store);
        let notes = notes_for(&a, vec![summary(1, "Jonas", "Lunch", 10), summary(2, "Lea", "", 20)]);
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].title.as_str(), notes[0].body.as_str()), ("Lea", "(No subject)"), "newest first");
        assert_eq!(notes[1].key, "message-1");
        assert_eq!(notes[1].id, Some(1), "its buttons act on the message");
        assert_eq!(notes[1].group, "maria@example.org");

        let many: Vec<_> = (1..=5).map(|i| summary(i, "X", "s", i)).collect();
        let notes = notes_for(&a, many);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "5 new messages");
        assert_eq!(notes[0].body, "maria@example.org");
        assert_eq!(notes[0].id, None);
        assert!(notes_for(&a, vec![]).is_empty());
    }

    #[test]
    fn with_the_lock_on_a_note_says_how_many_and_nothing_else() {
        let store = Store::open_in_memory().unwrap();
        let a = account(&store);
        let n = private_note(&a, 3).unwrap();
        assert_eq!(n.title, "Arcalo Mail");
        assert!(n.body.starts_with('3'), "{}", n.body);
        assert_eq!(n.id, None, "no buttons");
        assert!(!n.group.contains("maria"), "not even the account");
        assert!(private_note(&a, 0).is_none());
    }

    #[test]
    fn nothing_to_check_without_a_store() {
        let dir = std::env::temp_dir().join(format!("arcalo-mail-background-{}", std::process::id()));
        let r = run(&dir, Duration::from_secs(1));
        assert!(r.skipped);
        assert!(r.notes.is_empty());
    }

    #[test]
    fn the_background_check_can_be_switched_off() {
        let dir = std::env::temp_dir().join(format!("arcalo-mail-background-off-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("mail.db")).unwrap();
        account(&store);
        let settings = mail_core::settings::Settings { background_check: false, ..Default::default() };
        mail_core::settings::save(&store, &settings).unwrap();
        let r = run(&dir, Duration::from_secs(1));
        assert!(r.skipped, "{r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
