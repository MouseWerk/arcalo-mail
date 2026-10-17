//! Views over all accounts, as „Alle Postfächer“ shows them: the inboxes, sent mail, drafts or
//! archives of every account in one list, newest first, and the unread counts per account.
//!
//! Pages go by a keyset ([`Cursor`]: the date and id of the last row shown), never by an
//! offset: the next page starts where the index says, however far down the list is, and a
//! message that arrives meanwhile does not shift the rows. The list walks the index
//! `messages_by_date` (migration 0012) and stops after one page; the counts read the index
//! `messages_unread`.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::imap::Role;
use crate::store::{MessageSummary, SUMMARY_COLS, Store, summary};

/// Where a page ends: the sort date (`Date`, else the arrival) and id of its last row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub date: i64,
    pub id: i64,
}

/// One page of a list and where the next one starts (`None`: this was the last).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Page {
    pub items: Vec<MessageSummary>,
    pub next: Option<Cursor>,
}

/// The folders of a role in every account (or in one), newest first, `limit` rows after
/// `after`. Folders with much mail walk the date index; drafts, junk and trash, which are
/// small, go by their folders.
pub fn folder_page(store: &Store, role: Role, account: Option<i64>, after: Option<Cursor>, limit: u32) -> Result<Page> {
    let index =
        if matches!(role, Role::Drafts | Role::Junk | Role::Trash) { "messages_visible" } else { "messages_by_date" };
    let sql = format!(
        "SELECT {SUMMARY_COLS} FROM messages INDEXED BY {index}
         WHERE hidden = 0
           AND mailbox_id IN (SELECT id FROM mailboxes WHERE role = ?1 AND (?2 IS NULL OR account_id = ?2))
           AND (?3 IS NULL OR (COALESCE(date, internal_date), id) < (?3, ?4))
         ORDER BY COALESCE(date, internal_date) DESC, id DESC LIMIT ?5"
    );
    let mut st = store.conn().prepare_cached(&sql)?;
    let rows =
        st.query_map(params![role.as_str(), account, after.map(|c| c.date), after.map(|c| c.id), limit], summary)?;
    let items: Vec<MessageSummary> = rows.collect::<rusqlite::Result<_>>()?;
    let next = (items.len() as u32 == limit).then(|| items.last().map(|m| Cursor { date: m.date, id: m.id })).flatten();
    Ok(Page { items, next })
}

/// The unread and all messages of one account's inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AccountCount {
    pub account_id: i64,
    pub unread: u32,
    pub total: u32,
}

