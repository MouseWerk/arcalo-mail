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

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::account::{Security, Server};
use crate::error::{Error, Result};
use crate::{tr, trf};

/// Who connects. The timeouts (and later the proxy profile) are chosen per service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl Problem {
    /// Whether a later attempt may work without the user doing anything.
    pub fn is_transient(&self) -> bool {
        matches!(self, Problem::Dns { .. } | Problem::Refused { .. } | Problem::Timeout { .. } | Problem::Offline)
            || matches!(self, Problem::Protocol { .. })
    }

    /// Whether the user has to sign in again (or change the password) before it can work.
    pub fn needs_sign_in(&self) -> bool {
        matches!(self, Problem::Auth { .. } | Problem::AppPassword { .. })
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
fn platform_verifier() -> Result<Arc<rustls_platform_verifier::Verifier>> {
    static V: OnceLock<std::result::Result<Arc<rustls_platform_verifier::Verifier>, String>> = OnceLock::new();
    V.get_or_init(|| rustls_platform_verifier::Verifier::new(provider()).map(Arc::new).map_err(|e| e.to_string()))
        .clone()
        .map_err(|e| Error::Network(e))
}

/// The platform verifier, except for the one certificate the user trusted for this server.
/// It notes the fingerprint of what the server showed, so a refusal can offer to trust it.
#[derive(Debug)]
struct Verifier {
    inner: Arc<rustls_platform_verifier::Verifier>,
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

/// TCP to the server: the name is looked up first, so an unknown name and a closed port are
/// told apart.
async fn tcp(server: &Server) -> std::result::Result<TcpStream, Problem> {
    let host = server.host.trim();
    let port = server.port;
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
    let mut tcp = tcp(server).await?;
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
    let mut tcp = tcp(server).await?;
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

/// The HTTP client of a service: rustls with the platform verifier, a timeout, redirects only to
/// `https://` (or this computer). Every request still checks its address ([`check_https_url`]).
pub fn client_for(service: Service) -> Result<reqwest::Client> {
    let timeout = match service {
        Service::Autoconfig => Duration::from_secs(8),
        _ => Duration::from_secs(30),
    };
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(8))
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
