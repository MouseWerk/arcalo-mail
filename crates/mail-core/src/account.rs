//! Accounts: who the user is at a provider and how the app reaches the mailbox.
//!
//! The first version knows four kinds ([`Provider`]): any IMAP/SMTP server with a password,
//! Microsoft 365 / Outlook.com and Gmail / Google Workspace with OAuth 2.0 (IMAP and SMTP with
//! XOAUTH2; Microsoft Graph as the other transport for Microsoft), and JMAP. The account row
//! holds the server settings only; passwords and tokens live in the OS credential store under
//! [`secret_key`], never in the database or the settings.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::{tr, trf};

/// The service name of every credential store entry of the app.
pub const CREDENTIAL_SERVICE: &str = "de.mousewerk.arcalo.mail";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Any IMAP/SMTP server.
    Imap,
    /// Microsoft 365 and Outlook.com.
    Microsoft,
    /// Gmail and Google Workspace.
    Google,
    Jmap,
}

/// How a connection is secured. There is no plain-text mode for the internet:
/// [`Security::Plain`] is accepted for loopback hosts only (local bridges, tests), see
/// [`crate::network::check_server`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    Tls,
    /// Plain connection upgraded with STARTTLS before any login (SMTP 587, IMAP 143).
    StartTls,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

impl Server {
    pub fn new(host: &str, port: u16, security: Security) -> Self {
        Server { host: host.to_string(), port, security }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthProvider {
    Microsoft,
    Google,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "provider")]
pub enum Auth {
    /// A password (or an app password) from the credential store.
    Password,
    /// OAuth 2.0: the refresh token is in the credential store, access tokens only in memory.
    OAuth2(OAuthProvider),
}

/// How the mailbox is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Transport {
    /// IMAP for reading, SMTP for sending (also Microsoft and Google with XOAUTH2).
    Imap { imap: Server, smtp: Server, username: String, auth: Auth },
    /// Microsoft Graph (mail and send over HTTPS), Microsoft accounts only.
    Graph,
    /// A JMAP session resource (RFC 8620), `https://` only.
    Jmap { session_url: String, username: String, auth: Auth },
}

/// An account as stored (`accounts` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub provider: Provider,
    /// The name shown in the sidebar („Arbeit“, „Privat“); the address when empty.
    pub name: String,
    pub email: String,
    /// The sender name of new messages.
    pub sender_name: String,
    pub transport: Transport,
}

/// A new account before it is stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountDraft {
    pub provider: Provider,
    pub name: String,
    pub email: String,
    pub sender_name: String,
    pub transport: Transport,
}

/// What a credential store entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    Password,
    RefreshToken,
}

/// The credential store account name of a secret of an account (service [`CREDENTIAL_SERVICE`]).
pub fn secret_key(account_id: i64, kind: SecretKind) -> String {
    let what = match kind {
        SecretKind::Password => "password",
        SecretKind::RefreshToken => "oauth-refresh-token",
    };
    format!("account-{account_id}-{what}")
}

