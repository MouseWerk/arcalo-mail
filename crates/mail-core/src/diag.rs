//! The diagnostics package: a log of what the app did with its connections (kept in memory,
//! the last [`CAPACITY`] lines) and a report for a support request (Hilfe → „Diagnose
//! exportieren“). Nothing of the mail goes in: no subjects, no text, no addresses, no
//! passwords or tokens. [`redact`] removes what looks like an address, a token or a query
//! string from every line, also from server answers; the user can read the file before
//! sending it.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;

use crate::account::{Auth, Transport};
use crate::error::Result;
use crate::store::Store;

pub const CAPACITY: usize = 2000;

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    /// Unix seconds.
    pub at: i64,
    pub area: String,
    pub text: String,
}

static LOG: Mutex<VecDeque<Line>> = Mutex::new(VecDeque::new());

/// Notes a line (redacted).
pub fn log(area: &str, text: &str) {
    let line = Line { at: crate::store::now(), area: area.to_string(), text: redact(text) };
    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if log.len() >= CAPACITY {
        log.pop_front();
    }
    log.push_back(line);
}

pub fn lines() -> Vec<Line> {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
}

/// Removes addresses (`<address>`), tokens and long Base64 or hex runs (`<secret>`) and query
/// strings of URLs from a text.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&redact_word(word));
    }
    out
}

fn redact_word(word: &str) -> String {
    let trimmed = word.trim_matches(|c: char| "<>()[]\"',;".contains(c));
    if trimmed.is_empty() {
        return word.to_string();
    }
    let replace = |with: &str| word.replace(trimmed, with);
    if let Some((before, _)) = trimmed.split_once('?')
        && trimmed.contains("://")
    {
        return replace(&format!("{before}?<query>"));
    }
    let at = trimmed.find('@');
    if let Some(i) = at
        && i > 0
        && trimmed[i + 1..].contains('.')
        && !trimmed.contains("://")
    {
        return replace("<address>");
    }
    let tokenish = trimmed.len() >= 24
        && trimmed.chars().all(|c| c.is_ascii_alphanumeric() || "+/=-_.".contains(c))
        && trimmed.chars().any(|c| c.is_ascii_digit())
        && trimmed.chars().any(|c| c.is_ascii_alphabetic());
    if tokenish {
        return replace("<secret>");
    }
    word.to_string()
}

#[derive(Serialize)]
struct AccountReport {
    id: i64,
    provider: String,
    transport: serde_json::Value,
    folders: usize,
    messages: u32,
}

/// The report: the app, the settings, the accounts' server settings (no addresses, no user
/// names), the offline queue and the log.
pub fn report(store: &Store, app: &serde_json::Value) -> Result<String> {
    let settings = crate::settings::load(store)?;
    let mut accounts = vec![];
    for a in store.accounts()? {
        let mailboxes = store.mailboxes(a.id)?;
        let messages = mailboxes.iter().map(|m| m.total).sum();
        let transport = match &a.transport {
            Transport::Imap { imap, smtp, auth, .. } => serde_json::json!({
                "kind": "imap",
                "imap": format!("{}:{} {:?}{}", imap.host, imap.port, imap.security, if imap.trusted_cert.is_some() { " (trusted certificate)" } else { "" }),
                "smtp": format!("{}:{} {:?}{}", smtp.host, smtp.port, smtp.security, if smtp.trusted_cert.is_some() { " (trusted certificate)" } else { "" }),
                "auth": auth_name(auth),
            }),
            Transport::Jmap { session_url, auth, .. } => serde_json::json!({
                "kind": "jmap", "session": redact(session_url), "auth": auth_name(auth),
            }),
            Transport::Graph => serde_json::json!({ "kind": "graph" }),
            Transport::Pop3 { pop3, smtp, auth, leave_on_server, delete_after_days, .. } => serde_json::json!({
                "kind": "pop3",
                "pop3": format!("{}:{} {:?}", pop3.host, pop3.port, pop3.security),
                "smtp": format!("{}:{} {:?}", smtp.host, smtp.port, smtp.security),
                "auth": auth_name(auth),
                "leave_on_server": leave_on_server,
                "delete_after_days": delete_after_days,
            }),
        };
        accounts.push(AccountReport {
            id: a.id,
            provider: format!("{:?}", a.provider).to_lowercase(),
            transport,
            folders: mailboxes.len(),
            messages,
        });
    }
    let queue: Vec<serde_json::Value> = crate::outbox::entries(store)?
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "account": e.account_id, "kind": e.kind, "state": e.state, "attempts": e.attempts,
                "error": e.last_error.as_deref().map(redact),
            })
        })
        .collect();
    let mut out = String::new();
    out.push_str("Arcalo Mail diagnostics\n=======================\n\n");
    out.push_str(&serde_json::to_string_pretty(&serde_json::json!({
        "app": app,
        "schema": store.schema_version()?,
        "settings": settings,
        "accounts": accounts,
        "outbox": queue,
    }))?);
    out.push_str("\n\nLog (newest last)\n-----------------\n");
    for l in lines() {
        out.push_str(&format!("{} [{}] {}\n", l.at, l.area, l.text));
    }
    Ok(out)
}

fn auth_name(a: &Auth) -> &'static str {
    match a {
        Auth::Password => "password",
        Auth::OAuth2(_) => "oauth2",
        Auth::Token => "token",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_addresses_tokens_and_queries() {
        let t = redact(
            "NO [AUTHENTICATIONFAILED] maria@example.org token eyJ0eXAiOiJKV1QiLCJhbGciOi12345 https://api.example.org/jmap?access_token=abc imap.gmx.net:993",
        );
        assert!(!t.contains("maria@example.org") && t.contains("<address>"), "{t}");
        assert!(!t.contains("eyJ0eXAi") && t.contains("<secret>"), "{t}");
        assert!(t.contains("https://api.example.org/jmap?<query>") && !t.contains("abc"), "{t}");
        assert!(t.contains("imap.gmx.net:993") && t.contains("[AUTHENTICATIONFAILED]"), "{t}");
    }

    #[test]
    fn the_report_has_servers_but_no_people() {
        let (s, _, _) = crate::store::tests::store_with_inbox();
        log("test", "signed in as maria@example.org");
        let r = report(&s, &serde_json::json!({ "version": "1.0.0" })).unwrap();
        assert!(r.contains("imap.example.org:993"), "{r}");
        assert!(!r.contains("maria@example.org"), "{r}");
        assert!(r.contains("signed in as <address>"));
    }
}
