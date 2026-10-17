//! Full-text search over the local store (FTS5, `messages_fts`): works offline, and finds what
//! was synced. Server-side search for older mail is milestone 1 work (docs/plan.md).
//!
//! The input is plain words, as typed: each word must appear (a prefix is enough: „angeb“
//! finds „Angebot“), `"…"` keeps a phrase together, and `from:`, `subject:` limit a word to
//! the sender and recipients or the subject. FTS5's own syntax is never passed through, so no
//! input is a syntax error.

use rusqlite::params;

use crate::error::Result;
use crate::store::{MessageSummary, Store};

/// What the index and the query both see: ß and ẞ as ss (unicode61 removes diacritics, but
/// ß is a letter of its own there), so „Grüße“, „grüsse“ and „GRÜSSE“ find each other.
pub(crate) fn fold(s: &str) -> String {
    if s.contains(['ß', 'ẞ']) { s.replace(['ß', 'ẞ'], "ss") } else { s.to_string() }
}

/// The FTS5 query for the user's input, `None` when nothing searchable is left.
pub fn fts_query(input: &str) -> Option<String> {
    let mut terms = vec![];
    let mut rest = input.trim();
    while !rest.is_empty() {
        let (column, after) = match rest.split_once(':') {
            Some(("from" | "von" | "to" | "an", after)) if !after.starts_with(' ') => (Some("people"), after),
            Some(("subject" | "betreff", after)) if !after.starts_with(' ') => (Some("subject"), after),
            _ => (None, rest),
        };
        let (word, phrase, next) = if let Some(quoted) = after.strip_prefix('"') {
            let end = quoted.find('"').unwrap_or(quoted.len());
            (&quoted[..end], true, quoted.get(end + 1..).unwrap_or_default())
        } else {
            let end = after.find(char::is_whitespace).unwrap_or(after.len());
            (&after[..end], false, &after[end..])
        };
        // Only letters and digits reach FTS5 (unicode61 splits on everything else anyway).
        let tokens: Vec<String> = word
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| fold(&t.to_lowercase()))
            .collect();
        if !tokens.is_empty() {
            let body = if phrase || tokens.len() > 1 {
                format!("\"{}\"", tokens.join(" "))
            } else {
                format!("\"{}\"*", tokens[0])
            };
            terms.push(match column {
                Some(c) => format!("{c}:{body}"),
                None => body,
            });
        }
        rest = next.trim_start();
    }
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

/// Messages matching `input`, newest first as in a folder (every word must match, so the
/// newest hits are the ones people look for), optionally in one account.
pub fn search(store: &Store, input: &str, account_id: Option<i64>, limit: u32) -> Result<Vec<MessageSummary>> {
    let Some(q) = fts_query(input) else { return Ok(vec![]) };
    let conn = store.conn();
    let mut st = conn.prepare_cached(
        "SELECT f.rowid FROM messages_fts f JOIN messages m ON m.id = f.rowid JOIN mailboxes b ON b.id = m.mailbox_id
         WHERE messages_fts MATCH ?1 AND (?2 IS NULL OR m.account_id = ?2)
           -- A copy in Gmail's All Mail, Starred or Important only when there is no other.
           AND NOT (COALESCE(b.role, '') IN ('all', 'flagged', 'important') AND m.message_id IS NOT NULL
             AND m.gm_msgid IS NULL
             AND EXISTS (SELECT 1 FROM messages o JOIN mailboxes ob ON ob.id = o.mailbox_id
               WHERE o.account_id = m.account_id AND o.message_id = m.message_id AND o.id <> m.id AND o.hidden = 0
                 AND COALESCE(ob.role, '') NOT IN ('all', 'flagged', 'important')))
           -- Gmail's copies of one message (its labels, All Mail) by X-GM-MSGID: one hit, the
           -- inbox's, else a label's, else All Mail's.
           AND NOT (m.gm_msgid IS NOT NULL
             AND EXISTS (SELECT 1 FROM messages o JOIN mailboxes ob ON ob.id = o.mailbox_id
               WHERE o.account_id = m.account_id AND o.gm_msgid = m.gm_msgid AND o.id <> m.id AND o.hidden = 0
                 AND (CASE WHEN ob.role = 'inbox' THEN 0 WHEN COALESCE(ob.role, '') IN ('all', 'flagged', 'important')
                        THEN 2 ELSE 1 END, o.id)
                   < (CASE WHEN b.role = 'inbox' THEN 0 WHEN COALESCE(b.role, '') IN ('all', 'flagged', 'important')
                        THEN 2 ELSE 1 END, m.id)))
         ORDER BY m.internal_date DESC, m.uid DESC, m.id DESC
         LIMIT ?3",
    )?;
    let ids =
        st.query_map(params![q, account_id, limit], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    store.summaries(&ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse, tests::sample};
    use crate::store::tests::{header, store_with_inbox};

    #[test]
    fn queries() {
        assert_eq!(fts_query("angebot").as_deref(), Some("\"angebot\"*"));
        assert_eq!(fts_query("  Angebot  Oktober ").as_deref(), Some("\"angebot\"* AND \"oktober\"*"));
        assert_eq!(fts_query("\"neues Angebot\"").as_deref(), Some("\"neues angebot\""));
        assert_eq!(fts_query("from:maria").as_deref(), Some("people:\"maria\"*"));
        assert_eq!(fts_query("betreff:rechnung").as_deref(), Some("subject:\"rechnung\"*"));
        assert_eq!(fts_query("maria@example.org").as_deref(), Some("\"maria example org\""));
        // FTS5 syntax is never passed through.
        assert_eq!(fts_query("NOT OR ( * \"").as_deref(), Some("\"not\"* AND \"or\"*"));
        assert_eq!(fts_query("  ").as_deref(), None);
        assert_eq!(fts_query("-- ** ").as_deref(), None);
    }

    #[test]
    fn finds_by_subject_people_and_text_without_accents() {
        let (s, account, inbox) = store_with_inbox();
        let msgs = [
            ("a@x", "Angebot Oktober", "Viele Grüße aus Köln"),
            ("b@x", "Rechnung 42", "Bitte bis Freitag überweisen"),
            ("c@x", "Mittagessen", "Wer kommt mit?"),
        ];
        for (uid, (id, subject, body)) in msgs.iter().enumerate() {
            let raw = sample(id, subject, None, body);
            let p = parse(&raw).unwrap();
            let mid = s.insert_message(inbox, &header(uid as u32 + 1, &raw), &p).unwrap();
            s.set_body(mid, &p).unwrap();
        }
        let subjects = |q: &str| search(&s, q, None, 10).unwrap().into_iter().map(|m| m.subject).collect::<Vec<_>>();
        assert_eq!(subjects("angeb"), vec!["Angebot Oktober"]);
        assert_eq!(subjects("grusse koln"), vec!["Angebot Oktober"], "diacritics are ignored, ß is ss");
        assert_eq!(subjects("uberweisen"), vec!["Rechnung 42"]);
        assert_eq!(subjects("from:maria").len(), 3);
        assert!(subjects("subject:maria").is_empty());
        assert!(subjects("nichtda").is_empty());
        assert_eq!(search(&s, "rechnung", Some(account + 1), 10).unwrap().len(), 0, "other account");
        s.remove_uids(inbox, &[2]).unwrap();
        assert!(subjects("rechnung").is_empty(), "the index follows removals");
    }
}
