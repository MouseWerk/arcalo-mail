//! Threads (conversations) from `Message-ID`, `In-Reply-To` and `References`.
//!
//! A message joins the thread of any message it refers to; a message that arrives before its
//! parent is adopted when the parent comes, and two threads that a message connects are
//! merged. The thread id is the id of a message in it. Grouping by subject alone („Re: …“
//! without headers, some mobile clients) is left out on purpose for now: it joins unrelated
//! mails („Rechnung“) and needs JWZ's rules with a time window (docs/plan.md).

use rusqlite::params;

use crate::error::Result;
use crate::parse::ParsedMessage;
use crate::store::Store;

/// Gives the stored message `id` its thread.
pub fn assign(store: &Store, account_id: i64, id: i64, p: &ParsedMessage) -> Result<()> {
    let conn = store.conn();
    let mut related: Vec<String> = p.references.clone();
    if let Some(r) = &p.in_reply_to {
        related.push(r.clone());
    }
    let mut threads: Vec<i64> = vec![];
    // Parents and ancestors that are already here.
    let mut st = conn.prepare_cached(
        "SELECT DISTINCT thread_id FROM messages WHERE account_id = ?1 AND message_id = ?2 AND id <> ?3",
    )?;
    for r in &related {
        for t in st.query_map(params![account_id, r, id], |row| row.get::<_, i64>(0))? {
            threads.push(t?);
        }
    }
    // Copies of this message in other folders, and replies that arrived first.
    if let Some(own) = &p.message_id {
        for t in st.query_map(params![account_id, own, id], |row| row.get::<_, i64>(0))? {
            threads.push(t?);
        }
        let mut replies = conn.prepare_cached(
            "SELECT DISTINCT thread_id FROM messages WHERE account_id = ?1 AND id <> ?3
               AND (in_reply_to = ?2 OR EXISTS (SELECT 1 FROM json_each(messages.refs) WHERE value = ?2))",
        )?;
        for t in replies.query_map(params![account_id, own, id], |row| row.get::<_, i64>(0))? {
            threads.push(t?);
        }
    }
    threads.retain(|t| *t != 0);
    threads.sort_unstable();
    threads.dedup();
    let thread = threads.first().copied().unwrap_or(id);
    conn.execute("UPDATE messages SET thread_id = ?2 WHERE id = ?1", params![id, thread])?;
    for other in threads.iter().skip(1) {
        conn.execute(
            "UPDATE messages SET thread_id = ?2 WHERE account_id = ?1 AND thread_id = ?3",
            params![account_id, thread, other],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::parse::{parse, tests::sample};
    use crate::store::tests::{header, store_with_inbox};

    #[test]
    fn replies_join_their_thread() {
        let (s, _, inbox) = store_with_inbox();
        let add = |uid: u32, id: &str, reply: Option<&str>| {
            let raw = sample(id, "Angebot", reply, "x");
            s.insert_message(inbox, &header(uid, &raw), &parse(&raw).unwrap()).unwrap()
        };
        let a = add(1, "a@x", None);
        let b = add(2, "b@x", Some("a@x"));
        let other = add(3, "c@x", None);
        let list = s.messages(inbox, 10, 0).unwrap();
        let thread_of = |id: i64| list.iter().find(|m| m.id == id).unwrap().thread_id;
        assert_eq!(thread_of(a), thread_of(b));
        assert_ne!(thread_of(a), thread_of(other));
        assert_eq!(s.thread(thread_of(a)).unwrap().len(), 2);
    }

    #[test]
    fn a_reply_before_its_parent_is_adopted_and_threads_merge() {
        let (s, _, inbox) = store_with_inbox();
        let add = |uid: u32, id: &str, reply: Option<&str>| {
            let raw = sample(id, "Plan", reply, "x");
            s.insert_message(inbox, &header(uid, &raw), &parse(&raw).unwrap()).unwrap()
        };
        let reply1 = add(1, "r1@x", Some("root@x"));
        let reply2 = add(2, "r2@x", Some("root@x"));
        let root = add(3, "root@x", None);
        let ids: Vec<i64> =
            s.thread(s.messages(inbox, 10, 0).unwrap()[0].thread_id).unwrap().iter().map(|m| m.id).collect();
        for id in [reply1, reply2, root] {
            assert!(ids.contains(&id), "{id} not in {ids:?}");
        }
    }
}
