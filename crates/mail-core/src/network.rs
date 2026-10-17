//! The one place that decides how the app connects, as `network.rs` does in Arcalo.
//!
//! Every connection goes through this module: IMAP and SMTP (TCP with TLS or STARTTLS), every
//! HTTP client ([`client_for`]: the autoconfig lookups now; OAuth, JMAP, Microsoft Graph and the
//! update feed later) and the DNS lookups of the account setup ([`srv`], [`mx`]). A test fails
//! when other code opens a socket or builds a client
//! ([`tests::no_connection_is_opened_outside_network`]).
//!
//! The policy comes first: [`check_server`] and [`check_https_url`] refuse plain text to the
//! internet before anything is opened. TLS is rustls with the platform verifier (the system's
//! trust store on every platform); a certificate the system does not trust can be trusted for
//! one server by its SHA-256 fingerprint (`Server::trusted_cert`), which the user confirms after
//! [`Problem::Certificate`] showed it. What goes wrong while connecting is a [`Problem`], so the
//! setup and the status bar can say what to do about it.

use std::fmt;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::account::{Security, Server};
use crate::error::{Error, Result};
use crate::{tr, trf};

/// Who connects. The timeouts (and later the proxy profile) are chosen per service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Service {
    Imap,
    Smtp,
    /// The setup's lookups: autoconfig files and the provider database.
    Autoconfig,
    Jmap,
    /// Microsoft identity platform (login.microsoftonline.com).
    OAuthMicrosoft,
    /// Google's OAuth endpoints (accounts.google.com, oauth2.googleapis.com).
    OAuthGoogle,
    /// Microsoft Graph (graph.microsoft.com).
    Graph,
    /// The release feed of the desktop updater.
    UpdateFeed,
    /// A newsletter's one-click unsubscribe address (RFC 8058), only when the user asks.
    Unsubscribe,
    /// POP3 (RFC 1939) accounts.
    Pop3,
    /// ManageSieve (RFC 5804): the server's filter rules.
    Sieve,
    /// An AI provider the user set up (`ai`): a local one (Ollama, LM Studio) on this computer,
    /// or a cloud provider the user added.
    Ai,
    /// Address books: CardDAV servers and Microsoft's contacts.
    Contacts,
    /// Profile pictures: a sender's BIMI logo, Gravatar when switched on.
    Pictures,
}

/// Opening the TCP connection and the TLS handshake together.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// One answer of a server (IMAP, SMTP) once connected; IDLE has its own.
pub const IO_TIMEOUT: Duration = Duration::from_secs(60);

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

// ---------------------------------------------------------------------------- problems

/// What went wrong on the way to a mailbox, in words the user can act on. The setup shows it
/// with a hint (`ui/src/lib/problems.ts`), the status bar with its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Problem {
    /// The server name does not resolve.
    Dns {
        host: String,
    },
    /// Nothing answers on the port.
    Refused {
        host: String,
        port: u16,
    },
    Timeout {
        host: String,
        port: u16,
    },
    /// No network at all (no route, no interface).
    Offline,
    /// The TLS handshake failed for another reason than the certificate (often TLS and STARTTLS
    /// mixed up for the port).
    Tls {
        host: String,
        detail: String,
    },
    /// The certificate is not trusted by the system; the user may trust it by its fingerprint.
    Certificate {
        host: String,
        fingerprint: String,
        detail: String,
    },
    /// User name or password refused.
    Auth {
        detail: String,
    },
    /// The provider wants an app password, or IMAP/SMTP access switched on in its web settings.
    AppPassword {
        detail: String,
    },
    /// The connection broke, or the server answered something unexpected.
    Protocol {
        detail: String,
    },
    /// Microsoft: the organization's administrator has to approve Arcalo Mail first
    /// (AADSTS65001, AADSTS90094).
    AdminConsent {
        detail: String,
    },
    /// Microsoft: SMTP AUTH is switched off for the mailbox (`535 5.7.139`); reading works,
    /// sending needs it switched on by the user's IT.
    SmtpAuthDisabled {
        detail: String,
    },
    /// The proxy of the network profile refused the connection or could not reach the server.
    Proxy {
        proxy: String,
        detail: String,
    },
}

impl Problem {
    /// Whether a later attempt may work without the user doing anything.
    pub fn is_transient(&self) -> bool {
        matches!(self, Problem::Dns { .. } | Problem::Refused { .. } | Problem::Timeout { .. } | Problem::Offline)
            || matches!(self, Problem::Protocol { .. } | Problem::Proxy { .. })
    }

    /// Whether the user has to sign in again (or change the password) before it can work.
    pub fn needs_sign_in(&self) -> bool {
        matches!(self, Problem::Auth { .. } | Problem::AppPassword { .. } | Problem::AdminConsent { .. })
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Problem::Dns { host } => {
                trf!(
                    "Den Server {host} gibt es nicht (Name unbekannt).",
                    "The server {host} does not exist (unknown name)."
                )
            }
            Problem::Refused { host, port } => trf!(
                "{host} nimmt auf Port {port} keine Verbindung an.",
                "{host} does not accept connections on port {port}."
            ),
            Problem::Timeout { host, port } => trf!(
                "{host} hat auf Port {port} nicht rechtzeitig geantwortet.",
                "{host} did not answer in time on port {port}."
            ),
            Problem::Offline => tr!("Keine Verbindung zum Internet.", "No connection to the internet.").into(),
            Problem::Tls { host, detail } => trf!(
                "Die verschlüsselte Verbindung zu {host} kam nicht zustande ({detail}).",
                "The encrypted connection to {host} failed ({detail})."
            ),
            Problem::Certificate { host, detail, .. } => trf!(
                "Das Zertifikat von {host} ist nicht vertrauenswürdig ({detail}).",
                "The certificate of {host} is not trusted ({detail})."
            ),
            Problem::Auth { detail } => {
                trf!("Benutzername oder Passwort falsch ({detail}).", "Wrong user name or password ({detail}).")
            }
            Problem::AppPassword { detail } => trf!(
                "Der Anbieter verlangt ein App-Passwort oder freigeschalteten IMAP-Zugang ({detail}).",
                "The provider requires an app password or IMAP access switched on ({detail})."
            ),
            Problem::Protocol { detail } => {
                trf!("Der Server hat unerwartet geantwortet: {detail}", "The server answered unexpectedly: {detail}")
            }
            Problem::AdminConsent { detail } => trf!(
                "Die IT deiner Organisation muss Arcalo Mail zuerst freigeben ({detail}).",
                "Your organization's IT has to approve Arcalo Mail first ({detail})."
            ),
            Problem::SmtpAuthDisabled { detail } => trf!(
                "Senden über SMTP ist für dieses Konto abgeschaltet ({detail}).",
                "Sending over SMTP is switched off for this account ({detail})."
            ),
            Problem::Proxy { proxy, detail } => {
                trf!(
                    "Der Proxy {proxy} hat die Verbindung abgelehnt: {detail}",
                    "The proxy {proxy} refused the connection: {detail}"
                )
            }
        };
        f.write_str(&s)
    }
}

impl From<Problem> for Error {
    fn from(p: Problem) -> Self {
        Error::Connection(p)
    }
}

/// The problem behind an I/O error while connected (a reset, a closed connection, a timeout).
pub fn io_problem(e: &io::Error) -> Problem {
    match e.kind() {
        io::ErrorKind::TimedOut => Problem::Timeout { host: String::new(), port: 0 },
        io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => Problem::Offline,
        _ => Problem::Protocol { detail: e.to_string() },
    }
}

/// A login refusal in the server's words: an app password or a switched-off IMAP access when
/// the text says so (Gmail, iCloud, Yahoo, GMX, WEB.DE), else a wrong user name or password.
pub fn auth_problem(server_text: &str) -> Problem {
    let t = server_text.to_ascii_lowercase();
    let app = [
        "application-specific",
        "app password",
        "app-specific",
        "apppassword",
        "web login required",
        "please log in via your web browser",
        "not enabled for imap",
        "imap access is disabled",
        "imap is disabled",
        "enable imap",
        "pop3/imap",
        "freischalten",
    ];
    let detail = server_text.trim().to_string();
    if t.contains("5.7.139") || t.contains("smtpclientauthentication is disabled") {
        return Problem::SmtpAuthDisabled { detail };
    }
    if ["aadsts65001", "aadsts90094", "aadsts90095", "admin consent", "admin approval"].iter().any(|w| t.contains(w)) {
        return Problem::AdminConsent { detail };
    }
    if app.iter().any(|w| t.contains(w)) { Problem::AppPassword { detail } } else { Problem::Auth { detail } }
}

