//! Finding the servers of an address for the account setup, as Thunderbird does:
//!
//! 1. the built-in presets of common providers ([`preset_for`]: GMX, WEB.DE, T-Online,
//!    mailbox.org, Posteo, iCloud, Gmail, Yahoo, freenet),
//! 2. the provider's own autoconfig file (`https://autoconfig.<domain>/mail/config-v1.1.xml`,
//!    `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`),
//! 3. Mozilla's provider database (ISPDB, `https://autoconfig.thunderbird.net/v1.1/<domain>`),
//! 4. SRV records (RFC 6186: `_imaps._tcp`, `_submissions._tcp`, …),
//! 5. the MX records, for domains hosted at a known provider (IONOS, Strato, Google, …),
//! 6. a guess (`imap.<domain>`, `smtp.<domain>`).
//!
//! Everything goes through [`crate::network`] (HTTPS only, the system's resolver). The result
//! is a suggestion; the connection check (`crate::engine::check_account`) decides.

use serde::{Deserialize, Serialize};

use crate::account::{Security, Server, domain_of};
use crate::error::{Error, Result};
use crate::network::{self, Service};

/// Where the settings came from (shown in the setup: „Gefunden über …“).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Preset,
    Autoconfig,
    Ispdb,
    Srv,
    Mx,
    Guess,
}

/// What the user has to know before the password works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hint {
    /// An app password instead of the normal one (iCloud, Gmail, Yahoo).
    AppPassword,
    /// IMAP has to be switched on in the provider's web settings first (GMX, WEB.DE, freenet).
    EnableImap,
    /// T-Online: the separate e-mail password, not the login password.
    MailPassword,
    /// Microsoft accounts sign in with Microsoft (OAuth, coming in a later version).
    MicrosoftSignIn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovered {
    pub imap: Server,
    pub smtp: Server,
    pub username: String,
    pub source: Source,
    /// The provider's name („GMX“, „Posteo“), when known.
    pub provider: Option<String>,
    pub hint: Option<Hint>,
}

