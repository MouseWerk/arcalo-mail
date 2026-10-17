//! „Frag dein Postfach“: a question in plain words, answered from the mail on this device. The
//! local full-text index finds the messages (any of the question's words, best matches first,
//! only in the accounts the chosen provider may see); their sender, date, subject and the start
//! of their text go to the model, numbered, and the answer refers to them as `[n]`, which the
//! UI turns into links to the messages.

use rusqlite::params_from_iter;
use serde::Serialize;

use super::prompts::MailText;
use crate::error::Result;
use crate::search::fold;
use crate::store::Store;

/// Messages handed to the model, and the characters of each one's text.
pub const SOURCES: usize = 8;
pub const SOURCE_CHARS: usize = 900;

/// A message an answer can refer to as `[n]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    pub n: u32,
    pub message_id: i64,
    pub subject: String,
    pub from: String,
    pub date: i64,
}

const STOPWORDS: &[&str] = &[
    // German
    "aber",
    "alle",
    "allem",
    "als",
    "also",
    "am",
    "an",
    "auch",
    "auf",
    "aus",
    "bei",
    "bin",
    "bis",
    "bitte",
    "das",
    "dass",
    "dem",
    "den",
    "der",
    "des",
    "die",
    "dies",
    "diese",
    "dir",
    "doch",
    "du",
    "ein",
    "eine",
    "einem",
    "einen",
    "einer",
    "es",
    "für",
    "gibt",
    "hab",
    "habe",
    "hat",
    "hatte",
    "ich",
    "ihr",
    "im",
    "in",
    "ist",
    "ja",
    "kann",
    "mal",
    "man",
    "mein",
    "meine",
    "mich",
    "mir",
    "mit",
    "noch",
    "nur",
    "oder",
    "schon",
    "sein",
    "sich",
    "sie",
    "sind",
    "so",
    "über",
    "um",
    "und",
    "uns",
    "von",
    "vor",
    "war",
    "was",
    "wann",
    "warum",
    "welche",
    "welcher",
    "wer",
    "wie",
    "wieder",
    "wir",
    "wo",
    "wurde",
    "zu",
    "zum",
    "zur",
    "letzte",
    "letzten",
    "geschickt",
    "geschrieben",
    "mails",
    "mail",
    "nachricht",
    // English
    "a",
    "about",
    "all",
    "an",
    "and",
    "any",
    "are",
    "at",
    "be",
    "did",
    "do",
    "does",
    "for",
    "from",
    "has",
    "have",
    "how",
    "i",
    "in",
    "is",
    "it",
    "me",
    "my",
    "of",
    "on",
    "or",
    "please",
    "the",
    "to",
    "was",
    "what",
    "when",
    "where",
    "which",
    "who",
    "why",
    "with",
    "you",
    "last",
    "sent",
    "email",
    "emails",
    "message",
];

/// The searchable words of a question.
pub fn keywords(question: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for w in question.split(|c: char| !c.is_alphanumeric()) {
        let w = fold(&w.to_lowercase());
        if w.chars().count() < 3 || STOPWORDS.contains(&w.as_str()) || out.contains(&w) {
            continue;
        }
        out.push(w);
    }
    out.truncate(12);
    out
}

