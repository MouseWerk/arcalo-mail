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

/// The new messages the model may judge for „Wichtig“ (they go to the account's provider):
/// mail of people or other senders in the inbox, not newsletters, notifications or automatic
/// mail the heuristics recognized, not mail the Screener holds back or turned away, and not
/// mail the user decided about.
pub fn for_model(store: &Store, account_id: i64, ids: &[i64]) -> Result<Vec<i64>> {
    let mut out = vec![];
    for id in triage::personal(store, account_id, ids)? {
        if decided_by(store, id)? == Some(By::User) {
            continue;
        }
        if score(store, id)?.0 <= -10 {
            continue;
        }
        out.push(id);
    }
    Ok(out)
}

/// Messages the model judges one after another before the queue looks again.
pub const MODEL_BATCH: usize = 5;
/// Messages per account the model judges in an hour; the rest waits for the next one.
pub const MODEL_PER_HOUR: usize = 60;

/// New mail waiting for the model's judgement: every message once, in order, in batches, within
/// each account's hourly budget.
#[derive(Debug, Default)]
pub struct ModelQueue {
    waiting: std::collections::VecDeque<(i64, i64)>,
    /// When each account's recent messages were taken.
    used: std::collections::HashMap<i64, std::collections::VecDeque<i64>>,
}

impl ModelQueue {
    /// Adds messages of an account (ones already waiting are not added again).
    pub fn push(&mut self, account_id: i64, ids: &[i64]) {
        for &id in ids {
            if !self.waiting.contains(&(account_id, id)) {
                self.waiting.push_back((account_id, id));
            }
        }
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    fn room(&mut self, account_id: i64, now: i64) -> bool {
        let used = self.used.entry(account_id).or_default();
        while used.front().is_some_and(|t| t + 3600 <= now) {
            used.pop_front();
        }
        used.len() < MODEL_PER_HOUR
    }

    /// The next messages to judge at `now` (account, message), at most [`MODEL_BATCH`]; empty
    /// when nothing waits or every waiting account used its budget.
    pub fn next_batch(&mut self, now: i64) -> Vec<(i64, i64)> {
        let mut out = vec![];
        let mut i = 0;
        while i < self.waiting.len() && out.len() < MODEL_BATCH {
            let (account, id) = self.waiting[i];
            if self.room(account, now) {
                self.waiting.remove(i);
                self.used.entry(account).or_default().push_back(now);
                out.push((account, id));
            } else {
                i += 1;
            }
        }
        out
    }

    /// When the budget lets the next waiting message go (`None`: nothing waits).
    pub fn next_free(&self, now: i64) -> Option<i64> {
        self.waiting
            .iter()
            .map(|(account, _)| {
                let used = self.used.get(account);
                match used {
                    Some(u) if u.len() >= MODEL_PER_HOUR => u[u.len() - MODEL_PER_HOUR] + 3600,
                    _ => now,
                }
            })
            .min()
    }
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

    #[test]
    fn the_model_sees_mail_of_people_only() {
        let (s, account, inbox) = store_with_inbox();
        s.touch_contact(&Address { name: Some("Lea".into()), email: "lea@example.org".into() }, true, 1).unwrap();
        let lea = put(&s, inbox, 1, "From: Lea <lea@example.org>\r\nTo: maria@example.org", "Vertrag", "Bis morgen?");
        let tim = put(&s, inbox, 2, "From: Tim <tim@example.com>\r\nTo: maria@example.org", "Hallo", "Grüße");
        let news = put(
            &s,
            inbox,
            3,
            "From: Shop <news@shop.example>\r\nTo: maria@example.org\r\nList-Unsubscribe: <https://shop.example/u>",
            "Dringend: nur heute",
            "Jetzt kaufen?",
        );
        let robot = put(
            &s,
            inbox,
            4,
            "From: Paket <noreply@post.example>\r\nTo: maria@example.org\r\nAuto-Submitted: auto-generated",
            "Ihr Paket kommt",
            "Heute zwischen 10 und 12 Uhr.",
        );
        let spam = put(&s, inbox, 5, "From: Werbung <spam@example.net>\r\nTo: maria@example.org", "Hallo", "?");
        let decided = put(&s, inbox, 6, "From: Jonas <jonas@example.org>\r\nTo: maria@example.org", "Frage", "?");
        set(&s, decided, false, By::User).unwrap();
        crate::triage::block_sender(&s, "spam@example.net").unwrap();
        let all = [lea, tim, news, robot, spam, decided];
        assert_eq!(for_model(&s, account, &all).unwrap(), [lea, tim]);
        // With the Screener on, a first-time sender waits in „Neue Absender“: not sent anywhere.
        let mut st = crate::settings::load(&s).unwrap();
        st.screener = true;
        st.screener_since = Some(0);
        crate::settings::save(&s, &st).unwrap();
        assert_eq!(for_model(&s, account, &all).unwrap(), [lea]);
    }

    #[test]
    fn every_new_message_is_judged_in_batches_within_the_budget() {
        let mut q = ModelQueue::default();
        let ids: Vec<i64> = (1..=20).collect();
        q.push(1, &ids);
        q.push(1, &ids[..5]);
        let mut done = vec![];
        loop {
            let batch = q.next_batch(1_000);
            if batch.is_empty() {
                break;
            }
            assert!(batch.len() <= MODEL_BATCH);
            done.extend(batch.into_iter().map(|(_, id)| id));
        }
        assert_eq!(done, ids, "all of them, each once, in order");
        // More than the hour allows: the rest waits for the next hour, nothing is dropped.
        let many: Vec<i64> = (100..100 + MODEL_PER_HOUR as i64 + 7).collect();
        q.push(2, &many);
        let mut n = 0;
        while !q.next_batch(2_000).is_empty() {
            n += 1;
            assert!(n < 1000);
        }
        assert_eq!(q.waiting(), 7, "the budget of the hour is used");
        assert_eq!(q.next_free(2_000), Some(2_000 + 3600), "the hour of the first ones ends");
        // Another account has its own budget.
        q.push(3, &[1]);
        assert_eq!(q.next_batch(2_000), [(3, 1)]);
        let later: usize =
            std::iter::from_fn(|| Some(q.next_batch(2_000 + 3600)).filter(|b| !b.is_empty())).map(|b| b.len()).sum();
        assert_eq!(later, 7);
        assert_eq!(q.waiting(), 0);
    }
}