struct Preset {
    name: &'static str,
    domains: &'static [&'static str],
    /// Mail exchangers that give a hosted domain away (suffixes).
    mx: &'static [&'static str],
    imap: (&'static str, u16, Security),
    smtp: (&'static str, u16, Security),
    hint: Option<Hint>,
}

const PRESETS: &[Preset] = &[
    Preset {
        name: "GMX",
        domains: &["gmx.de", "gmx.net", "gmx.at", "gmx.ch", "gmx.eu", "gmx.li"],
        mx: &["gmx.net"],
        imap: ("imap.gmx.net", 993, Security::Tls),
        smtp: ("mail.gmx.net", 587, Security::StartTls),
        hint: Some(Hint::EnableImap),
    },
    Preset {
        name: "GMX",
        domains: &["gmx.com", "gmx.us", "gmx.co.uk", "gmx.fr", "gmx.es"],
        mx: &["gmx.com"],
        imap: ("imap.gmx.com", 993, Security::Tls),
        smtp: ("mail.gmx.com", 587, Security::StartTls),
        hint: Some(Hint::EnableImap),
    },
    Preset {
        name: "WEB.DE",
        domains: &["web.de"],
        mx: &["web.de"],
        imap: ("imap.web.de", 993, Security::Tls),
        smtp: ("smtp.web.de", 587, Security::StartTls),
        hint: Some(Hint::EnableImap),
    },
    Preset {
        name: "T-Online",
        domains: &["t-online.de", "magenta.de"],
        mx: &["t-online.de"],
        imap: ("secureimap.t-online.de", 993, Security::Tls),
        smtp: ("securesmtp.t-online.de", 465, Security::Tls),
        hint: Some(Hint::MailPassword),
    },
    Preset {
        name: "mailbox.org",
        domains: &["mailbox.org"],
        mx: &["mailbox.org"],
        imap: ("imap.mailbox.org", 993, Security::Tls),
        smtp: ("smtp.mailbox.org", 465, Security::Tls),
        hint: None,
    },
    Preset {
        name: "Posteo",
        domains: &[
            "posteo.de",
            "posteo.net",
            "posteo.at",
            "posteo.ch",
            "posteo.eu",
            "posteo.org",
            "posteo.me",
            "posteo.us",
            "posteo.uk",
            "posteo.be",
            "posteo.dk",
            "posteo.es",
            "posteo.fi",
            "posteo.fr",
            "posteo.it",
            "posteo.nl",
            "posteo.pt",
            "posteo.se",
            "posteo.com",
            "posteo.cl",
            "posteo.co",
            "posteo.ca",
            "posteo.ie",
            "posteo.jp",
        ],
        mx: &["posteo.de"],
        imap: ("posteo.de", 993, Security::Tls),
        smtp: ("posteo.de", 465, Security::Tls),
        hint: None,
    },
    Preset {
        name: "iCloud",
        domains: &["icloud.com", "me.com", "mac.com"],
        mx: &["mail.icloud.com"],
        imap: ("imap.mail.me.com", 993, Security::Tls),
        smtp: ("smtp.mail.me.com", 587, Security::StartTls),
        hint: Some(Hint::AppPassword),
    },
    Preset {
        name: "IONOS",
        domains: &["ionos.de", "online.de", "onlinehome.de", "1und1.de"],
        mx: &["ionos.de", "ionos.com", "kundenserver.de", "1and1.com", "1und1.de"],
        imap: ("imap.ionos.de", 993, Security::Tls),
        smtp: ("smtp.ionos.de", 465, Security::Tls),
        hint: None,
    },
    Preset {
        name: "STRATO",
        domains: &["strato.de"],
        mx: &["rzone.de"],
        imap: ("imap.strato.de", 993, Security::Tls),
        smtp: ("smtp.strato.de", 465, Security::Tls),
        hint: None,
    },
    Preset {
        name: "Gmail",
        domains: &["gmail.com", "googlemail.com"],
        mx: &["google.com", "googlemail.com"],
        imap: ("imap.gmail.com", 993, Security::Tls),
        smtp: ("smtp.gmail.com", 465, Security::Tls),
        hint: Some(Hint::AppPassword),
    },
    Preset {
        name: "Yahoo",
        domains: &["yahoo.com", "yahoo.de", "ymail.com", "rocketmail.com"],
        mx: &["yahoodns.net"],
        imap: ("imap.mail.yahoo.com", 993, Security::Tls),
        smtp: ("smtp.mail.yahoo.com", 465, Security::Tls),
        hint: Some(Hint::AppPassword),
    },
    Preset {
        name: "freenet",
        domains: &["freenet.de"],
        mx: &["freenet.de"],
        imap: ("mx.freenet.de", 993, Security::Tls),
        smtp: ("mx.freenet.de", 587, Security::StartTls),
        hint: Some(Hint::EnableImap),
    },
    Preset {
        name: "Microsoft",
        domains: &["outlook.com", "outlook.de", "hotmail.com", "hotmail.de", "live.com", "live.de", "msn.com"],
        mx: &["outlook.com"],
        imap: ("outlook.office365.com", 993, Security::Tls),
        smtp: ("smtp.office365.com", 587, Security::StartTls),
        hint: Some(Hint::MicrosoftSignIn),
    },
];

fn from_preset(p: &Preset, email: &str, source: Source) -> Discovered {
    Discovered {
        imap: Server::new(p.imap.0, p.imap.1, p.imap.2),
        smtp: Server::new(p.smtp.0, p.smtp.1, p.smtp.2),
        username: email.trim().to_string(),
        source,
        provider: Some(p.name.to_string()),
        hint: p.hint,
    }
}

/// The built-in settings of a provider's own domain.
pub fn preset_for(email: &str) -> Option<Discovered> {
    let domain = domain_of(email)?;
    PRESETS.iter().find(|p| p.domains.contains(&domain.as_str())).map(|p| from_preset(p, email, Source::Preset))
}

/// The provider whose mail exchangers a hosted domain uses.
pub fn preset_for_mx(email: &str, mx_hosts: &[String]) -> Option<Discovered> {
    let hit = mx_hosts
        .iter()
        .find_map(|h| PRESETS.iter().find(|p| p.mx.iter().any(|s| h == s || h.ends_with(&format!(".{s}")))))?;
    Some(from_preset(hit, email, Source::Mx))
}

fn fill(template: &str, email: &str) -> String {
    let (local, domain) = email.trim().rsplit_once('@').unwrap_or((email, ""));
    template.replace("%EMAILADDRESS%", email.trim()).replace("%EMAILLOCALPART%", local).replace("%EMAILDOMAIN%", domain)
}

/// Reads a Mozilla autoconfig file (`clientConfig` 1.1): the first IMAP and SMTP server with
/// TLS or STARTTLS and a password login (plain text only for a server on this computer, as the
/// network policy allows).
pub fn parse_config(xml: &str, email: &str, source: Source) -> Option<Discovered> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let provider = doc.descendants().find(|n| n.has_tag_name("emailProvider"))?;
    let text = |n: roxmltree::Node<'_, '_>, tag: &str| {
        n.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).map(|t| t.trim().to_string())
    };
    let server = |kind: &str, tag: &str| -> Option<(Server, String)> {
        provider.children().filter(|n| n.has_tag_name(tag) && n.attribute("type") == Some(kind)).find_map(|n| {
            let host = fill(&text(n, "hostname")?, email);
            let security = match text(n, "socketType")?.to_ascii_uppercase().as_str() {
                "SSL" | "TLS" => Security::Tls,
                "STARTTLS" => Security::StartTls,
                "PLAIN" if network::is_loopback(&host) => Security::Plain,
                _ => return None,
            };
            let password = n.children().filter(|c| c.has_tag_name("authentication")).any(|c| {
                matches!(c.text().map(str::trim), Some("password-cleartext" | "plain" | "password-encrypted"))
            });
            if !password && n.children().any(|c| c.has_tag_name("authentication")) {
                return None;
            }
            let port = text(n, "port")?.parse().ok()?;
            let user = fill(&text(n, "username").unwrap_or_else(|| "%EMAILADDRESS%".into()), email);
            Some((Server::new(&host, port, security), user))
        })
    };
    let (imap, username) = server("imap", "incomingServer")?;
    let (smtp, _) = server("smtp", "outgoingServer")?;
    Some(Discovered {
        imap,
        smtp,
        username,
        source,
        provider: text(provider, "displayShortName").or_else(|| text(provider, "displayName")),
        hint: None,
    })
}