// ---------------------------------------------------------------------------- TLS

/// The SHA-256 fingerprint of a certificate as shown to the user (`AB:CD:…`).
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    static P: OnceLock<Arc<rustls::crypto::CryptoProvider>> = OnceLock::new();
    P.get_or_init(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider())).clone()
}

/// The system's verifier, made once (reading the trust store takes a moment).
#[cfg(not(target_os = "android"))]
fn platform_verifier() -> Result<Arc<dyn ServerCertVerifier>> {
    static V: OnceLock<std::result::Result<Arc<rustls_platform_verifier::Verifier>, String>> = OnceLock::new();
    V.get_or_init(|| rustls_platform_verifier::Verifier::new(provider()).map(Arc::new).map_err(|e| e.to_string()))
        .clone()
        .map(|v| v as Arc<dyn ServerCertVerifier>)
        .map_err(Error::Network)
}

/// Android, as in Arcalo's companion app: the platform verifier needs the app's JVM context and
/// a Kotlin part of its own (also in the background check, where no activity exists), so the
/// phone checks against Mozilla's root store, compiled in.
#[cfg(target_os = "android")]
fn platform_verifier() -> Result<Arc<dyn ServerCertVerifier>> {
    static V: OnceLock<std::result::Result<Arc<rustls::client::WebPkiServerVerifier>, String>> = OnceLock::new();
    V.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        roots.add_parsable_certificates(webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned());
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .map_err(|e| e.to_string())
    })
    .clone()
    .map(|v| v as Arc<dyn ServerCertVerifier>)
    .map_err(Error::Network)
}

/// The TLS configuration of the HTTP clients on Android ([`platform_verifier`]); elsewhere
/// reqwest uses the platform verifier itself.
#[cfg(target_os = "android")]
fn http_tls() -> Result<ClientConfig> {
    let mut cfg = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Network(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(platform_verifier()?)
        .with_no_client_auth();
    // reqwest is built without HTTP/2 here.
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(cfg)
}

/// Hands the Android TLS configuration to an HTTP client; nothing to do elsewhere.
fn with_tls(builder: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder> {
    #[cfg(target_os = "android")]
    {
        Ok(builder.tls_backend_preconfigured(http_tls()?))
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(builder)
    }
}

/// The platform verifier, except for the one certificate the user trusted for this server.
/// It notes the fingerprint of what the server showed, so a refusal can offer to trust it.
#[derive(Debug)]
struct Verifier {
    inner: Arc<dyn ServerCertVerifier>,
    trusted: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity);
        if self.trusted.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(&fp)) {
            return Ok(ServerCertVerified::assertion());
        }
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(fp);
        self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

// ---------------------------------------------------------------------------- proxy profiles

/// How a network profile reaches the internet (as Arcalo's profiles, without PAC).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// Always direct; proxy variables of the environment are ignored.
    None,
    /// The system's proxy: the Windows Internet settings, elsewhere `HTTPS_PROXY`, `ALL_PROXY`
    /// and `NO_PROXY`. IMAP and SMTP use only a SOCKS proxy of the system.
    #[default]
    System,
    /// An HTTP proxy (CONNECT) and/or a SOCKS5 proxy with an exception list.
    Manual,
}

/// Id of the profile every service uses unless it is routed elsewhere („Standard“).
pub const DEFAULT_PROFILE: &str = "standard";

/// The service groups that can be routed through a profile of their own.
pub const ROUTE_GROUPS: &[&str] = &["mail", "sign_in", "setup", "updates", "ai"];

/// A named way out („Firma“, „Zuhause“): a proxy and its exceptions. The proxy password lives
/// in the credential store ([`proxy_password_key`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyProfile {
    /// `[a-z0-9-]`, stable; `standard` is the default profile.
    pub id: String,
    pub name: String,
    pub mode: ProxyMode,
    /// `host:port` or `http://host:port`: HTTPS through CONNECT, and IMAP and SMTP tunneled the same way.
    pub http_proxy: String,
    /// `host:port` or `socks5://host:port`; mail connections prefer it when both are set.
    pub socks_proxy: String,
    /// Exceptions: `host`, `.domain`, `*.domain`, IP, CIDR, `<local>`, `*`; comma separated.
    pub no_proxy: String,
    pub proxy_user: String,
}

impl Default for ProxyProfile {
    fn default() -> Self {
        ProxyProfile {
            id: DEFAULT_PROFILE.into(),
            name: "Standard".into(),
            mode: ProxyMode::System,
            http_proxy: String::new(),
            socks_proxy: String::new(),
            no_proxy: "localhost, 127.0.0.1, ::1".into(),
            proxy_user: String::new(),
        }
    }
}

/// The profiles and which service group uses which (`mail`, `sign_in`, `setup`, `updates`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    pub profiles: Vec<ProxyProfile>,
    pub routes: std::collections::BTreeMap<String, String>,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        NetworkSettings { profiles: vec![ProxyProfile::default()], routes: Default::default() }
    }
}

impl Service {
    /// The route group of a service ([`ROUTE_GROUPS`]).
    pub fn group(self) -> &'static str {
        match self {
            Service::Imap
            | Service::Smtp
            | Service::Jmap
            | Service::Graph
            | Service::Unsubscribe
            | Service::Pop3
            | Service::Sieve
            | Service::Contacts
            | Service::Pictures => "mail",
            Service::Ai => "ai",
            Service::OAuthMicrosoft | Service::OAuthGoogle => "sign_in",
            Service::Autoconfig => "setup",
            Service::UpdateFeed => "updates",
        }
    }
}

impl NetworkSettings {
    /// The profile a service uses: its route, else the default profile.
    pub fn profile_for(&self, service: Service) -> ProxyProfile {
        let id = self.routes.get(service.group()).map(String::as_str).unwrap_or(DEFAULT_PROFILE);
        self.profiles
            .iter()
            .find(|p| p.id == id)
            .or_else(|| self.profiles.iter().find(|p| p.id == DEFAULT_PROFILE))
            .cloned()
            .unwrap_or_default()
    }

    /// Checks the profiles before they are saved: ids, a default profile, proxy addresses.
    pub fn validate(&self) -> Result<()> {
        if !self.profiles.iter().any(|p| p.id == DEFAULT_PROFILE) {
            return Err(Error::Invalid(
                tr!("Das Profil „Standard“ fehlt.", "The “Standard” profile is missing.").into(),
            ));
        }
        for p in &self.profiles {
            if p.id.is_empty() || !p.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                return Err(Error::Invalid(trf!("Ungültige Profil-ID „{}“.", "Invalid profile id “{}”.", p.id)));
            }
            normalize_proxy_url(&p.http_proxy, "http")?;
            normalize_proxy_url(&p.socks_proxy, "socks5")?;
            if p.mode == ProxyMode::Manual && p.http_proxy.trim().is_empty() && p.socks_proxy.trim().is_empty() {
                return Err(Error::Invalid(trf!(
                    "Profil „{}“: Für einen eigenen Proxy bitte eine Proxy-Adresse eintragen.",
                    "Profile “{}”: for a manual proxy, enter a proxy address.",
                    p.name
                )));
            }
        }
        for (group, id) in &self.routes {
            if !ROUTE_GROUPS.contains(&group.as_str()) || !self.profiles.iter().any(|p| &p.id == id) {
                return Err(Error::Invalid(trf!("Unbekannte Zuordnung „{group}“.", "Unknown route “{group}”.")));
            }
        }
        Ok(())
    }
}

/// The credential store name of a profile's proxy password.
pub fn proxy_password_key(profile_id: &str) -> String {
    format!("proxy-{profile_id}-password")
}

/// The profiles in use and their proxy passwords (from the credential store), set by the shell
/// at start and when the settings change. Unset: the default profile.
static CONFIG: std::sync::RwLock<Option<(NetworkSettings, std::collections::BTreeMap<String, String>)>> =
    std::sync::RwLock::new(None);

