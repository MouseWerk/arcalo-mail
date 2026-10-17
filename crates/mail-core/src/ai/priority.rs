//! Priority detection („Wichtig“). Local heuristics decide for every new message of an inbox,
//! on this device and without a model: a person (not a newsletter or a machine) whom the user
//! wrote to before, written to the user directly, with words of urgency or a deadline. When the
//! account has an AI provider, the model may decide again ([`super::assistant`]); a decision
//! of the user („Nicht wichtig“) is never overruled.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::{MessageSummary, Store};
use crate::triage::{self, Category};

/// Who decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    Rules,
    Model,
    User,
}

impl By {
    fn as_str(self) -> &'static str {
        match self {
            By::Rules => "rules",
            By::Model => "model",
            By::User => "user",
        }
    }
}

/// The score at which a message counts as important.
pub const THRESHOLD: i32 = 4;

const URGENT: &[&str] = &[
    "dringend",
    "eilig",
    "asap",
    "urgent",
    "wichtig",
    "important",
    "frist",
    "deadline",
    "bis morgen",
    "bis heute",
    "heute noch",
    "by tomorrow",
    "by today",
    "as soon as possible",
    "so schnell wie möglich",
    "bitte bestätige",
    "bitte bestätigen",
    "please confirm",
    "rückmeldung bis",
    "reply by",
];

/// From, subject, To, Cc, text and the account's address of a message.
type Facts = (Option<String>, String, String, String, Option<String>, String);

/// The heuristics' score of a message and why.
pub fn score(store: &Store, id: i64) -> Result<(i32, Vec<&'static str>)> {
    let row: Option<Facts> = store
        .conn()
        .query_row(
            "SELECT m.from_email, m.subject, m.to_json, m.cc_json, b.text, a.email
             FROM messages m JOIN accounts a ON a.id = m.account_id LEFT JOIN message_bodies b ON b.message_id = m.id
             WHERE m.id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let Some((from, subject, to_json, _cc_json, text, me)) = row else { return Ok((0, vec![])) };
    let mut s = 0;
    let mut why = vec![];
    match triage::category_of(store, id)? {
        Category::People => {
            s += 1;
            why.push("person");
        }
        Category::Other => {}
        _ => return Ok((-10, vec!["automatic"])),
    }
    if let Some(from) = from.as_deref() {
        if from.eq_ignore_ascii_case(&me) {
            return Ok((-10, vec!["own"]));
        }
        let wrote: Option<i64> = store
            .conn()
            .query_row("SELECT sent FROM contacts WHERE email = ?1 COLLATE NOCASE", [from], |r| r.get(0))
            .optional()?;
        if wrote.unwrap_or(0) > 0 {
            s += 2;
            why.push("contact");
        }
    }
    let to: Vec<crate::parse::Address> = serde_json::from_str(&to_json).unwrap_or_default();
    if to.iter().any(|a| a.email.eq_ignore_ascii_case(&me)) {
        s += 1;
        why.push("to_me");
        if to.len() <= 3 {
            s += 1;
            why.push("few_recipients");
        }
    }
    let text = text.unwrap_or_default();
    let head: String = text.chars().take(1500).collect();
    let hay = format!("{} {}", subject, head).to_lowercase();
    if URGENT.iter().any(|w| hay.contains(w)) {
        s += 2;
        why.push("urgent");
    }
    if head.contains('?') {
        s += 1;
        why.push("question");
    }
    Ok((s, why))
}

/// Sets the decision.
pub fn set(store: &Store, id: i64, important: bool, by: By) -> Result<()> {
    store.conn().execute(
        "UPDATE messages SET important = ?2, important_by = ?3 WHERE id = ?1",
        params![id, important, by.as_str()],
    )?;
    Ok(())
}

/// Who decided about a message, if anyone did.
pub fn decided_by(store: &Store, id: i64) -> Result<Option<By>> {
    let by: Option<Option<String>> =
        store.conn().query_row("SELECT important_by FROM messages WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    Ok(match by.flatten().as_deref() {
        Some("rules") => Some(By::Rules),
        Some("model") => Some(By::Model),
        Some("user") => Some(By::User),
        _ => None,
    })
}

/// Decides for new messages of an inbox that nobody decided for yet; returns the important ones.
pub fn mark_new(store: &Store, ids: &[i64]) -> Result<Vec<i64>> {
    let mut out = vec![];
    for &id in ids {
        if decided_by(store, id)?.is_some() {
            continue;
        }
        let (s, _) = score(store, id)?;
        set(store, id, s >= THRESHOLD, By::Rules)?;
        if s >= THRESHOLD {
            out.push(id);
        }
    }
    Ok(out)
}

/// Fills `important` in rows of a list.
pub fn annotate(store: &Store, rows: &mut [MessageSummary]) -> Result<()> {
    let mut st = store.conn().prepare_cached("SELECT COALESCE(important, 0) FROM messages WHERE id = ?1")?;
    for r in rows.iter_mut() {
        r.important = st.query_row([r.id], |x| x.get(0)).optional()?.unwrap_or(false);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Address, parse};
    use crate::store::tests::{header, store_with_inbox};

    fn put(s: &Store, inbox: i64, uid: u32, head: &str, subject: &str, body: &str) -> i64 {
        let raw = format!(
            "{head}\r\nSubject: {subject}\r\nMessage-ID: <p{uid}@example.org>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        );
        let p = parse(raw.as_bytes()).unwrap();
        let id = s.insert_message(inbox, &header(uid, raw.as_bytes()), &p).unwrap();
        s.set_body(id, &p).unwrap();
        id
    }

    #[test]
    fn people_who_need_something_soon_are_important_newsletters_never() {
        let (s, _, inbox) = store_with_inbox();
        s.touch_contact(&Address { name: Some("Lea".into()), email: "lea@example.org".into() }, true, 1).unwrap();
        let lea = put(
            &s,
            inbox,
            1,
            "From: Lea <lea@example.org>\r\nTo: maria@example.org",
            "Vertrag",
            "Kannst du den Vertrag bitte bis morgen unterschreiben?",
        );
        let stranger = put(
            &s,
            inbox,
            2,
            "From: Tim <tim@example.com>\r\nTo: maria@example.org",
            "Hallo",
            "Schöne Grüße aus Berlin.",
        );
        let news = put(
            &s,
            inbox,
            3,
            "From: Shop <news@shop.example>\r\nTo: maria@example.org\r\nList-Unsubscribe: <https://shop.example/u>",
            "Dringend: nur heute 50 %",
            "Wichtig! Jetzt kaufen?",
        );
        let important = mark_new(&s, &[lea, stranger, news]).unwrap();
        assert_eq!(important, [lea]);
        let (score_lea, why) = score(&s, lea).unwrap();
        assert!(score_lea >= THRESHOLD && why.contains(&"urgent") && why.contains(&"contact"), "{why:?}");
        // The user's own decision stays.
        set(&s, lea, false, By::User).unwrap();
        assert!(mark_new(&s, &[lea]).unwrap().is_empty());
        assert_eq!(decided_by(&s, lea).unwrap(), Some(By::User));
        let mut rows = s.summaries(&[lea, news]).unwrap();
        set(&s, news, true, By::Model).unwrap();
        annotate(&s, &mut rows).unwrap();
        assert_eq!(rows.iter().map(|r| r.important).collect::<Vec<_>>(), [false, true]);
    }
}