/// Where the lookups go; the tests point them at a server on this computer.
#[derive(Debug, Clone)]
pub struct Lookup {
    /// Autoconfig URLs with `{domain}` and `{email}`, in order.
    pub autoconfig: Vec<String>,
    pub ispdb: String,
    /// Whether DNS (SRV, MX) is asked.
    pub dns: bool,
}

impl Default for Lookup {
    fn default() -> Self {
        Lookup {
            autoconfig: vec![
                "https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={email}".into(),
                "https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml?emailaddress={email}".into(),
            ],
            ispdb: "https://autoconfig.thunderbird.net/v1.1/{domain}".into(),
            dns: true,
        }
    }
}

async fn fetch(client: &network::HttpClient, url: &str) -> Option<String> {
    network::check_https_url(url).ok()?;
    let r = client.get(url).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let body = r.text().await.ok()?;
    body.contains("clientConfig").then_some(body)
}

async fn srv_pair(domain: &str) -> Option<(Server, Server)> {
    let first = |list: Vec<network::SrvTarget>| list.into_iter().next();
    let imap = match first(network::srv(&format!("_imaps._tcp.{domain}")).await.ok()?) {
        Some(t) => Server::new(&t.host, t.port, Security::Tls),
        None => {
            let t = first(network::srv(&format!("_imap._tcp.{domain}")).await.ok()?)?;
            Server::new(&t.host, t.port, if t.port == 993 { Security::Tls } else { Security::StartTls })
        }
    };
    let smtp = match first(network::srv(&format!("_submissions._tcp.{domain}")).await.ok()?) {
        Some(t) => Server::new(&t.host, t.port, Security::Tls),
        None => {
            let t = first(network::srv(&format!("_submission._tcp.{domain}")).await.ok()?)?;
            Server::new(&t.host, t.port, if t.port == 465 { Security::Tls } else { Security::StartTls })
        }
    };
    Some((imap, smtp))
}

/// Finds the servers of an address (see the module comment for the order).
pub async fn discover(email: &str, lookup: &Lookup) -> Result<Discovered> {
    let domain = domain_of(email).ok_or_else(|| {
        Error::Invalid(crate::trf!(
            "„{email}“ ist keine gültige E-Mail-Adresse.",
            "“{email}” is not a valid e-mail address.",
            email = email.trim()
        ))
    })?;
    if let Some(p) = preset_for(email) {
        return Ok(p);
    }
    let client = network::client_for(Service::Autoconfig)?;
    let url = |t: &str| t.replace("{domain}", &domain).replace("{email}", email.trim());
    // The lookups run side by side; the first source in the order above wins.
    let auto = async {
        for t in &lookup.autoconfig {
            if let Some(d) = fetch(&client, &url(t)).await.and_then(|x| parse_config(&x, email, Source::Autoconfig)) {
                return Some(d);
            }
        }
        None
    };
    let ispdb =
        async { fetch(&client, &url(&lookup.ispdb)).await.and_then(|x| parse_config(&x, email, Source::Ispdb)) };
    let srv = async { if lookup.dns { srv_pair(&domain).await } else { None } };
    let mx = async { if lookup.dns { network::mx(&domain).await.unwrap_or_default() } else { vec![] } };
    let (auto, ispdb, srv, mx) = tokio::join!(auto, ispdb, srv, mx);
    if let Some(d) = auto.or(ispdb) {
        return Ok(d);
    }
    if let Some((imap, smtp)) = srv {
        return Ok(Discovered {
            imap,
            smtp,
            username: email.trim().into(),
            source: Source::Srv,
            provider: None,
            hint: None,
        });
    }
    if let Some(d) = preset_for_mx(email, &mx) {
        return Ok(d);
    }
    Ok(Discovered {
        imap: Server::new(&format!("imap.{domain}"), 993, Security::Tls),
        smtp: Server::new(&format!("smtp.{domain}"), 587, Security::StartTls),
        username: email.trim().into(),
        source: Source::Guess,
        provider: None,
        hint: None,
    })
}