/// Sets the network profiles; the next connection uses them.
pub fn configure(settings: NetworkSettings, passwords: std::collections::BTreeMap<String, String>) {
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Some((settings, passwords));
}

/// `host:port` or a URL → a proxy URL without a path (`http://host:port`, `socks5://host:port`).
pub fn normalize_proxy_url(raw: &str, default_scheme: &str) -> Result<Option<String>> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let with_scheme = if s.contains("://") { s.to_owned() } else { format!("{default_scheme}://{s}") };
    let bad = || {
        Error::Invalid(trf!(
            "Proxy-Adresse „{s}“ ist ungültig (erwartet Host:Port).",
            "Proxy address “{s}” is invalid (expected host:port)."
        ))
    };
    let url = reqwest::Url::parse(&with_scheme).map_err(|_| bad())?;
    let scheme = match url.scheme() {
        "socks" | "socks5" | "socks5h" => "socks5",
        "http" | "https" => "http",
        other => {
            return Err(Error::Invalid(trf!(
                "Proxy-Protokoll „{other}“ wird nicht unterstützt (HTTP oder SOCKS5).",
                "Proxy protocol “{other}” is not supported (HTTP or SOCKS5)."
            )));
        }
    };
    let host = url.host_str().filter(|h| !h.is_empty()).ok_or_else(bad)?;
    let port = url.port().unwrap_or(if scheme == "socks5" { 1080 } else { 8080 });
    Ok(Some(format!("{scheme}://{host}:{port}")))
}

/// The proxy configured in the operating system.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SystemProxy {
    pub http: Option<String>,
    pub https: Option<String>,
    pub socks: Option<String>,
    /// Exceptions in [`NoProxy`] syntax.
    pub bypass: String,
    /// Where the values came from, for the settings page.
    pub source: String,
}

/// Proxy from the environment (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`, also lower
/// case). `get` reads a variable (injected for tests).
pub fn parse_env(get: impl Fn(&str) -> Option<String>) -> SystemProxy {
    let var = |names: &[&str]| names.iter().find_map(|n| get(n)).filter(|v| !v.trim().is_empty());
    let all = var(&["ALL_PROXY", "all_proxy"]);
    let norm = |v: Option<String>, scheme: &str| v.and_then(|v| normalize_proxy_url(&v, scheme).ok().flatten());
    let socks = all.clone().filter(|a| a.trim_start().starts_with("socks"));
    let all_http = all.filter(|a| !a.trim_start().starts_with("socks"));
    SystemProxy {
        http: norm(var(&["http_proxy", "HTTP_PROXY"]).or(all_http.clone()), "http"),
        https: norm(var(&["https_proxy", "HTTPS_PROXY"]).or(all_http), "http"),
        socks: norm(socks, "socks5"),
        bypass: var(&["no_proxy", "NO_PROXY"]).unwrap_or_default(),
        source: tr!(
            "Umgebungsvariablen (HTTPS_PROXY, ALL_PROXY, NO_PROXY)",
            "environment variables (HTTPS_PROXY, ALL_PROXY, NO_PROXY)"
        )
        .into(),
    }
}

/// Interprets the WinINet values `ProxyEnable`, `ProxyServer` and `ProxyOverride`.
/// `ProxyServer` is `host:port` (all protocols) or `http=host:port;https=host:port;socks=host:port`.
pub fn parse_wininet(enable: u32, server: &str, overrides: &str) -> SystemProxy {
    let mut sp = SystemProxy {
        source: tr!("Windows-Interneteinstellungen", "Windows Internet settings").into(),
        ..Default::default()
    };
    if enable == 0 || server.trim().is_empty() {
        return sp;
    }
    let norm = |v: &str, scheme: &str| normalize_proxy_url(v, scheme).ok().flatten();
    if server.contains('=') {
        for part in server.split(';') {
            let Some((k, v)) = part.split_once('=') else { continue };
            match k.trim().to_ascii_lowercase().as_str() {
                "http" => sp.http = norm(v, "http"),
                "https" => sp.https = norm(v, "http"),
                "socks" => sp.socks = norm(v, "socks5"),
                _ => {}
            }
        }
    } else {
        let p = norm(server, "http");
        sp.http = p.clone();
        sp.https = p;
    }
    sp.bypass = overrides.split(';').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
    sp
}

/// The proxy of the operating system now: the Windows Internet settings, elsewhere the
/// environment.
pub fn system_proxy() -> SystemProxy {
    #[cfg(windows)]
    {
        win::read()
    }
    #[cfg(not(windows))]
    {
        parse_env(|n| std::env::var(n).ok())
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn dword(name: &str) -> u32 {
        let (key, name) = (wide(KEY), wide(name));
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: valid NUL-terminated strings and a correctly sized output buffer.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut value as *mut u32).cast(),
                &mut size,
            )
        };
        if rc == 0 { value } else { 0 }
    }

    fn string(name: &str) -> String {
        let (key, name) = (wide(KEY), wide(name));
        let mut buf = vec![0u16; 4096];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: as above; `size` is the buffer size in bytes.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc != 0 {
            return String::new();
        }
        let len = (size as usize / 2).saturating_sub(1).min(buf.len());
        String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_owned()
    }

    pub fn read() -> super::SystemProxy {
        super::parse_wininet(dword("ProxyEnable"), &string("ProxyServer"), &string("ProxyOverride"))
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Bypass {
    All,
    /// `<local>`: host names without a dot.
    Local,
    /// `example.com`: the domain and its subdomains.
    Domain(String),
    /// `.example.com` / `*.example.com`: subdomains only.
    Subdomains(String),
    Ip(IpAddr),
    Cidr(IpAddr, u8),
}

/// The exception list of a proxy, as in `NO_PROXY` or the Windows „Ausnahmen“ field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NoProxy {
    entries: Vec<Bypass>,
}

impl NoProxy {
    /// Entries separated by commas, semicolons or whitespace.
    pub fn parse(list: &str) -> NoProxy {
        let mut entries = vec![];
        for raw in list.split([',', ';', ' ', '\n', '\r', '\t']) {
            let e = raw.trim().to_ascii_lowercase();
            if e.is_empty() {
                continue;
            }
            if e == "*" {
                entries.push(Bypass::All);
            } else if e == "<local>" {
                entries.push(Bypass::Local);
            } else if let Some((ip, bits)) = e.split_once('/') {
                if let (Ok(ip), Ok(bits)) = (ip.trim_matches(['[', ']']).parse::<IpAddr>(), bits.parse::<u8>()) {
                    entries.push(Bypass::Cidr(ip, bits.min(if ip.is_ipv4() { 32 } else { 128 })));
                }
            } else if let Ok(ip) = e.trim_matches(['[', ']']).parse::<IpAddr>() {
                entries.push(Bypass::Ip(ip));
            } else {
                // `host:port`: the port is ignored.
                let host = match e.rsplit_once(':') {
                    Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.contains(':') => h.to_owned(),
                    _ => e,
                };
                if let Some(rest) = host.strip_prefix("*.").or_else(|| host.strip_prefix('.')) {
                    entries.push(Bypass::Subdomains(rest.to_owned()));
                } else {
                    entries.push(Bypass::Domain(host));
                }
            }
        }
        NoProxy { entries }
    }

    /// Whether `host` (a name or an IP) bypasses the proxy.
    pub fn matches(&self, host: &str) -> bool {
        let host = host.trim().trim_matches(['[', ']']).trim_end_matches('.').to_ascii_lowercase();
        let ip = host.parse::<IpAddr>().ok();
        self.entries.iter().any(|e| match e {
            Bypass::All => true,
            Bypass::Local => ip.is_none() && !host.contains('.'),
            Bypass::Domain(d) => host == *d || host.strip_suffix(d.as_str()).is_some_and(|p| p.ends_with('.')),
            Bypass::Subdomains(d) => host.strip_suffix(d.as_str()).is_some_and(|p| p.ends_with('.') && p.len() > 1),
            Bypass::Ip(a) => ip == Some(*a),
            Bypass::Cidr(net, bits) => ip.is_some_and(|ip| in_cidr(ip, *net, *bits)),
        })
    }
}

fn in_cidr(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits.min(32)) };
            u32::from(a) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits.min(128)) };
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

