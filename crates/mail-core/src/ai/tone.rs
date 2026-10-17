//! The user's tone, learned on this device from the account's sent mail: the usual greeting and
//! closing, „du“ or „Sie“, the length of sentences and a few short samples. It is kept in the
//! store (`settings`, key `ai_tone:<account>`) and goes into a reply prompt only to the provider
//! that may see the account's mail.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::{Store, now};

/// Sent messages looked at.
const SAMPLE_MESSAGES: u32 = 40;
/// Samples kept for the prompt, and their length.
const SAMPLES: usize = 3;
const SAMPLE_CHARS: usize = 280;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tone {
    pub greeting: Option<String>,
    pub closing: Option<String>,
    /// `Some(true)`: mostly „du“; `Some(false)`: mostly „Sie“.
    pub informal: Option<bool>,
    pub words_per_sentence: u32,
    pub samples: Vec<String>,
    /// How many sent messages it was learned from.
    pub learned_from: u32,
    pub learned_at: i64,
}

fn key(account_id: i64) -> String {
    format!("ai_tone:{account_id}")
}

/// The tone learned for an account, if any.
pub fn load(store: &Store, account_id: i64) -> Result<Option<Tone>> {
    Ok(store.setting(&key(account_id))?.and_then(|raw| serde_json::from_str(&raw).ok()))
}

/// Forgets what was learned.
pub fn forget(store: &Store, account_id: i64) -> Result<()> {
    store.conn().execute("DELETE FROM settings WHERE key = ?1", [key(account_id)])?;
    Ok(())
}

const GREETINGS: &[&str] = &[
    "sehr geehrte",
    "sehr geehrter",
    "liebe",
    "lieber",
    "hallo",
    "hi",
    "hey",
    "moin",
    "servus",
    "guten morgen",
    "guten tag",
    "dear",
    "hello",
    "good morning",
];

const CLOSINGS: &[&str] = &[
    "viele grüße",
    "beste grüße",
    "liebe grüße",
    "herzliche grüße",
    "mit freundlichen grüßen",
    "freundliche grüße",
    "schöne grüße",
    "grüße",
    "gruß",
    "lg",
    "vg",
    "danke",
    "best regards",
    "kind regards",
    "best",
    "regards",
    "cheers",
    "thanks",
    "thank you",
];

/// The greeting word(s) of a first line („Hallo Jonas,“ → „Hallo“).
fn greeting_of(line: &str) -> Option<String> {
    let l = line.trim();
    if l.chars().count() > 50 {
        return None;
    }
    let lower = l.to_lowercase();
    GREETINGS
        .iter()
        .find(|g| lower.starts_with(*g) && lower[g.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric()))
        .map(|g| {
            let words = g.split(' ').count();
            l.split_whitespace().take(words).collect::<Vec<_>>().join(" ").trim_end_matches(',').to_string()
        })
}

/// The closing of a line near the end („Viele Grüße,“ → „Viele Grüße“).
fn closing_of(line: &str) -> Option<String> {
    let l = line.trim().trim_end_matches([',', '!', '.']);
    let lower = l.to_lowercase();
    CLOSINGS.iter().find(|c| lower == **c || (lower.starts_with(*c) && l.chars().count() < 40)).map(|_| l.to_string())
}