/// The messages of `accounts` that best match the question, with their text for the prompt.
pub fn retrieve(
    store: &Store,
    question: &str,
    accounts: &[i64],
    date: impl Fn(i64) -> String,
) -> Result<Vec<(Source, MailText)>> {
    let words = keywords(question);
    if words.is_empty() || accounts.is_empty() {
        return Ok(vec![]);
    }
    let query = words.iter().map(|w| format!("\"{w}\"*")).collect::<Vec<_>>().join(" OR ");
    let marks = accounts.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    // Best matches first (bm25 weighs subject and people above the text), the newer of two equal
    // ones first; one copy per Message-ID; never drafts, trash or spam.
    let sql = format!(
        "SELECT m.id, m.subject, COALESCE(NULLIF(m.from_name, ''), m.from_email, ''), COALESCE(m.date, m.internal_date),
           b.text, b.html, m.snippet
         FROM messages_fts f JOIN messages m ON m.id = f.rowid JOIN mailboxes x ON x.id = m.mailbox_id
         LEFT JOIN message_bodies b ON b.message_id = m.id
         WHERE messages_fts MATCH ? AND m.hidden = 0 AND m.account_id IN ({marks})
           AND COALESCE(x.role, '') NOT IN ('drafts', 'trash', 'junk', 'all', 'flagged', 'important')
         ORDER BY bm25(messages_fts, 4.0, 2.0, 1.0) + (strftime('%s', 'now') - COALESCE(m.date, m.internal_date)) / 31536000.0, m.id DESC
         LIMIT 40"
    );
    let mut args: Vec<rusqlite::types::Value> = vec![query.into()];
    args.extend(accounts.iter().map(|a| rusqlite::types::Value::from(*a)));
    let mut st = store.conn().prepare(&sql)?;
    type Row = (i64, String, String, i64, Option<String>, Option<String>, String);
    let rows: Vec<Row> = st
        .query_map(params_from_iter(args), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut out: Vec<(Source, MailText)> = vec![];
    let mut seen: Vec<(String, String, i64)> = vec![];
    for (id, subject, from, at, text, html, snippet) in rows {
        // The same message in two folders.
        let k = (subject.clone(), from.clone(), at);
        if seen.contains(&k) {
            continue;
        }
        seen.push(k);
        let body = super::plain_text(text.as_deref(), html.as_deref(), SOURCE_CHARS);
        let n = out.len() as u32 + 1;
        out.push((
            Source { n, message_id: id, subject: subject.clone(), from: from.clone(), date: at },
            MailText { from, date: date(at), subject, text: if body.is_empty() { snippet } else { body } },
        ));
        if out.len() >= SOURCES {
            break;
        }
    }
    Ok(out)
}

/// The numbers an answer refers to (`[2]`, `[1, 3]`), in order, each once.
pub fn cited(answer: &str) -> Vec<u32> {
    let mut out = vec![];
    let mut rest = answer;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(']') else { break };
        for part in after[..end].split(',') {
            if let Ok(n) = part.trim().parse::<u32>()
                && !out.contains(&n)
            {
                out.push(n);
            }
        }
        rest = &after[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::store::tests::{header, store_with_inbox};

    fn put(s: &Store, inbox: i64, uid: u32, from: &str, subject: &str, body: &str) -> i64 {
        let raw = format!(
            "From: {from}\r\nTo: maria@example.org\r\nSubject: {subject}\r\nMessage-ID: <a{uid}@example.org>\r\nDate: Fri, 16 Oct 2026 09:{uid:02}:00 +0000\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        );
        let p = parse(raw.as_bytes()).unwrap();
        let id = s.insert_message(inbox, &header(uid, raw.as_bytes()), &p).unwrap();
        s.set_body(id, &p).unwrap();
        id
    }

    #[test]
    fn finds_the_messages_of_the_allowed_accounts_only() {
        let (s, account, inbox) = store_with_inbox();
        let offer = put(
            &s,
            inbox,
            1,
            "Lea Weber <lea@example.org>",
            "Angebot Website",
            "Das Angebot für die Website kommt am Dienstag, 4.800 Euro.",
        );
        put(&s, inbox, 2, "Jonas <jonas@example.org>", "Mittagessen", "Wer kommt mit zum Italiener?");
        let other = s
            .add_account(&crate::account::AccountDraft {
                provider: crate::account::Provider::Imap,
                name: "Privat".into(),
                email: "m@example.net".into(),
                sender_name: String::new(),
                transport: crate::account::preset(crate::account::Provider::Imap, "m@example.net").unwrap(),
            })
            .unwrap();
        let other_inbox = s
            .upsert_mailbox(
                other,
                &crate::imap::MailboxEntry { name: "INBOX".into(), delimiter: None, attributes: vec![] },
            )
            .unwrap();
        put(&s, other_inbox, 1, "Bank <bank@example.net>", "Angebot Kredit", "Ihr persönliches Angebot.");
        assert_eq!(keywords("Wann ist das Angebot für die Website da?"), ["angebot", "website"]);
        let hits = retrieve(&s, "Wann ist das Angebot für die Website da?", &[account], |_| "16.10.".into()).unwrap();
        assert_eq!(hits.len(), 1, "the other account's offer stays out: {hits:?}");
        assert_eq!(hits[0].0.message_id, offer);
        assert_eq!(hits[0].0.n, 1);
        assert!(hits[0].1.text.contains("4.800 Euro"));
        assert_eq!(hits[0].1.from, "Lea Weber");
        let both = retrieve(&s, "Angebot", &[account, other], |_| String::new()).unwrap();
        assert_eq!(both.len(), 2);
        assert!(retrieve(&s, "und oder", &[account], |_| String::new()).unwrap().is_empty());
    }

    #[test]
    fn citations() {
        assert_eq!(cited("Am Dienstag [1]. Der Preis steht in [2, 3] und [1]."), [1, 2, 3]);
        assert!(cited("Keine Quelle").is_empty());
    }
}