/// The way one connection takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    Direct,
    /// An HTTP proxy: `CONNECT host:port` for mail and HTTPS.
    Http {
        host: String,
        port: u16,
        auth: Option<(String, String)>,
    },
    /// A SOCKS5 proxy (RFC 1928), the name resolved by the proxy.
    Socks5 {
        host: String,
        port: u16,
        auth: Option<(String, String)>,
    },
}

impl Via {
    /// The proxy as shown in messages (`http://proxy:8080`), without credentials.
    pub fn label(&self) -> String {
        match self {
            Via::Direct => String::new(),
            Via::Http { host, port, .. } => format!("http://{host}:{port}"),
            Via::Socks5 { host, port, .. } => format!("socks5://{host}:{port}"),
        }
    }

    fn url(&self) -> Option<reqwest::Url> {
        let (scheme, host, port, auth) = match self {
            Via::Direct => return None,
            Via::Http { host, port, auth } => ("http", host, port, auth),
            Via::Socks5 { host, port, auth } => ("socks5h", host, port, auth),
        };
        let mut u = reqwest::Url::parse(&format!("{scheme}://{host}:{port}")).ok()?;
        if let Some((user, pw)) = auth {
            let _ = u.set_username(user);
            let _ = u.set_password(Some(pw));
        }
        Some(u)
    }
}

fn via_of(url: &str, auth: &Option<(String, String)>) -> Option<Via> {
    let u = reqwest::Url::parse(url).ok()?;
    let host = u.host_str()?.trim_matches(['[', ']']).to_string();
    let port = u.port_or_known_default()?;
    Some(if u.scheme().starts_with("socks") {
        Via::Socks5 { host, port, auth: auth.clone() }
    } else {
        Via::Http { host, port, auth: auth.clone() }
    })
}

/// The way to `host` for a profile. `socket`: IMAP or SMTP (the SOCKS proxy first, as it
/// carries any protocol); else HTTPS (the HTTP proxy first).
pub fn route(profile: &ProxyProfile, password: Option<&str>, system: &SystemProxy, host: &str, socket: bool) -> Via {
    let auth = (!profile.proxy_user.trim().is_empty())
        .then(|| (profile.proxy_user.trim().to_string(), password.unwrap_or_default().to_string()));
    let (http, socks, bypass) = match profile.mode {
        ProxyMode::None => return Via::Direct,
        // IMAP and SMTP take only a SOCKS proxy from the system: an HTTP proxy set for the
        // browser rarely lets port 993 or 587 through. A profile of its own can tunnel them.
        ProxyMode::System if socket => (None, system.socks.clone(), system.bypass.clone()),
        ProxyMode::System => {
            (system.https.clone().or(system.http.clone()), system.socks.clone(), system.bypass.clone())
        }
        ProxyMode::Manual => (
            normalize_proxy_url(&profile.http_proxy, "http").ok().flatten(),
            normalize_proxy_url(&profile.socks_proxy, "socks5").ok().flatten(),
            profile.no_proxy.clone(),
        ),
    };
    // This computer is never reached through a proxy.
    if is_loopback(host) || NoProxy::parse(&bypass).matches(host) {
        return Via::Direct;
    }
    let pick = if socket { socks.or(http) } else { http.or(socks) };
    pick.and_then(|p| via_of(&p, &auth)).unwrap_or(Via::Direct)
}

/// The way a service takes to `host` with the configured profiles.
pub fn route_for(service: Service, host: &str) -> Via {
    let cfg = CONFIG.read().unwrap_or_else(|e| e.into_inner());
    let (settings, passwords) = cfg.clone().unwrap_or_default();
    let profile = settings.profile_for(service);
    let socket = matches!(service, Service::Imap | Service::Smtp | Service::Pop3 | Service::Sieve);
    let system = if profile.mode == ProxyMode::System { system_proxy() } else { SystemProxy::default() };
    route(&profile, passwords.get(&profile.id).map(String::as_str), &system, host, socket)
}

fn proxy_problem(via: &Via, detail: impl Into<String>) -> Problem {
    Problem::Proxy { proxy: via.label(), detail: detail.into() }
}

/// `CONNECT host:port` through an HTTP proxy; the stream then belongs to the server.
async fn http_connect(mut s: TcpStream, via: &Via, host: &str, port: u16) -> std::result::Result<TcpStream, Problem> {
    let Via::Http { auth, .. } = via else { return Ok(s) };
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some((user, pw)) = auth {
        let b = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pw}"));
        req.push_str(&format!("Proxy-Authorization: Basic {b}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await.map_err(|e| proxy_problem(via, e.to_string()))?;
    // The answer's head, byte by byte, so nothing of the server's greeting is read with it.
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 {
            return Err(proxy_problem(via, "answer too long"));
        }
        match tokio::time::timeout(CONNECT_TIMEOUT, tokio::io::AsyncReadExt::read(&mut s, &mut byte)).await {
            Ok(Ok(1)) => head.push(byte[0]),
            Ok(Ok(_)) => return Err(proxy_problem(via, tr!("Verbindung geschlossen", "connection closed"))),
            Ok(Err(e)) => return Err(proxy_problem(via, e.to_string())),
            Err(_) => return Err(proxy_problem(via, tr!("keine Antwort", "no answer"))),
        }
    }
    let status = String::from_utf8_lossy(&head);
    let first = status.lines().next().unwrap_or_default().trim().to_string();
    if first.split_whitespace().nth(1) == Some("200") { Ok(s) } else { Err(proxy_problem(via, first)) }
}

/// The SOCKS5 handshake (RFC 1928, user name and password per RFC 1929), the name resolved by
/// the proxy.
async fn socks5_connect(mut s: TcpStream, via: &Via, host: &str, port: u16) -> std::result::Result<TcpStream, Problem> {
    use tokio::io::AsyncReadExt;
    let Via::Socks5 { auth, .. } = via else { return Ok(s) };
    let err = |d: &str| proxy_problem(via, d.to_string());
    let io = |e: io::Error| proxy_problem(via, e.to_string());
    let methods: &[u8] = if auth.is_some() { &[0x00, 0x02] } else { &[0x00] };
    let mut hello = vec![0x05, methods.len() as u8];
    hello.extend_from_slice(methods);
    s.write_all(&hello).await.map_err(io)?;
    let mut reply = [0u8; 2];
    s.read_exact(&mut reply).await.map_err(io)?;
    match (reply[0], reply[1]) {
        (0x05, 0x00) => {}
        (0x05, 0x02) => {
            let (user, pw) = auth.clone().unwrap_or_default();
            let mut m = vec![0x01, user.len().min(255) as u8];
            m.extend_from_slice(&user.as_bytes()[..user.len().min(255)]);
            m.push(pw.len().min(255) as u8);
            m.extend_from_slice(&pw.as_bytes()[..pw.len().min(255)]);
            s.write_all(&m).await.map_err(io)?;
            let mut r = [0u8; 2];
            s.read_exact(&mut r).await.map_err(io)?;
            if r[1] != 0 {
                return Err(err(tr!("Anmeldung am Proxy abgelehnt", "proxy sign-in refused")));
            }
        }
        _ => return Err(err(tr!("keine passende Anmeldeart", "no acceptable sign-in method"))),
    }
    let mut req = vec![0x05, 0x01, 0x00];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => {
            req.push(0x01);
            req.extend_from_slice(&a.octets());
        }
        Ok(IpAddr::V6(a)) => {
            req.push(0x04);
            req.extend_from_slice(&a.octets());
        }
        Err(_) => {
            let h = &host.as_bytes()[..host.len().min(255)];
            req.push(0x03);
            req.push(h.len() as u8);
            req.extend_from_slice(h);
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.map_err(io)?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await.map_err(io)?;
    if head[1] != 0 {
        let why = match head[1] {
            0x02 => tr!("nicht erlaubt", "not allowed"),
            0x03 => tr!("Netz nicht erreichbar", "network unreachable"),
            0x04 => tr!("Server nicht erreichbar", "host unreachable"),
            0x05 => tr!("Verbindung abgelehnt", "connection refused"),
            _ => tr!("Fehler", "failure"),
        };
        return Err(err(why));
    }
    let rest = match head[3] {
        0x01 => 4 + 2,
        0x04 => 16 + 2,
        0x03 => {
            let mut n = [0u8; 1];
            s.read_exact(&mut n).await.map_err(io)?;
            n[0] as usize + 2
        }
        _ => return Err(err("bad address type")),
    };
    let mut skip = vec![0u8; rest];
    s.read_exact(&mut skip).await.map_err(io)?;
    Ok(s)
}

// ---------------------------------------------------------------------------- the stream

#[derive(Debug)]
enum Inner {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

/// A connection to a mail server: plain (loopback only) or TLS. `prelude` is read before the
/// socket (the SMTP greeting already consumed by STARTTLS, handed on to lettre once more).
#[derive(Debug)]
pub struct MailStream {
    inner: Inner,
    prelude: Vec<u8>,
}

impl MailStream {
    pub fn is_encrypted(&self) -> bool {
        matches!(self.inner, Inner::Tls(_))
    }
}

impl AsyncRead for MailStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if !self.prelude.is_empty() {
            let n = self.prelude.len().min(buf.remaining());
            let rest = self.prelude.split_off(n);
            buf.put_slice(&self.prelude);
            self.prelude = rest;
            return Poll::Ready(Ok(()));
        }
        match &mut self.inner {
            Inner::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Inner::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MailStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        match &mut self.inner {
            Inner::Plain(s) => Pin::new(s).poll_write(cx, data),
            Inner::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, data),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            Inner::Plain(s) => Pin::new(s).poll_flush(cx),
            Inner::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.inner {
            Inner::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Inner::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

impl lettre::transport::smtp::client::AsyncTokioStream for MailStream {
    fn peer_addr(&self) -> io::Result<std::net::SocketAddr> {
        match &self.inner {
            Inner::Plain(s) => s.peer_addr(),
            Inner::Tls(s) => s.get_ref().0.peer_addr(),
        }
    }
}

/// TCP to the server, through the proxy of the service's network profile when it has one.
async fn tcp(server: &Server, service: Service) -> std::result::Result<TcpStream, Problem> {
    let host = server.host.trim();
    let via = route_for(service, host);
    match &via {
        Via::Direct => direct(host, server.port).await,
        Via::Http { host: ph, port: pp, .. } | Via::Socks5 { host: ph, port: pp, .. } => {
            let s = direct(ph, *pp).await.map_err(|p| proxy_problem(&via, p.to_string()))?;
            match via {
                Via::Http { .. } => http_connect(s, &via, host, server.port).await,
                _ => socks5_connect(s, &via, host, server.port).await,
            }
        }
    }
}

/// TCP straight to a host: the name is looked up first, so an unknown name and a closed port
/// are told apart.
async fn direct(host: &str, port: u16) -> std::result::Result<TcpStream, Problem> {
    let addrs: Vec<_> = match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port))).await {
        Err(_) => return Err(Problem::Timeout { host: host.into(), port }),
        Ok(Err(_)) => return Err(Problem::Dns { host: host.into() }),
        Ok(Ok(a)) => a.collect(),
    };
    if addrs.is_empty() {
        return Err(Problem::Dns { host: host.into() });
    }
    let mut last = Problem::Refused { host: host.into(), port };
    for addr in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Err(_) => last = Problem::Timeout { host: host.into(), port },
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => {
                last = match e.kind() {
                    io::ErrorKind::ConnectionRefused => Problem::Refused { host: host.into(), port },
                    io::ErrorKind::TimedOut => Problem::Timeout { host: host.into(), port },
                    io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => Problem::Offline,
                    _ => Problem::Protocol { detail: e.to_string() },
                }
            }
        }
    }
    Err(last)
}