/// The domain of an address (`a@example.org` → `example.org`), lower case.
pub fn domain_of(email: &str) -> Option<String> {
    let (local, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.trim().to_ascii_lowercase();
    let ok = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then_some(domain)
}

/// The usual settings for an address: fixed servers for Microsoft and Google, a guess from the
/// domain for IMAP (`imap.<domain>` / `smtp.<domain>`; autoconfig and SRV lookup are milestone 1
/// work, docs/plan.md) and the well-known JMAP resource.
pub fn preset(provider: Provider, email: &str) -> Result<Transport> {
    let domain = domain_of(email).ok_or_else(|| invalid_address(email))?;
    let username = email.trim().to_string();
    Ok(match provider {
        Provider::Microsoft => Transport::Imap {
            imap: Server::new("outlook.office365.com", 993, Security::Tls),
            smtp: Server::new("smtp.office365.com", 587, Security::StartTls),
            username,
            auth: Auth::OAuth2(OAuthProvider::Microsoft),
        },
        Provider::Google => Transport::Imap {
            imap: Server::new("imap.gmail.com", 993, Security::Tls),
            smtp: Server::new("smtp.gmail.com", 465, Security::Tls),
            username,
            auth: Auth::OAuth2(OAuthProvider::Google),
        },
        Provider::Imap => Transport::Imap {
            imap: Server::new(&format!("imap.{domain}"), 993, Security::Tls),
            smtp: Server::new(&format!("smtp.{domain}"), 465, Security::Tls),
            username,
            auth: Auth::Password,
        },
        Provider::Jmap => Transport::Jmap {
            session_url: format!("https://{domain}/.well-known/jmap"),
            username,
            auth: Auth::Password,
        },
    })
}

fn invalid_address(email: &str) -> Error {
    Error::Invalid(trf!(
        "„{email}“ ist keine gültige E-Mail-Adresse.",
        "“{email}” is not a valid e-mail address.",
        email = email.trim()
    ))
}

/// Checks a draft before it is stored: the address, the servers and that the transport fits
/// the provider.
pub fn validate(draft: &AccountDraft) -> Result<()> {
    domain_of(&draft.email).ok_or_else(|| invalid_address(&draft.email))?;
    match (&draft.provider, &draft.transport) {
        (_, Transport::Imap { imap, smtp, username, auth }) => {
            if username.trim().is_empty() {
                return Err(Error::Invalid(tr!("Der Benutzername fehlt.", "The user name is missing.").into()));
            }
            crate::network::check_server(imap)?;
            crate::network::check_server(smtp)?;
            let fits = match draft.provider {
                Provider::Imap => *auth == Auth::Password,
                Provider::Microsoft => *auth == Auth::OAuth2(OAuthProvider::Microsoft),
                Provider::Google => *auth == Auth::OAuth2(OAuthProvider::Google),
                Provider::Jmap => false,
            };
            if !fits {
                return Err(mismatch());
            }
        }
        (Provider::Microsoft, Transport::Graph) => {}
        (Provider::Jmap, Transport::Jmap { session_url, .. }) => crate::network::check_https_url(session_url)?,
        _ => return Err(mismatch()),
    }
    Ok(())
}

fn mismatch() -> Error {
    Error::Invalid(
        tr!(
            "Diese Verbindungsart passt nicht zum Anbieter des Kontos.",
            "This connection type does not fit the account's provider."
        )
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(provider: Provider, email: &str) -> AccountDraft {
        AccountDraft {
            provider,
            name: String::new(),
            email: email.into(),
            sender_name: "Maria Muster".into(),
            transport: preset(provider, email).unwrap(),
        }
    }

    #[test]
    fn presets_are_valid() {
        for p in [Provider::Imap, Provider::Microsoft, Provider::Google, Provider::Jmap] {
            validate(&draft(p, "maria@example.org")).unwrap();
        }
        let Transport::Imap { imap, smtp, .. } = preset(Provider::Imap, "Maria@Example.ORG").unwrap() else {
            panic!("imap expected")
        };
        assert_eq!(imap.host, "imap.example.org");
        assert_eq!(smtp.port, 465);
    }

    #[test]
    fn bad_addresses_are_refused() {
        for bad in ["", "maria", "@example.org", "maria@localhost", "maria@exa mple.org", "maria@.org"] {
            assert!(domain_of(bad).is_none(), "{bad}");
            assert!(preset(Provider::Imap, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn transport_must_fit_the_provider() {
        let mut d = draft(Provider::Google, "maria@gmail.com");
        d.transport = preset(Provider::Imap, "maria@gmail.com").unwrap();
        assert!(validate(&d).is_err(), "Google with a password");
        d.transport = Transport::Graph;
        assert!(validate(&d).is_err(), "Graph is Microsoft only");
        let mut m = draft(Provider::Microsoft, "maria@outlook.com");
        m.transport = Transport::Graph;
        validate(&m).unwrap();
    }

    #[test]
    fn plain_text_to_the_internet_is_refused() {
        let mut d = draft(Provider::Imap, "maria@example.org");
        if let Transport::Imap { imap, .. } = &mut d.transport {
            imap.security = Security::Plain;
        }
        assert!(validate(&d).is_err());
    }

    #[test]
    fn no_secret_is_part_of_the_stored_account() {
        let json = serde_json::to_string(&draft(Provider::Imap, "maria@example.org")).unwrap();
        for word in ["\"password\":", "token\":", "secret\":"] {
            assert!(!json.to_lowercase().contains(word), "{word} in {json}");
        }
        assert_eq!(secret_key(7, SecretKind::Password), "account-7-password");
        assert_eq!(secret_key(7, SecretKind::RefreshToken), "account-7-oauth-refresh-token");
    }
}
