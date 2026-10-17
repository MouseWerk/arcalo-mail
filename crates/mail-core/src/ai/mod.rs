//! The AI assistant, local first, built the way Arcalo's AI engine is (`crates/arcalo-core/src/ai`
//! there): providers speaking the OpenAI chat protocol ([`provider`], [`client`]), streamed
//! answers that can be cancelled, and a privacy rule that decides where a text may go
//! ([`AiSettings::for_account`]).
//!
//! The assistant is off until the user sets up a provider. A provider on this computer (Ollama,
//! LM Studio, any OpenAI-compatible server on loopback) may see the mail of every account that
//! has no choice of its own; a cloud provider (OpenAI, Anthropic, Mistral, Azure OpenAI, any
//! compatible API) only the accounts the user chose it for. What goes out, per feature, is
//! listed in README.md („Privacy“).
//!
//! The features ([`assistant`]): a thread's summary, reply drafts in the user's tone (learned on
//! this device from sent mail, [`tone`]), writing help in the compose sheet, translation,
//! questions to the mailbox answered from the local search index ([`ask`]), priority
//! („Wichtig“, local heuristics first, [`priority`]) and tasks and dates ([`extract`]). Prompts
//! and answers are in the user's language ([`prompts`]).

pub mod ask;
pub mod assistant;
pub mod client;
pub mod extract;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
pub mod priority;
pub mod prompts;
pub mod provider;
pub mod tone;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use client::{AiClient, Answer, ChatMessage};
pub use provider::{AiProvider, ProviderKind};

use crate::error::{Error, Result};
use crate::trf;

/// The account choice that lets no provider see the account's mail.
pub const OFF: &str = "off";

/// Settings → KI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// The providers the user set up; none: the assistant is off.
    pub providers: Vec<AiProvider>,
    /// The provider of accounts without a choice of their own. Only a provider on this computer
    /// is used that way; a cloud provider needs the account's own choice.
    pub default_provider: String,
    /// Per account (its id as text): the provider that may see its mail, or [`OFF`].
    pub accounts: BTreeMap<String, String>,
    /// Long conversations get their summary when opened.
    pub auto_summary: bool,
    /// New mail is checked for „Wichtig“ (the local heuristics always; the model when the
    /// account has a provider).
    pub priority: bool,
    /// Reply drafts follow the user's tone, learned on this device from sent mail.
    pub tone: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            providers: vec![],
            default_provider: String::new(),
            accounts: BTreeMap::new(),
            auto_summary: true,
            priority: true,
            tone: true,
        }
    }
}

/// Where a text of an account goes: the line „Bleibt auf diesem Gerät (Ollama)“ or „Geht an
/// api.openai.com (OpenAI)“ next to every AI button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Destination {
    /// `None`: no provider may see this account's mail.
    pub provider: Option<String>,
    pub name: String,
    pub host: String,
    pub local: bool,
}

impl AiSettings {
    /// The assistant is set up.
    pub fn on(&self) -> bool {
        !self.providers.is_empty()
    }

    pub fn provider(&self, id: &str) -> Option<&AiProvider> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// The provider that may see the mail of `account_id`: the account's own choice, else the
    /// default provider when it runs on this computer. Mail never goes to a cloud provider the
    /// user did not choose for the account.
    pub fn for_account(&self, account_id: i64) -> Option<&AiProvider> {
        match self.accounts.get(&account_id.to_string()).map(String::as_str) {
            Some(OFF) => None,
            Some(id) if !id.is_empty() => self.provider(id),
            _ => self
                .provider(&self.default_provider)
                .or_else(|| self.providers.iter().find(|p| p.local()))
                .filter(|p| p.local()),
        }
    }

    pub fn destination(&self, account_id: i64) -> Destination {
        match self.for_account(account_id) {
            Some(p) => Destination {
                provider: Some(p.id.clone()),
                name: p.display_name().to_string(),
                host: p.host(),
                local: p.local(),
            },
            None => Destination { provider: None, name: String::new(), host: String::new(), local: true },
        }
    }

    /// The provider for `account_id`, or why there is none.
    pub fn require(&self, account_id: i64, account_name: &str) -> Result<AiProvider> {
        if !self.on() {
            return Err(Error::Invalid(trf!(
                "Der KI-Assistent ist aus. Richte in den Einstellungen unter KI einen Anbieter ein, zum Beispiel Ollama auf diesem Gerät.",
                "The AI assistant is off. Set up a provider in Settings under AI, for example Ollama on this device."
            )));
        }
        self.for_account(account_id).cloned().ok_or_else(|| {
            Error::Invalid(trf!(
                "Für das Konto „{account_name}“ ist kein KI-Anbieter erlaubt. Wähle in den Einstellungen unter KI, wer seine Mails sehen darf.",
                "No AI provider may see the mail of the account “{account_name}”. Choose one in Settings under AI."
            ))
        })
    }