/// The TLS handshake on a TCP connection, with the platform verifier and the server's trusted
/// fingerprint.
async fn tls(tcp: TcpStream, server: &Server) -> std::result::Result<TlsStream<TcpStream>, Problem> {
    let host = server.host.trim().to_string();
    let tls_err = |detail: String| Problem::Tls { host: host.clone(), detail };
    let inner = platform_verifier().map_err(|e| tls_err(e.to_string()))?;
    let seen = Arc::new(Mutex::new(None));
    let verifier = Verifier { inner, trusted: server.trusted_cert.clone(), seen: seen.clone() };
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| tls_err(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    let name = ServerName::try_from(host.clone()).map_err(|e| tls_err(e.to_string()))?;
    let connector = TlsConnector::from(Arc::new(config));
    match tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, tcp)).await {
        Err(_) => Err(Problem::Timeout { host: host.clone(), port: server.port }),
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => {
            let rustls_err = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>());
            match rustls_err {
                Some(rustls::Error::InvalidCertificate(c)) => {
                    let fingerprint = seen.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
                    Err(Problem::Certificate { host: host.clone(), fingerprint, detail: certificate_reason(c) })
                }
                Some(other) => Err(tls_err(other.to_string())),
                None => Err(tls_err(e.to_string())),
            }
        }
    }
}

fn certificate_reason(e: &rustls::CertificateError) -> String {
    use rustls::CertificateError as C;
    match e {
        C::Expired | C::ExpiredContext { .. } => tr!("abgelaufen", "expired").into(),
        C::NotValidYet | C::NotValidYetContext { .. } => tr!("noch nicht gültig", "not valid yet").into(),
        C::UnknownIssuer => tr!("unbekannter Aussteller oder selbst signiert", "unknown issuer or self-signed").into(),
        C::NotValidForName | C::NotValidForNameContext { .. } => {
            tr!("ausgestellt für einen anderen Namen", "issued for another name").into()
        }
        C::Revoked => tr!("widerrufen", "revoked").into(),
        other => format!("{other:?}"),
    }
}

/// Reads one CRLF line (the STARTTLS exchanges before TLS).
async fn line(r: &mut BufReader<&mut TcpStream>) -> std::result::Result<String, Problem> {
    let mut s = String::new();
    match tokio::time::timeout(IO_TIMEOUT, r.read_line(&mut s)).await {
        Err(_) => Err(Problem::Timeout { host: String::new(), port: 0 }),
        Ok(Err(e)) => Err(io_problem(&e)),
        Ok(Ok(0)) => Err(Problem::Protocol { detail: tr!("Verbindung geschlossen", "connection closed").into() }),
        Ok(Ok(_)) => Ok(s),
    }
}

async fn send(tcp: &mut TcpStream, text: &str) -> std::result::Result<(), Problem> {
    tcp.write_all(text.as_bytes()).await.map_err(|e| io_problem(&e))
}

fn no_starttls(host: &str) -> Problem {
    Problem::Tls {
        host: host.into(),
        detail: tr!("der Server bietet kein STARTTLS an", "the server does not offer STARTTLS").into(),
    }
}

/// An IMAP connection, encrypted as the server setting says. `greeted`: the server's greeting
/// was read already (STARTTLS), so the client does not wait for one.
pub struct ImapTransport {
    pub stream: MailStream,
    pub greeted: bool,
}

/// Connects to an IMAP server: TLS from the first byte, or STARTTLS before anything else is
/// sent; plain text only to this computer ([`check_server`]).
pub async fn connect_imap(server: &Server) -> Result<ImapTransport> {
    check_server(server)?;
    let mut tcp = tcp(server, Service::Imap).await?;
    match server.security {
        Security::Plain => {
            Ok(ImapTransport { stream: MailStream { inner: Inner::Plain(tcp), prelude: vec![] }, greeted: false })
        }
        Security::Tls => {
            let s = tls(tcp, server).await?;
            Ok(ImapTransport { stream: MailStream { inner: Inner::Tls(Box::new(s)), prelude: vec![] }, greeted: false })
        }
        Security::StartTls => {
            {
                let mut r = BufReader::new(&mut tcp);
                let greeting = line(&mut r).await?;
                if !greeting.to_ascii_uppercase().starts_with("* OK") {
                    return Err(Problem::Protocol { detail: greeting.trim().to_string() }.into());
                }
            }
            send(&mut tcp, "s0 STARTTLS\r\n").await?;
            loop {
                let mut r = BufReader::new(&mut tcp);
                let l = line(&mut r).await?;
                if let Some(rest) = l.strip_prefix("s0 ") {
                    if !rest.to_ascii_uppercase().starts_with("OK") {
                        return Err(no_starttls(&server.host).into());
                    }
                    break;
                }
            }
            let s = tls(tcp, server).await?;
            Ok(ImapTransport { stream: MailStream { inner: Inner::Tls(Box::new(s)), prelude: vec![] }, greeted: true })
        }
    }
}

