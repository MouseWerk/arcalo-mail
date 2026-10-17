//! What the phone's home-screen widget shows: the unread count of the inboxes and the newest
//! three mails of the People tab. The shell writes it as JSON next to the database after each
//! sync and background check (`widget.json`); the Android widget reads that file, so it needs
//! neither the app running nor the database.

use serde::Serialize;

use crate::error::Result;
use crate::store::Store;
use crate::triage::{Category, InboxQuery, inbox};

/// A mail of the widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub id: i64,
    pub from: String,
    pub subject: String,
    /// Unix seconds.
    pub date: i64,
    pub unread: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Widget {
    pub unread: u32,
    pub people: Vec<Row>,
    /// When it was written (Unix seconds).
    pub at: i64,
}

pub fn snapshot(store: &Store) -> Result<Widget> {
    let q = InboxQuery { account: None, tab: Category::People, focus: false, threads: true, bundles: false };
    let (_, rows) = inbox(store, &q, 3, 0)?;
    Ok(Widget {
        unread: store.unread_in_inboxes()?,
        people: rows
            .into_iter()
            .map(|m| Row {
                id: m.id,
                from: m.from.map(|a| a.name.filter(|n| !n.trim().is_empty()).unwrap_or(a.email)).unwrap_or_default(),
                subject: if m.subject.trim().is_empty() {
                    crate::tr!("(Ohne Betreff)", "(No subject)").into()
                } else {
                    m.subject
                },
                date: m.date,
                unread: !m.seen,
            })
            .collect(),
        at: crate::store::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{AccountDraft, Provider, preset};
    use crate::imap::MailboxEntry;
    use crate::parse::{parse, tests::sample};
    use crate::store::tests::header;

    #[test]
    fn unread_count_and_the_newest_people_mails() {
        let s = Store::open_in_memory().unwrap();
        let a = s
            .add_account(&AccountDraft {
                provider: Provider::Imap,
                name: String::new(),
                email: "maria@example.org".into(),
                sender_name: String::new(),
                transport: preset(Provider::Imap, "maria@example.org").unwrap(),
            })
            .unwrap();
        let inbox_id = s
            .upsert_mailbox(a, &MailboxEntry { name: "INBOX".into(), delimiter: Some('/'), attributes: vec![] })
            .unwrap();
        for uid in 1..=4 {
            let raw = sample(&format!("w{uid}@x"), &format!("Frage {uid}"), None, "Hallo");
            let mut h = header(uid, &raw);
            h.internal_date = 1_792_000_000 + uid as i64;
            s.insert_message(inbox_id, &h, &parse(&raw).unwrap()).unwrap();
        }
        let w = snapshot(&s).unwrap();
        assert_eq!(w.unread, 4);
        assert_eq!(w.people.len(), 3, "the newest three");
        assert!(w.people.iter().all(|r| r.from == "Maria Muster" && r.unread));
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains("\"unread\":4"));
    }
}