fn most_common(items: &[String]) -> Option<String> {
    let mut counts: Vec<(String, u32)> = vec![];
    for i in items {
        match counts.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(i)) {
            Some((_, n)) => *n += 1,
            None => counts.push((i.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts.into_iter().next().map(|(k, _)| k)
}

/// Learns the tone from the newest sent messages of the account (the Sent folder, written from
/// the account's address); `None` when there are none with text yet.
pub fn learn(store: &Store, account_id: i64) -> Result<Option<Tone>> {
    let mut st = store.conn().prepare(
        "SELECT b.text, b.html FROM messages m JOIN mailboxes x ON x.id = m.mailbox_id
         JOIN message_bodies b ON b.message_id = m.id JOIN accounts a ON a.id = m.account_id
         WHERE m.account_id = ?1 AND x.role = 'sent' AND lower(COALESCE(m.from_email, '')) = lower(a.email)
         ORDER BY m.internal_date DESC LIMIT ?2",
    )?;
    let bodies: Vec<(Option<String>, Option<String>)> = st
        .query_map(params![account_id, SAMPLE_MESSAGES], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let texts: Vec<String> = bodies
        .iter()
        .map(|(t, h)| super::plain_text(t.as_deref(), h.as_deref(), 4000))
        .filter(|t| !t.trim().is_empty())
        .collect();
    if texts.is_empty() {
        return Ok(None);
    }
    let mut greetings = vec![];
    let mut closings = vec![];
    let (mut du, mut sie) = (0u32, 0u32);
    let (mut words, mut sentences) = (0u32, 0u32);
    let mut samples = vec![];
    for t in &texts {
        let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
        if let Some(g) = lines.first().and_then(|l| greeting_of(l)) {
            greetings.push(g);
        }
        if let Some(c) = lines.iter().rev().take(4).find_map(|l| closing_of(l)) {
            closings.push(c);
        }
        for w in t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
            match w {
                "du" | "dich" | "dir" | "dein" | "deine" | "deinen" | "deinem" | "deiner" | "Du" | "Dich" | "Dir" => {
                    du += 1
                }
                "Sie" | "Ihnen" | "Ihr" | "Ihre" | "Ihren" | "Ihrem" | "Ihrer" => sie += 1,
                _ => {}
            }
        }
        for s in t.split(['.', '!', '?']).map(str::trim).filter(|s| s.split_whitespace().count() >= 3) {
            words += s.split_whitespace().count() as u32;
            sentences += 1;
        }
        if samples.len() < SAMPLES && t.chars().count() >= 40 {
            let sample: String =
                t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(SAMPLE_CHARS).collect();
            samples.push(sample);
        }
    }
    // „Sie“ also starts sentences about third persons; it counts only when clearly ahead.
    let informal = match (du, sie) {
        (0, 0) => None,
        (d, s) if d >= s => Some(true),
        (d, s) if s > d * 2 => Some(false),
        _ => None,
    };
    let tone = Tone {
        greeting: most_common(&greetings),
        closing: most_common(&closings),
        informal,
        words_per_sentence: if sentences > 0 { (words as f32 / sentences as f32).round() as u32 } else { 0 },
        samples,
        learned_from: texts.len() as u32,
        learned_at: now(),
    };
    store.set_setting(&key(account_id), &serde_json::to_string(&tone)?)?;
    Ok(Some(tone))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::MailboxEntry;
    use crate::parse::parse;
    use crate::store::tests::{header, store_with_inbox};

    fn sent(s: &Store, mailbox: i64, uid: u32, to: &str, body: &str) {
        let raw = format!(
            "From: Maria Muster <maria@example.org>\r\nTo: {to}\r\nSubject: S{uid}\r\nMessage-ID: <s{uid}@example.org>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        );
        let p = parse(raw.as_bytes()).unwrap();
        let id = s.insert_message(mailbox, &header(uid, raw.as_bytes()), &p).unwrap();
        s.set_body(id, &p).unwrap();
    }

    #[test]
    fn learns_greeting_closing_and_du_from_sent_mail() {
        let (s, account, inbox) = store_with_inbox();
        let sent_box = s
            .upsert_mailbox(
                account,
                &MailboxEntry { name: "Sent".into(), delimiter: Some('/'), attributes: vec!["\\Sent".into()] },
            )
            .unwrap();
        assert_eq!(learn(&s, account).unwrap(), None, "nothing sent yet");
        sent(
            &s,
            sent_box,
            1,
            "lea@example.org",
            "Hallo Lea,\r\n\r\nkannst du mir die Zahlen bis Freitag schicken? Danke dir.\r\n\r\nViele Grüße\r\nMaria",
        );
        sent(
            &s,
            sent_box,
            2,
            "jonas@example.org",
            "Hallo Jonas,\r\n\r\ndein Entwurf ist gut. Ich habe zwei kleine Punkte markiert.\r\n\r\nViele Grüße\r\nMaria\r\n-- \r\nMaria Muster",
        );
        sent(
            &s,
            sent_box,
            3,
            "chef@example.org",
            "Hi Tom,\r\n\r\nalles klar, ich bin dabei.\r\n\r\nBeste Grüße,\r\nMaria",
        );
        // Mail in the inbox is not the user's own.
        sent(
            &s,
            inbox,
            9,
            "x@example.org",
            "Sehr geehrte Frau Muster, Ihre Bestellung ist da. Mit freundlichen Grüßen",
        );
        let t = learn(&s, account).unwrap().unwrap();
        assert_eq!(t.greeting.as_deref(), Some("Hallo"));
        assert_eq!(t.closing.as_deref(), Some("Viele Grüße"));
        assert_eq!(t.informal, Some(true));
        assert_eq!(t.learned_from, 3);
        assert!(t.words_per_sentence > 0);
        assert!(t.samples.iter().any(|x| x.starts_with("Hallo Lea, kannst du")), "{:?}", t.samples);
        assert!(t.samples.iter().all(|x| !x.contains("Maria Muster")), "no signature in samples");
        assert_eq!(load(&s, account).unwrap(), Some(t));
        forget(&s, account).unwrap();
        assert_eq!(load(&s, account).unwrap(), None);
    }

    #[test]
    fn greetings_and_closings() {
        assert_eq!(greeting_of("Sehr geehrte Frau Weber,").as_deref(), Some("Sehr geehrte"));
        assert_eq!(greeting_of("Hallo,").as_deref(), Some("Hallo"));
        assert_eq!(greeting_of("Hallowach bin ich"), None);
        assert_eq!(closing_of("Viele Grüße,").as_deref(), Some("Viele Grüße"));
        assert_eq!(closing_of("Best regards").as_deref(), Some("Best regards"));
        assert_eq!(closing_of("Bitte schick mir das Angebot"), None);
    }
}