/// A POP3 connection (RFC 1939, RFC 2595 `STLS`), encrypted as the server setting says;
/// `greeted`: the `+OK` greeting was read already (STLS).
pub type Pop3Transport = ImapTransport;

/// Connects to a POP3 server: TLS from the first byte (995), or `STLS` before the login (110);
/// plain text only to this computer.
pub async fn connect_pop3(server: &Server) -> Result<Pop3Transport> {
    check_server(server)?;
    let mut tcp = tcp(server, Service::Pop3).await?;
    match server.security {
        Security::Plain => {
            Ok(ImapTransport { stream: MailStream { inner: Inner::Plain(tcp), prelude: vec![] }, greeted: false })
        }
        Security::Tls => {
            let s = tls(tcp, server).await?;
            Ok(ImapTransport { stream: MailStream { inner: Inner::Tls(Box::new(s)), prelude: vec![] }, greeted: false })
        }
        Security::StartTls => {
            {
                let mut r = BufReader::new(&mut tcp);
                let greeting = line(&mut r).await?;
                if !greeting.starts_with("+OK") {
                    return Err(Problem::Protocol { detail: greeting.trim().to_string() }.into());
                }
            }
            send(&mut tcp, "STLS\r\n").await?;
            let ok = {
                let mut r = BufReader::new(&mut tcp);
                line(&mut r).await?
            };
            if !ok.starts_with("+OK") {
                return Err(no_starttls(&server.host).into());
            }
            let s = tls(tcp, server).await?;
            Ok(ImapTransport { stream: MailStream { inner: Inner::Tls(Box::new(s)), prelude: vec![] }, greeted: true })
        }
    }
}

/// Connects to a ManageSieve server (RFC 5804, port 4190): STARTTLS after the first capability
/// list, TLS from the first byte, or plain text to this computer only. In every case the
/// client reads a capability list first (the server sends it again after STARTTLS).
pub async fn connect_sieve(server: &Server) -> Result<MailStream> {
    check_server(server)?;
    let mut tcp = tcp(server, Service::Sieve).await?;
    match server.security {
        Security::Plain => Ok(MailStream { inner: Inner::Plain(tcp), prelude: vec![] }),
        Security::Tls => Ok(MailStream { inner: Inner::Tls(Box::new(tls(tcp, server).await?)), prelude: vec![] }),
        Security::StartTls => {
            let mut starttls = false;
            loop {
                let l = {
                    let mut r = BufReader::new(&mut tcp);
                    line(&mut r).await?
                };
                let upper = l.trim().to_ascii_uppercase();
                if upper.starts_with("\"STARTTLS\"") {
                    starttls = true;
                }
                if upper.starts_with("OK") {
                    break;
                }
                if upper.starts_with("NO") || upper.starts_with("BYE") {
                    return Err(Problem::Protocol { detail: l.trim().to_string() }.into());
                }
            }
            if !starttls {
                return Err(no_starttls(&server.host).into());
            }
            send(&mut tcp, "STARTTLS\r\n").await?;
            let ok = {
                let mut r = BufReader::new(&mut tcp);
                line(&mut r).await?
            };
            if !ok.trim().to_ascii_uppercase().starts_with("OK") {
                return Err(no_starttls(&server.host).into());
            }
            Ok(MailStream { inner: Inner::Tls(Box::new(tls(tcp, server).await?)), prelude: vec![] })
        }
    }
}

/// Reads an SMTP reply (`250-…` lines up to `250 …`); returns the code and the lines.
async fn smtp_reply(tcp: &mut TcpStream) -> std::result::Result<(u16, Vec<String>), Problem> {
    let mut r = BufReader::new(tcp);
    let mut lines = vec![];
    loop {
        let l = line(&mut r).await?;
        let code = l
            .get(..3)
            .and_then(|c| c.parse::<u16>().ok())
            .ok_or_else(|| Problem::Protocol { detail: l.trim().into() })?;
        lines.push(l.get(4..).unwrap_or_default().trim().to_string());
        if l.as_bytes().get(3) != Some(&b'-') {
            return Ok((code, lines));
        }
    }
}

/// Connects to an SMTP server (TLS, or STARTTLS on submission port 587; plain to this computer
/// only). The returned stream starts with a greeting, as lettre expects.
pub async fn connect_smtp(server: &Server) -> Result<MailStream> {
    check_server(server)?;
    let mut tcp = tcp(server, Service::Smtp).await?;
    match server.security {
        Security::Plain => Ok(MailStream { inner: Inner::Plain(tcp), prelude: vec![] }),
        Security::Tls => Ok(MailStream { inner: Inner::Tls(Box::new(tls(tcp, server).await?)), prelude: vec![] }),
        Security::StartTls => {
            let (code, text) = smtp_reply(&mut tcp).await?;
            if code != 220 {
                return Err(Problem::Protocol { detail: text.join(" ") }.into());
            }
            send(&mut tcp, &format!("EHLO {}\r\n", hello_name())).await?;
            let (code, caps) = smtp_reply(&mut tcp).await?;
            if code != 250 || !caps.iter().any(|c| c.eq_ignore_ascii_case("STARTTLS")) {
                return Err(no_starttls(&server.host).into());
            }
            send(&mut tcp, "STARTTLS\r\n").await?;
            let (code, _) = smtp_reply(&mut tcp).await?;
            if code != 220 {
                return Err(no_starttls(&server.host).into());
            }
            let s = tls(tcp, server).await?;
            Ok(MailStream { inner: Inner::Tls(Box::new(s)), prelude: b"220 ready\r\n".to_vec() })
        }
    }
}

/// The name in EHLO: a neutral one, so mail does not carry the computer's name.
pub fn hello_name() -> &'static str {
    "arcalo-mail.localdomain"
}

// ---------------------------------------------------------------------------- HTTP and DNS

/// An HTTP client made by [`client_for`].
pub type HttpClient = reqwest::Client;

/// The HTTP client of a service: rustls with the platform verifier, a timeout, redirects only to
/// `https://` (or this computer). Every request still checks its address ([`check_https_url`]).
pub fn client_for(service: Service) -> Result<HttpClient> {
    let timeout = match service {
        Service::Autoconfig => Duration::from_secs(8),
        _ => Duration::from_secs(30),
    };
    let proxy = reqwest::Proxy::custom(move |url| url.host_str().and_then(|h| route_for(service, h).url()));
    with_tls(reqwest::Client::builder())?
        .no_proxy()
        .proxy(proxy)
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(8))
        .user_agent(concat!("ArcaloMail/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 5 || check_https_url(a.url().as_str()).is_err() { a.stop() } else { a.follow() }
        }))
        .build()
        .map_err(|e| Error::Network(e.to_string()))
}

/// The loopback listener of an OAuth sign-in on the desktop (RFC 8252, 7.3): the browser is
/// sent back to `http://127.0.0.1:<port>` with the code; nothing else on the network can
/// reach it.
pub struct RedirectListener {
    listener: tokio::net::TcpListener,
    pub port: u16,
}

/// Opens the listener on a free port of this computer.
pub async fn redirect_listener() -> Result<RedirectListener> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| Error::Network(e.to_string()))?;
    let port = listener.local_addr().map_err(|e| Error::Network(e.to_string()))?.port();
    Ok(RedirectListener { listener, port })
}

impl RedirectListener {
    /// The redirect URI to register and send (no path: Microsoft compares it as given).
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Waits for the browser's redirect and answers it with `page(query)` (HTML); returns the query
    /// string (`code=…&state=…` or `error=…`). Other requests (a favicon) get a 404.
    pub async fn wait(self, timeout: Duration, page: &(dyn Fn(&str) -> String + Sync)) -> Result<String> {
        let accept = async {
            loop {
                let (stream, _) = self.listener.accept().await.map_err(|e| Error::Network(e.to_string()))?;
                let mut r = BufReader::new(stream);
                let mut first = String::new();
                if tokio::time::timeout(IO_TIMEOUT, r.read_line(&mut first)).await.is_err() {
                    continue;
                }
                loop {
                    let mut h = String::new();
                    match tokio::time::timeout(IO_TIMEOUT, r.read_line(&mut h)).await {
                        Ok(Ok(n)) if n > 2 => continue,
                        _ => break,
                    }
                }
                let target = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let query = target.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
                let hit = query.split('&').any(|kv| kv.starts_with("code=") || kv.starts_with("error="));
                let mut stream = r.into_inner();
                let (status, body) = if hit { ("200 OK", page(&query)) } else { ("404 Not Found", String::new()) };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.flush().await;
                if hit {
                    return Ok(query);
                }
            }
        };
        match tokio::time::timeout(timeout, accept).await {
            Ok(r) => r,
            Err(_) => Err(Error::Network(
                tr!(
                    "Die Anmeldung im Browser wurde nicht rechtzeitig abgeschlossen.",
                    "The sign-in in the browser was not finished in time."
                )
                .into(),
            )),
        }
    }
}