    /// Checks and tidies the providers and drops choices of providers that are gone.
    pub fn normalize(&mut self) -> Result<()> {
        self.providers = provider::normalize(std::mem::take(&mut self.providers))?;
        let ids: Vec<String> = self.providers.iter().map(|p| p.id.clone()).collect();
        if !ids.contains(&self.default_provider) {
            self.default_provider = self.providers.iter().find(|p| p.local()).map(|p| p.id.clone()).unwrap_or_default();
        }
        self.accounts.retain(|_, v| v == OFF || ids.contains(v));
        Ok(())
    }
}

/// `YYYY-MM-DD` and the weekday (0 Monday) of a Unix time in UTC shifted by `offset` seconds.
pub fn civil(secs: i64) -> (i64, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    let weekday = (days + 3).rem_euclid(7) as u32;
    (y, m, d, weekday)
}

/// Days since 1970-01-01 of a date (proleptic Gregorian).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The plain text of a message for a prompt: the text part (or the HTML as text), without the
/// quoted earlier messages and the signature, at most `max` characters.
pub fn plain_text(text: Option<&str>, html: Option<&str>, max: usize) -> String {
    let source = match (text.filter(|t| !t.trim().is_empty()), html) {
        (Some(t), _) => t.to_string(),
        (None, Some(h)) => crate::compose::html_to_text(h),
        (None, None) => String::new(),
    };
    let mut out: Vec<&str> = vec![];
    for line in source.lines() {
        let l = line.trim_end();
        let lower = l.trim().to_lowercase();
        // Where the quote of the earlier message starts.
        let quote_head = (lower.starts_with("am ") && lower.contains(" schrieb") && lower.ends_with(':'))
            || (lower.starts_with("on ") && lower.contains(" wrote") && lower.ends_with(':'))
            || lower.starts_with("-----original message")
            || lower.starts_with("-----ursprüngliche nachricht");
        if quote_head || l == "-- " || l == "--" {
            break;
        }
        if l.trim_start().starts_with('>') {
            continue;
        }
        if l.trim().is_empty() && out.last().is_some_and(|p| p.trim().is_empty()) {
            continue;
        }
        out.push(l);
    }
    let joined = out.join("\n").trim().to_string();
    if joined.chars().count() <= max {
        joined
    } else {
        let mut s: String = joined.chars().take(max).collect();
        s.push('…');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> AiSettings {
        let mut s = AiSettings {
            providers: vec![
                AiProvider::new(ProviderKind::Ollama),
                AiProvider { model: "gpt-4o-mini".into(), ..AiProvider::new(ProviderKind::Openai) },
            ],
            ..Default::default()
        };
        s.normalize().unwrap();
        s
    }

    #[test]
    fn mail_goes_to_a_cloud_provider_only_when_chosen_for_the_account() {
        let mut s = settings();
        assert_eq!(s.default_provider, "ollama", "the local provider is the default");
        assert_eq!(s.for_account(1).unwrap().id, "ollama");
        assert!(s.destination(1).local);
        // The default set to the cloud provider does not open any account's mail to it.
        s.default_provider = "openai".into();
        assert!(s.for_account(1).is_none());
        s.accounts.insert("2".into(), "openai".into());
        let d = s.destination(2);
        assert_eq!((d.name.as_str(), d.host.as_str(), d.local), ("OpenAI", "api.openai.com", false));
        s.accounts.insert("3".into(), OFF.into());
        s.default_provider = "ollama".into();
        assert!(s.for_account(3).is_none());
        assert!(s.require(3, "Privat").unwrap_err().to_string().contains("“Privat”"));
        // A provider that is removed takes its choices along.
        s.providers.retain(|p| p.id != "openai");
        s.normalize().unwrap();
        assert!(!s.accounts.contains_key("2"));
        assert!(AiSettings::default().require(1, "x").is_err(), "off until a provider is set up");
    }

    #[test]
    fn dates() {
        assert_eq!(civil(1_792_195_200), (2026, 10, 17, 5), "a Saturday");
        assert_eq!(days_from_civil(2026, 10, 17) * 86400, 1_792_195_200);
        assert_eq!(civil(0), (1970, 1, 1, 3));
    }

    #[test]
    fn plain_text_leaves_quotes_and_signatures_out() {
        let t = "Hallo Maria,\n\npasst Montag?\n\n\n\nGruß Jonas\n-- \nJonas Berger\nTel. 123\n";
        assert_eq!(plain_text(Some(t), None, 500), "Hallo Maria,\n\npasst Montag?\n\nGruß Jonas");
        let q = "Ja, passt.\n\nAm 16.10.2026 um 09:30 schrieb Jonas:\n> passt Montag?";
        assert_eq!(plain_text(Some(q), None, 500), "Ja, passt.");
        assert_eq!(plain_text(None, Some("<p>Hi <b>there</b></p>"), 500), "Hi there");
        assert_eq!(plain_text(Some("abcdef"), None, 3), "abc…");
    }
}
