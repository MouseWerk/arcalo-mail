//! Threads (conversations) from `Message-ID`, `In-Reply-To` and `References`.
//!
//! A message joins the thread of any message it refers to; a message that arrives before its
//! parent is adopted when the parent comes, and two threads that a message connects are
//! merged. The thread id is the id of a message in it.
//!
//! Replies that lost their headers („Re: …“ from some mobile clients and web forms) join by
//! subject, with JWZ's restraint: only a message whose subject says it is a reply or a forward
//! (`Re:`, `AW:`, `Fwd:`, `WG:` …), only when the headers found nothing, only to a message of
//! the same account with the same base subject from [`SUBJECT_WINDOW`] before it, and never
//! for subjects too short to mean anything. A plain „Rechnung“ never joins another „Rechnung“.
//! The setting `subject_threads` switches it off.

use rusqlite::params;

use crate::error::Result;
use crate::parse::ParsedMessage;
use crate::store::Store;

/// How far back a reply looks for its conversation by subject.
pub const SUBJECT_WINDOW: i64 = 45 * 86400;

/// The subject without reply and forward prefixes (in several languages, also `Re[2]:` and
/// `[Liste]` tags in front of them), in lower case with single spaces; and whether it had such
/// a prefix.
pub fn base_subject(subject: &str) -> (String, bool) {
    let mut s = subject.trim().to_lowercase();
    let mut prefixed = false;
    loop {
        let before = s.clone();
        // A mailing list tag in front: „[team] re: …“.
        if s.starts_with('[')
            && let Some(end) = s.find(']')
            && s[end + 1..].trim_start().contains(':')
        {
            let rest = s[end + 1..].trim_start().to_string();
            if strip_prefix(&rest).is_some() {
                s = rest;
            }
        }
        if let Some(rest) = strip_prefix(&s) {
            s = rest.trim_start().to_string();
            prefixed = true;
        }
        if s == before {
            break;
        }
    }
    (s.split_whitespace().collect::<Vec<_>>().join(" "), prefixed)
}

fn strip_prefix(s: &str) -> Option<&str> {
    const PREFIXES: &[&str] =
        &["re", "aw", "antw", "wg", "fwd", "fw", "sv", "vs", "tr", "rif", "r", "ref", "odp", "doorst", "wtr"];
    let (head, rest) = s.split_once(':')?;
    let word = head.trim();
    // `re[2]`, `re(2)`.
    let word = word.split(['[', '(']).next().unwrap_or(word).trim();
    PREFIXES.contains(&word).then_some(rest)
}

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
    if threads.is_empty() && store.subject_threads() {
        threads.extend(by_subject(store, account_id, id, p)?);
    }
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

/// The conversation of a reply without reply headers, by its subject (see the module comment).
fn by_subject(store: &Store, account_id: i64, id: i64, p: &ParsedMessage) -> Result<Option<i64>> {
    let (base, prefixed) = base_subject(&p.subject);
    if !prefixed || base.chars().count() < 4 {
        return Ok(None);
    }
    let conn = store.conn();
    let date = p.date.unwrap_or_else(crate::store::now);
    let mut st = conn.prepare_cached(
        "SELECT thread_id FROM messages WHERE account_id = ?1 AND base_subject = ?2 AND id <> ?3 AND thread_id <> 0
           AND COALESCE(date, internal_date) BETWEEN ?4 AND ?5
         ORDER BY COALESCE(date, internal_date) DESC LIMIT 1",
    )?;
    let hit = st
        .query_map(params![account_id, base, id, date - SUBJECT_WINDOW, date + 86400], |r| r.get::<_, i64>(0))?
        .next()
        .transpose()?;
    Ok(hit)
}

#[cfg(test)]
mod tests {
    use super::base_subject;
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
    fn base_subjects() {
        assert_eq!(base_subject("Re: AW: Fwd:  Angebot  Mai"), ("angebot mai".into(), true));
        assert_eq!(base_subject("RE[2]: Plan"), ("plan".into(), true));
        assert_eq!(base_subject("[team] Re: Plan"), ("plan".into(), true));
        assert_eq!(base_subject("Rechnung"), ("rechnung".into(), false));
        assert_eq!(base_subject("Rechnung: März"), ("rechnung: märz".into(), false));
    }

    /// A reply without headers joins by subject; a plain subject never does; the setting
    /// switches it off.
    #[test]
    fn replies_without_headers_join_by_subject() {
        let (s, _, inbox) = store_with_inbox();
        let add = |uid: u32, id: &str, subject: &str| {
            let raw = sample(id, subject, None, "x");
            s.insert_message(inbox, &header(uid, &raw), &parse(&raw).unwrap()).unwrap()
        };
        let root = add(1, "root@x", "Angebot Mai");
        let reply = add(2, "reply@x", "AW: Angebot Mai");
        let other = add(3, "other@x", "Angebot Mai");
        let thread_of = |id: i64| s.summaries(&[id]).unwrap()[0].thread_id;
        assert_eq!(thread_of(root), thread_of(reply));
        assert_ne!(thread_of(other), thread_of(root), "only replies join by subject");
        s.set_subject_threads(false);
        let off = add(4, "off@x", "Re: Angebot Mai");
        assert_ne!(thread_of(off), thread_of(root));
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