/// An HTTP client for a long-lived stream (JMAP's EventSource push): no total timeout, but a
/// read timeout, so a stream the server stopped feeding (no pings) ends.
pub fn client_for_events(service: Service) -> Result<HttpClient> {
    let proxy = reqwest::Proxy::custom(move |url| url.host_str().and_then(|h| route_for(service, h).url()));
    with_tls(reqwest::Client::builder())?
        .no_proxy()
        .proxy(proxy)
        .connect_timeout(Duration::from_secs(8))
        .read_timeout(Duration::from_secs(150))
        .user_agent(concat!("ArcaloMail/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 5 || check_https_url(a.url().as_str()).is_err() { a.stop() } else { a.follow() }
        }))
        .build()
        .map_err(|e| Error::Network(e.to_string()))
}

fn resolver() -> Result<hickory_resolver::TokioResolver> {
    hickory_resolver::TokioResolver::builder_tokio().and_then(|b| b.build()).map_err(|e| Error::Network(e.to_string()))
}

/// One SRV record (RFC 2782): where a service of a domain runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvTarget {
    pub host: String,
    pub port: u16,
    pub priority: u16,
}

/// SRV lookup (`_imaps._tcp.example.org`), best first; empty when there is none. A target of
/// `.` means the service is not offered (RFC 6186) and is left out.
pub async fn srv(name: &str) -> Result<Vec<SrvTarget>> {
    use hickory_resolver::proto::rr::{RData, RecordType};
    let r = resolver()?;
    let Ok(lookup) = tokio::time::timeout(Duration::from_secs(5), r.lookup(name, RecordType::SRV))
        .await
        .map_err(|_| Error::Network("DNS".into()))?
    else {
        return Ok(vec![]);
    };
    let mut out: Vec<SrvTarget> = lookup
        .answers()
        .iter()
        .filter_map(|rec| match &rec.data {
            RData::SRV(s) => Some(SrvTarget {
                host: s.target.to_utf8().trim_end_matches('.').to_string(),
                port: s.port,
                priority: s.priority,
            }),
            _ => None,
        })
        .filter(|t| !t.host.is_empty() && t.port != 0)
        .collect();
    out.sort_by_key(|t| t.priority);
    Ok(out)
}

/// The mail exchangers of a domain, best first (to recognize hosted domains: IONOS, Strato,
/// Google Workspace, Microsoft 365).
pub async fn mx(domain: &str) -> Result<Vec<String>> {
    use hickory_resolver::proto::rr::{RData, RecordType};
    let r = resolver()?;
    let Ok(lookup) = tokio::time::timeout(Duration::from_secs(5), r.lookup(domain, RecordType::MX))
        .await
        .map_err(|_| Error::Network("DNS".into()))?
    else {
        return Ok(vec![]);
    };
    let mut out: Vec<(u16, String)> = lookup
        .answers()
        .iter()
        .filter_map(|rec| match &rec.data {
            RData::MX(m) => Some((m.preference, m.exchange.to_utf8().trim_end_matches('.').to_ascii_lowercase())),
            _ => None,
        })
        .collect();
    out.sort();
    Ok(out.into_iter().map(|(_, h)| h).collect())
}

