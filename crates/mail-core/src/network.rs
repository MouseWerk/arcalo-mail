//! The one place that decides how the app connects, as `network.rs` does in Arcalo.
//!
//! Every connection goes through this module: IMAP and SMTP (TCP with TLS or STARTTLS) and every
//! HTTP client (OAuth, JMAP, Microsoft Graph, the update feed). A test fails when other code
//! opens a socket or builds a client ([`tests::no_connection_is_opened_outside_network`]).
//!
//! What exists now is the policy: [`check_server`] and [`check_https_url`] refuse plain text to
//! the internet before anything is opened, and [`Service`] names who connects. The connectors
//! themselves land here in milestone 1 (`connect_imap`, `connect_smtp`: tokio-rustls with the
//! platform verifier) and milestone 2 (`client_for(Service)`: reqwest with rustls), together
//! with the proxy profiles taken over from Arcalo (docs/plan.md).

use std::net::IpAddr;

use crate::account::{Security, Server};
use crate::error::{Error, Result};
use crate::{tr, trf};

/// Who connects. The proxy profile and the timeouts will be chosen per service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Imap,
    Smtp,
    Jmap,
    /// Microsoft identity platform (login.microsoftonline.com).
    OAuthMicrosoft,
    /// Google's OAuth endpoints (accounts.google.com, oauth2.googleapis.com).
    OAuthGoogle,
    /// Microsoft Graph (graph.microsoft.com).
    Graph,
    /// The release feed of the desktop updater.
    UpdateFeed,
}

/// Whether a host name or address is this computer (`localhost`, 127.0.0.0/8, ::1).
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost") || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Checks a server before a connection: a host, a port, and TLS or STARTTLS unless the host is
/// this computer.
pub fn check_server(server: &Server) -> Result<()> {
    let host = server.host.trim();
    if host.is_empty() || host.contains(char::is_whitespace) || host.contains('/') {
        return Err(Error::Invalid(trf!(
            "„{host}“ ist kein gültiger Servername.",
            "“{host}” is not a valid server name.",
            host = host
        )));
    }
    if server.port == 0 {
        return Err(Error::Invalid(tr!("Der Port fehlt.", "The port is missing.").into()));
    }
    if server.security == Security::Plain && !is_loopback(host) {
        return Err(Error::Network(trf!(
            "Unverschlüsselte Verbindungen zu {host} sind nicht erlaubt. Wähle TLS oder STARTTLS.",
            "Unencrypted connections to {host} are not allowed. Choose TLS or STARTTLS.",
            host = host
        )));
    }
    Ok(())
}

/// Checks an HTTP address: `https://` with a host; `http://` only to this computer (tests,
/// the OAuth redirect listener).
pub fn check_https_url(url: &str) -> Result<()> {
    let u = url.trim();
    let (scheme, rest) = u.split_once("://").unwrap_or(("", u));
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host_only = if host.starts_with('[') {
        host.split_inclusive(']').next().unwrap_or(host)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let ok = !host_only.is_empty()
        && match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => is_loopback(host_only),
            _ => false,
        };
    if ok {
        Ok(())
    } else {
        Err(Error::Network(trf!(
            "„{url}“ ist keine sichere Adresse (https:// erwartet).",
            "“{url}” is not a secure address (https:// expected).",
            url = u
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_only_to_this_computer() {
        let plain = |h: &str| Server::new(h, 1143, Security::Plain);
        assert!(check_server(&plain("127.0.0.1")).is_ok());
        assert!(check_server(&plain("localhost")).is_ok());
        assert!(check_server(&plain("::1")).is_ok());
        assert!(check_server(&plain("imap.example.org")).is_err());
        assert!(check_server(&plain("10.0.0.1")).is_err());
        assert!(check_server(&Server::new("imap.example.org", 993, Security::Tls)).is_ok());
        assert!(check_server(&Server::new("imap.example.org", 0, Security::Tls)).is_err());
        assert!(check_server(&Server::new("imap example.org", 993, Security::Tls)).is_err());
        assert!(check_server(&Server::new("", 993, Security::Tls)).is_err());
    }

    #[test]
    fn https_urls() {
        for ok in [
            "https://jmap.example.org/.well-known/jmap",
            "http://127.0.0.1:8080/x",
            "http://[::1]:9/",
            "http://localhost/cb",
        ] {
            assert!(check_https_url(ok).is_ok(), "{ok}");
        }
        for bad in
            ["http://jmap.example.org/", "ftp://x.org", "https://", "jmap.example.org", "http://localhost.example.org/"]
        {
            assert!(check_https_url(bad).is_err(), "{bad}");
        }
    }

    /// Sockets and HTTP clients are made in this module only (CLAUDE.md, „Code rules“).
    #[test]
    fn no_connection_is_opened_outside_network() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = vec![];
        let mut stack = vec![root.join("src"), root.join("../../src-tauri/src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        assert!(files.len() > 10, "sources not found: {}", files.len());
        let patterns = [
            "TcpStream::connect",
            "UdpSocket::bind",
            "reqwest::Client",
            "Client::builder(",
            "ClientBuilder::new(",
            "reqwest::get(",
            "TlsConnector::from(",
            "async_imap::connect",
            "async_imap::Client::new",
            "SmtpTransport::",
            "AsyncSmtpTransport::",
        ];
        let mut stray = vec![];
        for f in &files {
            if f.ends_with("mail-core/src/network.rs") {
                continue;
            }
            let text = std::fs::read_to_string(f).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (i, line) in code.lines().enumerate() {
                if patterns.iter().any(|p| line.contains(p)) && !line.trim_start().starts_with("//") {
                    stray.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
                }
            }
        }
        assert!(stray.is_empty(), "open connections in mail_core::network:\n{}", stray.join("\n"));
    }
}