/// The inbox counts of every account (the account switcher's numbers).
pub fn inbox_counts(store: &Store) -> Result<Vec<AccountCount>> {
    let mut st = store.conn().prepare_cached(
        "SELECT b.account_id,
           (SELECT COUNT(*) FROM messages m INDEXED BY messages_unread
             WHERE m.mailbox_id = b.id AND m.hidden = 0 AND m.seen = 0),
           (SELECT COUNT(*) FROM messages m INDEXED BY messages_unread WHERE m.mailbox_id = b.id AND m.hidden = 0)
         FROM mailboxes b WHERE b.role = 'inbox' ORDER BY b.account_id",
    )?;
    let rows = st.query_map([], |r| Ok(AccountCount { account_id: r.get(0)?, unread: r.get(1)?, total: r.get(2)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::account::{AccountDraft, Provider, preset};
    use crate::imap::MailboxEntry;
    use std::time::{Duration, Instant};

    /// A store with `accounts` accounts of `per_account` messages each (70 % inbox, 15 % archive,
    /// 10 % sent, 5 % drafts), written in bulk; with their search entries.
    pub(crate) fn big_store(accounts: u32, per_account: u32) -> Store {
        let s = Store::open_in_memory().unwrap();
        for a in 0..accounts {
            let email = format!("konto{a}@example.org");
            let account = s
                .add_account(&AccountDraft {
                    provider: Provider::Imap,
                    name: String::new(),
                    email: email.clone(),
                    sender_name: String::new(),
                    transport: preset(Provider::Imap, &email).unwrap(),
                })
                .unwrap();
            let folder = |name: &str, attr: &str| {
                s.upsert_mailbox(
                    account,
                    &MailboxEntry {
                        name: name.into(),
                        delimiter: Some('/'),
                        attributes: if attr.is_empty() { vec![] } else { vec![attr.into()] },
                    },
                )
                .unwrap()
            };
            let (inbox, archive, sent, drafts) = (
                folder("INBOX", ""),
                folder("Archive", "\\Archive"),
                folder("Sent", "\\Sent"),
                folder("Drafts", "\\Drafts"),
            );
            s.conn()
                .execute(
                    "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
                     INSERT INTO messages (account_id, mailbox_id, uid, message_id, subject, from_name, from_email,
                       date, internal_date, size, seen, snippet, base_subject, category, thread_id)
                     SELECT ?2, CASE WHEN i % 20 < 14 THEN ?3 WHEN i % 20 < 17 THEN ?4 WHEN i % 20 < 19 THEN ?5 ELSE ?6 END,
                       i, 'm' || i || '-' || ?2 || '@example.net',
                       'Betreff ' || i || CASE WHEN i % 97 = 0 THEN ' Lissabon' ELSE '' END,
                       'Absender ' || (i % 700), 'absender' || (i % 700) || '@example.net',
                       1700000000 + i * 61 + ?2, 1700000000 + i * 61 + ?2, 4000, i % 3 <> 0,
                       'Die ersten Zeilen der Nachricht ' || i,
                       'betreff ' || i,
                       CASE i % 5 WHEN 0 THEN 'newsletters' WHEN 1 THEN 'notifications' WHEN 2 THEN 'receipts' ELSE 'people' END,
                       i
                     FROM n",
                    params![per_account, account, inbox, archive, sent, drafts],
                )
                .unwrap();
        }
        s.conn()
            .execute(
                "INSERT INTO messages_fts (rowid, subject, people, body) SELECT id, subject, from_email, snippet FROM messages",
                [],
            )
            .unwrap();
        s.conn().execute("UPDATE messages SET thread_id = id", []).unwrap();
        s
    }

    /// How long `f` takes, the best of three (the first run warms the caches).
    pub(crate) fn timed<T>(mut f: impl FnMut() -> T) -> (T, Duration) {
        let mut best = Duration::MAX;
        let mut out = None;
        for _ in 0..3 {
            let t = Instant::now();
            out = Some(f());
            best = best.min(t.elapsed());
        }
        (out.unwrap(), best)
    }

    fn plan(s: &Store, sql: &str) -> String {
        let mut st = s.conn().prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = st.query_map([], |r| r.get::<_, String>(3)).unwrap();
        rows.map(|r| r.unwrap()).collect::<Vec<_>>().join(" | ")
    }

    /// 3 accounts of 30,000 messages: the first page of each unified folder and of a tab of the
    /// unified inbox, the next page, the counts and search keep their budgets (with room for a
    /// slow CI machine: twice the targets of 50 ms, 20 ms and 200 ms).
    #[test]
    fn unified_views_of_three_large_accounts_are_fast() {
        let s = big_store(3, 30_000);
        for role in [Role::Inbox, Role::Sent, Role::Drafts, Role::Archive] {
            let (p, t) = timed(|| folder_page(&s, role, None, None, 50).unwrap());
            assert_eq!(p.items.len(), 50, "{role:?}");
            assert!(p.items.windows(2).all(|w| (w[0].date, w[0].id) > (w[1].date, w[1].id)), "newest first");
            assert!(t < Duration::from_millis(100), "{role:?}: first page {t:?}");
            let (next, t) = timed(|| folder_page(&s, role, None, p.next, 50).unwrap());
            assert!(t < Duration::from_millis(100), "{role:?}: next page {t:?}");
            let last = p.items.last().unwrap();
            assert!((next.items[0].date, next.items[0].id) < (last.date, last.id), "goes on where the page ended");
            assert!(next.items.iter().all(|m| !p.items.iter().any(|o| o.id == m.id)));
        }
        let accounts: std::collections::HashSet<i64> =
            folder_page(&s, Role::Inbox, None, None, 50).unwrap().items.iter().map(|m| m.account_id).collect();
        assert_eq!(accounts.len(), 3, "all accounts in one list");
        let flat = crate::triage::InboxQuery {
            account: None,
            tab: crate::triage::Category::People,
            focus: false,
            threads: false,
            bundles: false,
        };
        let (p, t) = timed(|| crate::triage::inbox_page(&s, &flat, None, 50).unwrap());
        assert_eq!(p.items.len(), 50);
        assert!(t < Duration::from_millis(100), "unified inbox: {t:?}");
        let (offset, _) = timed(|| crate::triage::inbox(&s, &flat, 50, 0).unwrap());
        assert_eq!(
            p.items.iter().map(|m| m.id).collect::<Vec<_>>(),
            offset.1.iter().map(|m| m.id).collect::<Vec<_>>(),
            "the same rows as the paged list"
        );
        let (p2, t) = timed(|| crate::triage::inbox_page(&s, &flat, p.next, 50).unwrap());
        assert!(t < Duration::from_millis(100), "unified inbox, next page: {t:?}");
        let (offset2, _) = timed(|| crate::triage::inbox(&s, &flat, 50, 50).unwrap());
        assert_eq!(
            p2.items.iter().map(|m| m.id).collect::<Vec<_>>(),
            offset2.1.iter().map(|m| m.id).collect::<Vec<_>>()
        );
        let (counts, t) = timed(|| inbox_counts(&s).unwrap());
        assert_eq!(counts.len(), 3);
        assert_eq!(counts[0].total, 21_000);
        assert!(t < Duration::from_millis(40), "counts: {t:?}");
        let (_, t) = timed(|| {
            for a in 1..=3 {
                s.mailboxes(a).unwrap();
            }
        });
        assert!(t < Duration::from_millis(40), "folder counts: {t:?}");
        let (hits, t) = timed(|| crate::search::search(&s, "Lissabon", None, 50).unwrap());
        assert_eq!(hits.len(), 50);
        assert!(t < Duration::from_millis(400), "search: {t:?}");
        let (_, t) = timed(|| crate::search::search(&s, "Betreff", None, 50).unwrap());
        assert!(t < Duration::from_millis(400), "search, a word in every message: {t:?}");
        // The tabs' numbers: as when every message is decided one by one, in a fraction of it.
        crate::triage::set_sender_category(&s, "absender3@example.net", Some(crate::triage::Category::Newsletters))
            .unwrap();
        crate::triage::block_sender(&s, "absender4@example.net").unwrap();
        s.touch_contact(&crate::parse::Address { name: None, email: "Absender5@example.net".into() }, true, 1).unwrap();
        let (tabs, t) = timed(|| crate::triage::counts(&s, None).unwrap());
        assert!(t < Duration::from_millis(40), "tab counts: {t:?}");
        assert_eq!(tabs, crate::triage::full_counts(&s, None).unwrap());
        assert_eq!(crate::triage::counts(&s, Some(2)).unwrap(), crate::triage::full_counts(&s, Some(2)).unwrap());
    }

    #[test]
    #[ignore = "a measurement: run by hand"]
    fn measure_the_unified_views() {
        let s = big_store(3, 30_000);
        let q = crate::triage::InboxQuery {
            account: None,
            tab: crate::triage::Category::People,
            focus: false,
            threads: false,
            bundles: true,
        };
        let (_, t) = timed(|| crate::triage::inbox(&s, &q, 50, 0).unwrap());
        println!("triage inbox, first page: {t:?}");
        let flat = crate::triage::InboxQuery { bundles: false, ..q.clone() };
        let (p, t) = timed(|| crate::triage::inbox_page(&s, &flat, None, 50).unwrap());
        let (p2, t2) = timed(|| crate::triage::inbox_page(&s, &flat, p.next, 50).unwrap());
        println!("triage inbox by keyset: first page {t:?}, next {t2:?} ({} + {})", p.items.len(), p2.items.len());
        let receipts = crate::triage::InboxQuery { tab: crate::triage::Category::Receipts, ..flat.clone() };
        let (_, t) = timed(|| crate::triage::inbox_page(&s, &receipts, None, 50).unwrap());
        println!("receipts by keyset: {t:?}");
        let (_, t) = timed(|| crate::triage::counts(&s, None).unwrap());
        println!("triage counts: {t:?}");
        let (_, t) = timed(|| {
            for a in 1..=3 {
                s.mailboxes(a).unwrap();
            }
        });
        println!("folder counts of 3 accounts: {t:?}");
        let (_, t) = timed(|| crate::search::search(&s, "Lissabon", None, 50).unwrap());
        println!("search across accounts: {t:?}");
        let (_, t) = timed(|| crate::search::search(&s, "Betreff", None, 50).unwrap());
        println!("search, a word in every message: {t:?}");
        for role in [Role::Inbox, Role::Sent, Role::Drafts, Role::Archive] {
            let (p, t) = timed(|| folder_page(&s, role, None, None, 50).unwrap());
            let (_, t2) = timed(|| folder_page(&s, role, None, p.next, 50).unwrap());
            println!("all {}: first page {t:?}, next {t2:?}", role.as_str());
        }
        let (_, t) = timed(|| inbox_counts(&s).unwrap());
        println!("inbox counts per account: {t:?}");
        println!("{}", plan(&s, "SELECT id FROM messages WHERE mailbox_id = 1 AND hidden = 0 AND seen = 0"));
        println!(
            "E: {}",
            plan(
                &s,
                "WITH e(email) AS (SELECT email FROM senders WHERE category IS NOT NULL OR screener = 'blocked' UNION SELECT email FROM contacts WHERE sent > 0) SELECT m.category FROM e CROSS JOIN messages m INDEXED BY messages_sender ON m.from_email = e.email COLLATE NOCASE LEFT JOIN senders s ON s.email = m.from_email LEFT JOIN contacts c ON c.email = m.from_email WHERE m.mailbox_id IN (SELECT id FROM mailboxes WHERE role = 'inbox') AND m.hidden = 0"
            )
        );
        println!(
            "T: {}",
            plan(
                &s,
                "SELECT category, COUNT(*) FROM messages INDEXED BY messages_tabs WHERE mailbox_id IN (SELECT id FROM mailboxes WHERE role = 'inbox') AND hidden = 0 AND (snoozed_until IS NULL OR snoozed_until <= 5) GROUP BY mailbox_id, category"
            )
        );
        let base = crate::triage::BASE.replace("?1", "NULL").replace("?2", "0").replace("?3", "NULL");
        println!("{}", plan(&s, &base));
    }
}