/// The TXT records of a name, each one's strings joined (BIMI's `default._bimi.<domain>`,
/// DMARC's `_dmarc.<domain>`); empty when there is none.
pub async fn txt(name: &str) -> Result<Vec<String>> {
    use hickory_resolver::proto::rr::{RData, RecordType};
    let r = resolver()?;
    let Ok(lookup) = tokio::time::timeout(Duration::from_secs(5), r.lookup(name, RecordType::TXT))
        .await
        .map_err(|_| Error::Network("DNS".into()))?
    else {
        return Ok(vec![]);
    };
    Ok(lookup
        .answers()
        .iter()
        .filter_map(|rec| match &rec.data {
            RData::TXT(t) => {
                Some(t.txt_data.iter().map(|part| String::from_utf8_lossy(part).into_owned()).collect::<String>())
            }
            _ => None,
        })
        .collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small HTTP server on this computer for the tests of other modules (OAuth, JMAP,
    /// autoconfig): `handle(method, path_and_query, headers, body)` answers each request.
    pub(crate) mod testhttp {
        use std::sync::Arc;

        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        pub(crate) struct Reply {
            pub status: u16,
            pub content_type: String,
            pub body: Vec<u8>,
            pub location: Option<String>,
        }

        impl Reply {
            pub fn status(status: u16, body: &str) -> Reply {
                Reply {
                    status,
                    content_type: "application/json".into(),
                    body: body.as_bytes().to_vec(),
                    location: None,
                }
            }
        }

        type Handler = dyn Fn(&str, &str, &[(String, String)], &[u8]) -> Reply + Send + Sync;

        /// Serves until the test ends; returns the port.
        pub(crate) async fn serve(handle: Arc<Handler>) -> u16 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                loop {
                    let Ok((s, _)) = listener.accept().await else { return };
                    let handle = handle.clone();
                    tokio::spawn(async move {
                        let mut r = BufReader::new(s);
                        loop {
                            let mut first = String::new();
                            if r.read_line(&mut first).await.unwrap_or(0) == 0 {
                                return;
                            }
                            let mut parts = first.split_whitespace();
                            let method = parts.next().unwrap_or_default().to_string();
                            let target = parts.next().unwrap_or("/").to_string();
                            let mut headers = vec![];
                            let mut length = 0usize;
                            loop {
                                let mut h = String::new();
                                if r.read_line(&mut h).await.unwrap_or(0) == 0 || h.trim().is_empty() {
                                    break;
                                }
                                if let Some((k, v)) = h.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        length = v.trim().parse().unwrap_or(0);
                                    }
                                    headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                                }
                            }
                            let mut body = vec![0u8; length];
                            if r.read_exact(&mut body).await.is_err() {
                                return;
                            }
                            let reply = handle(&method, &target, &headers, &body);
                            let location = reply.location.map(|l| format!("Location: {l}\r\n")).unwrap_or_default();
                            let head = format!(
                                "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\n{location}\r\n",
                                reply.status,
                                reply.content_type,
                                reply.body.len()
                            );
                            let s = r.get_mut();
                            if s.write_all(head.as_bytes()).await.is_err() || s.write_all(&reply.body).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            });
            port
        }
    }

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

    #[test]
    fn login_refusals_are_told_apart() {
        assert!(matches!(auth_problem("[AUTHENTICATIONFAILED] Invalid credentials (Failure)"), Problem::Auth { .. }));
        assert!(matches!(
            auth_problem(
                "[ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833"
            ),
            Problem::AppPassword { .. }
        ));
        assert!(matches!(
            auth_problem("Login failed: IMAP access is disabled for this account"),
            Problem::AppPassword { .. }
        ));
        assert!(Problem::Auth { detail: String::new() }.needs_sign_in());
        assert!(Problem::Offline.is_transient());
        assert!(
            !Problem::Certificate { host: "x".into(), fingerprint: "AA".into(), detail: String::new() }.is_transient()
        );
    }

    #[test]
    fn proxy_routes_and_exceptions() {
        let manual = ProxyProfile {
            mode: ProxyMode::Manual,
            http_proxy: "proxy.example.org:3128".into(),
            socks_proxy: "socks5://socks.example.org".into(),
            no_proxy: "intern.example.org, .corp.example, 10.0.0.0/8".into(),
            proxy_user: "maria".into(),
            ..ProxyProfile::default()
        };
        let none = SystemProxy::default();
        let auth = Some(("maria".to_string(), "pw".to_string()));
        assert_eq!(
            route(&manual, Some("pw"), &none, "imap.example.org", true),
            Via::Socks5 { host: "socks.example.org".into(), port: 1080, auth: auth.clone() }
        );
        assert_eq!(
            route(&manual, Some("pw"), &none, "login.microsoftonline.com", false),
            Via::Http { host: "proxy.example.org".into(), port: 3128, auth }
        );
        for direct in ["intern.example.org", "mail.corp.example", "10.1.2.3", "127.0.0.1", "localhost"] {
            assert_eq!(route(&manual, None, &none, direct, true), Via::Direct, "{direct}");
        }
        let off = ProxyProfile { mode: ProxyMode::None, ..manual.clone() };
        assert_eq!(route(&off, None, &none, "imap.example.org", true), Via::Direct);
        // The system's HTTP proxy carries HTTPS, not IMAP and SMTP; its SOCKS proxy carries both.
        let env = parse_env(|n| match n {
            "HTTPS_PROXY" => Some("http://web-proxy:8080".into()),
            "NO_PROXY" => Some("example.net".into()),
            _ => None,
        });
        let system = ProxyProfile::default();
        assert_eq!(route(&system, None, &env, "imap.example.org", true), Via::Direct);
        assert!(matches!(route(&system, None, &env, "jmap.example.org", false), Via::Http { port: 8080, .. }));
        assert_eq!(route(&system, None, &env, "api.example.net", false), Via::Direct);
        let socks = parse_env(|n| (n == "ALL_PROXY").then(|| "socks5://127.0.0.2:1080".to_string()));
        assert!(matches!(route(&system, None, &socks, "imap.example.org", true), Via::Socks5 { .. }));
        let win = parse_wininet(1, "http=p:80;https=p:443;socks=s:1080", "<local>;*.intra");
        assert_eq!((win.https.as_deref(), win.socks.as_deref()), (Some("http://p:443"), Some("socks5://s:1080")));
        assert!(NoProxy::parse(&win.bypass).matches("server") && NoProxy::parse(&win.bypass).matches("a.intra"));
        assert!(normalize_proxy_url("ftp://x:1", "http").is_err());
    }

    #[test]
    fn network_settings_are_checked_and_routed() {
        let mut n = NetworkSettings::default();
        n.profiles.push(ProxyProfile {
            id: "firma".into(),
            name: "Firma".into(),
            mode: ProxyMode::Manual,
            http_proxy: "proxy:3128".into(),
            ..ProxyProfile::default()
        });
        n.routes.insert("mail".into(), "firma".into());
        n.validate().unwrap();
        assert_eq!(n.profile_for(Service::Imap).id, "firma");
        assert_eq!(n.profile_for(Service::Autoconfig).id, DEFAULT_PROFILE);
        n.routes.insert("mail".into(), "fehlt".into());
        assert!(n.validate().is_err());
        n.routes.clear();
        n.profiles[1].http_proxy.clear();
        assert!(n.validate().is_err(), "manual without an address");
    }

    /// A server that greets like IMAP, reached through an HTTP CONNECT proxy and a SOCKS5
    /// proxy (with user name and password) on this computer.
    #[tokio::test]
    async fn tunnels_through_http_connect_and_socks5() {
        use tokio::io::AsyncReadExt;
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = server.accept().await else { return };
                let _ = s.write_all(b"* OK ready\r\n").await;
            }
        });
        async fn pipe(mut a: TcpStream, port: u16) {
            let mut b = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
        }
        // HTTP CONNECT, checking the target and the credentials.
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = http.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = http.accept().await else { return };
                let mut head = vec![];
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    s.read_exact(&mut b).await.unwrap();
                    head.push(b[0]);
                }
                let h = String::from_utf8_lossy(&head).to_string();
                assert!(h.starts_with("CONNECT mail.example.org:993 HTTP/1.1"), "{h}");
                assert!(h.contains("Proxy-Authorization: Basic bWFyaWE6cHc="), "{h}");
                s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
                tokio::spawn(pipe(s, server_port));
            }
        });
        let auth = Some(("maria".to_string(), "pw".to_string()));
        let via = Via::Http { host: "127.0.0.1".into(), port: http_port, auth: auth.clone() };
        let s = direct("127.0.0.1", http_port).await.unwrap();
        let mut s = http_connect(s, &via, "mail.example.org", 993).await.unwrap();
        let mut greeting = [0u8; 12];
        s.read_exact(&mut greeting).await.unwrap();
        assert_eq!(&greeting, b"* OK ready\r\n");

        // SOCKS5 with user name and password, the name resolved by the proxy.
        let socks = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socks_port = socks.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = socks.accept().await else { return };
                let mut hello = [0u8; 4];
                s.read_exact(&mut hello).await.unwrap();
                assert_eq!(hello, [5, 2, 0, 2]);
                s.write_all(&[5, 2]).await.unwrap();
                let mut v = [0u8; 2];
                s.read_exact(&mut v).await.unwrap();
                let mut user = vec![0u8; v[1] as usize];
                s.read_exact(&mut user).await.unwrap();
                let mut n = [0u8; 1];
                s.read_exact(&mut n).await.unwrap();
                let mut pw = vec![0u8; n[0] as usize];
                s.read_exact(&mut pw).await.unwrap();
                assert_eq!((user.as_slice(), pw.as_slice()), (&b"maria"[..], &b"pw"[..]));
                s.write_all(&[1, 0]).await.unwrap();
                let mut req = [0u8; 5];
                s.read_exact(&mut req).await.unwrap();
                assert_eq!(&req[..4], &[5, 1, 0, 3]);
                let mut name = vec![0u8; req[4] as usize + 2];
                s.read_exact(&mut name).await.unwrap();
                assert_eq!(&name[..name.len() - 2], b"mail.example.org");
                s.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await.unwrap();
                tokio::spawn(pipe(s, server_port));
            }
        });
        let via = Via::Socks5 { host: "127.0.0.1".into(), port: socks_port, auth };
        let s = direct("127.0.0.1", socks_port).await.unwrap();
        let mut s = socks5_connect(s, &via, "mail.example.org", 993).await.unwrap();
        let mut greeting = [0u8; 12];
        s.read_exact(&mut greeting).await.unwrap();
        assert_eq!(&greeting, b"* OK ready\r\n");

        // A proxy that refuses: the problem names the proxy.
        let refuse = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let refuse_port = refuse.local_addr().unwrap().port();
        tokio::spawn(async move {
            let Ok((mut s, _)) = refuse.accept().await else { return };
            let mut b = [0u8; 512];
            let _ = s.read(&mut b).await;
            let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
        });
        let via = Via::Http { host: "127.0.0.1".into(), port: refuse_port, auth: None };
        let s = direct("127.0.0.1", refuse_port).await.unwrap();
        let err = http_connect(s, &via, "mail.example.org", 993).await.unwrap_err();
        assert!(matches!(&err, Problem::Proxy { detail, .. } if detail.contains("403")), "{err:?}");
        assert!(err.is_transient());
    }

    #[test]
    fn fingerprints_are_sha256_in_pairs() {
        let f = fingerprint(b"abc");
        assert_eq!(f.len(), 32 * 3 - 1);
        assert!(f.starts_with("BA:78:16:BF"), "{f}");
    }

    #[tokio::test]
    async fn names_ports_and_plain_text_are_reported() {
        // A closed port on this computer.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let err = connect_imap(&Server::new("127.0.0.1", port, Security::Plain)).await.err().unwrap();
        assert!(matches!(err, Error::Connection(Problem::Refused { .. })), "{err:?}");
        let err = connect_imap(&Server::new("mail.invalid", 993, Security::Tls)).await.err().unwrap();
        assert!(
            matches!(err, Error::Connection(Problem::Dns { .. }) | Error::Connection(Problem::Timeout { .. })),
            "{err:?}"
        );
        let err = connect_smtp(&Server::new("smtp.example.org", 25, Security::Plain)).await.err().unwrap();
        assert!(matches!(err, Error::Network(_)), "{err:?}");
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
            "TcpListener::bind",
            "UdpSocket::bind",
            "lookup_host(",
            "reqwest::Client",
            "Client::builder(",
            "ClientBuilder::new(",
            "reqwest::get(",
            "TlsConnector::from(",
            "TokioResolver::",
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