/// Whether the provider files sent mail in Sent itself (Gmail, Microsoft): then the app does
/// not add a second copy.
pub fn server_files_sent(smtp: &Server) -> bool {
    let h = smtp.host.to_ascii_lowercase();
    ["gmail.com", "googlemail.com", "office365.com", "outlook.com"]
        .iter()
        .any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GMX_LIKE: &str = r#"<?xml version="1.0"?>
<clientConfig version="1.1">
  <emailProvider id="example.net">
    <domain>example.net</domain>
    <displayName>Example Mail</displayName>
    <displayShortName>Example</displayShortName>
    <incomingServer type="pop3">
      <hostname>pop.example.net</hostname><port>995</port><socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.example.net</hostname><port>143</port><socketType>plain</socketType>
      <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.example.net</hostname><port>993</port><socketType>SSL</socketType>
      <username>%EMAILLOCALPART%</username><authentication>OAuth2</authentication>
      <authentication>password-cleartext</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.%EMAILDOMAIN%</hostname><port>587</port><socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn reads_autoconfig_files() {
        let d = parse_config(GMX_LIKE, "maria@example.net", Source::Ispdb).unwrap();
        assert_eq!(d.imap, Server::new("imap.example.net", 993, Security::Tls), "no plain text, no POP");
        assert_eq!(d.username, "maria");
        assert_eq!(d.smtp, Server::new("smtp.example.net", 587, Security::StartTls));
        assert_eq!(d.provider.as_deref(), Some("Example"));
        assert!(parse_config("<html>nope</html>", "a@b.de", Source::Ispdb).is_none());
        // Plain text only for a server on this computer.
        let local = GMX_LIKE.replace("imap.example.net</hostname><port>143", "127.0.0.1</hostname><port>143");
        let d = parse_config(&local, "maria@example.net", Source::Autoconfig).unwrap();
        assert_eq!(d.imap, Server::new("127.0.0.1", 143, Security::Plain));
    }

    #[test]
    fn presets_of_german_providers() {
        let gmx = preset_for("Maria@GMX.de").unwrap();
        assert_eq!((gmx.imap.host.as_str(), gmx.smtp.port, gmx.hint), ("imap.gmx.net", 587, Some(Hint::EnableImap)));
        assert_eq!(preset_for("a@web.de").unwrap().smtp.host, "smtp.web.de");
        assert_eq!(preset_for("a@t-online.de").unwrap().hint, Some(Hint::MailPassword));
        assert_eq!(preset_for("a@posteo.net").unwrap().imap.host, "posteo.de");
        assert_eq!(preset_for("a@mailbox.org").unwrap().smtp, Server::new("smtp.mailbox.org", 465, Security::Tls));
        assert_eq!(preset_for("a@me.com").unwrap().hint, Some(Hint::AppPassword));
        assert!(preset_for("a@example.org").is_none());
        let ionos = preset_for_mx("info@baeckerei-example.de", &["mx00.ionos.de".into()]).unwrap();
        assert_eq!((ionos.imap.host.as_str(), ionos.source), ("imap.ionos.de", Source::Mx));
        assert_eq!(preset_for_mx("a@b.de", &["smtpin.rzone.de".into()]).unwrap().provider.as_deref(), Some("STRATO"));
        assert!(preset_for_mx("a@b.de", &["mail.example.org".into()]).is_none());
        assert!(server_files_sent(&Server::new("smtp.gmail.com", 465, Security::Tls)));
        assert!(!server_files_sent(&Server::new("smtp.web.de", 587, Security::StartTls)));
    }

    /// The provider's autoconfig file wins over the database; without both, a guess.
    #[tokio::test]
    async fn discovers_over_http_in_order() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (status, body) = if req.starts_with("GET /ispdb/example.net") {
                    ("200 OK", GMX_LIKE.to_string())
                } else {
                    ("404 Not Found", String::new())
                };
                let resp =
                    format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        let lookup = Lookup {
            autoconfig: vec![format!("http://127.0.0.1:{port}/auto/{{domain}}")],
            ispdb: format!("http://127.0.0.1:{port}/ispdb/{{domain}}"),
            dns: false,
        };
        let d = discover("maria@example.net", &lookup).await.unwrap();
        assert_eq!((d.source, d.imap.host.as_str()), (Source::Ispdb, "imap.example.net"));
        let g = discover("maria@nirgendwo.example", &lookup).await.unwrap();
        assert_eq!((g.source, g.imap.host.as_str()), (Source::Guess, "imap.nirgendwo.example"));
        assert_eq!(discover("a@gmx.de", &lookup).await.unwrap().source, Source::Preset);
        assert!(discover("kein-at", &lookup).await.is_err());
    }
}
