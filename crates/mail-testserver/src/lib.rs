//! An IMAP and SMTP server on this computer, for Arcalo Mail's protocol tests, the e2e tests
//! and the README screenshots. It serves the in-memory fake (`mail_core::imap::fake::FakeImap`)
//! over the wire: RFC 3501 with LITERAL+, IDLE, ENABLE, CONDSTORE/QRESYNC, UIDPLUS, MOVE and
//! SPECIAL-USE (each can be switched off), STARTTLS and implicit TLS with a self-signed
//! certificate, and an SMTP server (AUTH PLAIN and LOGIN) that delivers mail to the account's
//! inbox when it is one of the recipients.
//!
//! The same mailboxes are served over JMAP (RFC 8620/8621, with EventSource push) and an
//! identity provider for OAuth sign-ins (XOAUTH2 on IMAP and SMTP) on the control port; the
//! inbox also over POP3. A ManageSieve server keeps uploaded rules, and the control port serves
//! an OpenAI-compatible AI provider (`/ai/v1/…`), a CardDAV address book (`/dav/…`), two CalDAV
//! calendars (`/caldav/…`), Graph's calendar endpoints (`/graph/v1.0/…`) and Jonas, who reads
//! and writes encrypted mail (`/pgp/…`).
//!
//! A small HTTP control port lets a test deliver mail, look at the server's state, read what
//! was sent, make commands fail and take the server offline and back. It listens on loopback
//! only and never talks to the internet.

mod control;
mod imap;
mod jmap;
pub mod peer;
mod pop3;
mod sieve;
mod smtp;

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use mail_core::imap::Capabilities;
use mail_core::imap::fake::FakeImap;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

/// A connection, plain or TLS (after STARTTLS or from the first byte).
pub(crate) trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}
pub(crate) type Io = Box<dyn Duplex>;

#[derive(Debug, Clone)]
pub struct Options {
    /// The account's address (and IMAP/SMTP user name).
    pub user: String,
    pub password: String,
    pub caps: Capabilities,
    /// Serve STARTTLS and implicit TLS with a self-signed certificate for `localhost`.
    pub tls: bool,
    /// File sent mail in Sent itself, as Gmail does.
    pub files_sent: bool,
    /// Fixed ports (0: any free one).
    pub imap_port: u16,
    pub smtp_port: u16,
    pub control_port: u16,
    pub pop3_port: u16,
    pub sieve_port: u16,
    /// JMAP with push (EventSource); without it clients poll.
    pub jmap_push: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            user: "maria@example.org".into(),
            password: "secret".into(),
            caps: FakeImap::default().caps,
            tls: false,
            files_sent: false,
            imap_port: 0,
            smtp_port: 0,
            control_port: 0,
            pop3_port: 0,
            sieve_port: 0,
            jmap_push: true,
        }
    }
}

/// A request to the web side of a sender (`/unsubscribe…`, `/track…`, `/img…` on the control
/// port): what a newsletter's unsubscribe address or tracking pixel would have received.
#[derive(Debug, Clone, Serialize)]
pub struct WebRequest {
    pub method: String,
    pub path: String,
    pub content_type: Option<String>,
    pub body: String,
}

/// A message the SMTP server took.
#[derive(Debug, Clone, Serialize)]
pub struct SentMail {
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    #[serde(skip)]
    pub raw: Vec<u8>,
    pub size: usize,
}

pub struct Shared {
    pub fake: Mutex<FakeImap>,
    pub sent: Mutex<Vec<SentMail>>,
    pub password: Mutex<String>,
    pub user: String,
    pub files_sent: bool,
    changes: watch::Sender<u64>,
    offline: watch::Sender<bool>,
    tls: Option<TlsAcceptor>,
    /// IMAP connections opened so far (the tests check the worker's pool).
    pub connections: Mutex<u32>,
    /// The plain-text IMAP and SMTP ports (for the autoconfig file of the control port).
    pub ports: Mutex<(u16, u16)>,
    /// The control port (for addresses the fakes hand out).
    pub control: Mutex<u16>,
    /// A slow server: new connections wait this long before the greeting (milliseconds).
    pub delay_ms: Mutex<u64>,
    /// The identity provider whose access tokens IMAP and SMTP take (XOAUTH2).
    pub oauth: Mutex<mail_core::oauth::fake::FakeOAuth>,
    /// SMTP refuses XOAUTH2 with `535 5.7.139` (SMTP AUTH switched off for the mailbox).
    pub smtp_auth_disabled: Mutex<bool>,
    /// SMTP offers 8BITMIME (on unless a test imitates GreenMail, which offers only AUTH and
    /// SMTPUTF8); without it, a message with 8-bit bytes is refused.
    pub smtp_8bitmime: Mutex<bool>,
    /// The JMAP view of the same mailboxes (`jmap.rs`).
    pub jmap: Mutex<mail_core::jmap::fake::FakeJmapState>,
    /// Requests to the senders' web addresses (`control.rs`).
    pub web: Mutex<Vec<WebRequest>>,
    /// The AI provider of the control port (`/ai/v1/…`).
    pub ai: Mutex<mail_core::ai::fake::FakeAi>,
    /// The ManageSieve server's scripts.
    pub sieve: Mutex<mail_core::sieve::fake::FakeSieve>,
    /// The CardDAV address book of the control port (`/dav/…`).
    pub carddav: Mutex<mail_core::carddav::fake::FakeCardDav>,
    /// POP3 sessions so far.
    pub pop3_sessions: Mutex<u32>,
    /// The CalDAV calendars of the control port (`/caldav/…`).
    pub caldav: Mutex<mail_core::calendar::caldav::fake::FakeCalDav>,
    /// Graph's calendars (`/graph/v1.0/…`).
    pub graph_calendar: Mutex<mail_core::calendar::graph::fake::FakeGraphCalendar>,
    /// Jonas, the other side of encrypted mail (`peer.rs`).
    pub peer: Mutex<peer::Peer>,
}

impl Shared {
    /// Changes the mailbox as the test or another client would; IDLE connections notice.
    pub fn with<R>(&self, f: impl FnOnce(&mut FakeImap) -> R) -> R {
        let r = f(&mut self.fake.lock().unwrap_or_else(|e| e.into_inner()));
        self.changed();
        r
    }

    pub(crate) fn changed(&self) {
        self.changes.send_modify(|v| *v += 1);
    }

    pub fn set_offline(&self, offline: bool) {
        self.offline.send_replace(offline);
    }

    pub fn is_offline(&self) -> bool {
        *self.offline.borrow()
    }

    pub(crate) fn check_login(&self, user: &str, password: &str) -> bool {
        user.eq_ignore_ascii_case(&self.user) && password == *self.password.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits until the server is taken offline (a connection then ends).
    pub(crate) async fn until_offline(&self) {
        let mut rx = self.offline.subscribe();
        let _ = rx.wait_for(|o| *o).await;
    }

    pub(crate) fn subscribe_changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }
}

/// A running server.
pub struct Running {
    pub imap: u16,
    pub smtp: u16,
    /// Implicit TLS ports (with `tls`).
    pub imaps: Option<u16>,
    pub smtps: Option<u16>,
    pub control: u16,
    pub pop3: u16,
    pub sieve: u16,
    /// The SHA-256 fingerprint of the certificate (with `tls`).
    pub fingerprint: Option<String>,
    pub shared: Arc<Shared>,
}

fn tls_acceptor() -> io::Result<(TlsAcceptor, String)> {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
        .map_err(io::Error::other)?;
    let der = ck.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::try_from(ck.signing_key.serialize_der()).map_err(io::Error::other)?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(vec![der.clone()], key)
        .map_err(io::Error::other)?;
    Ok((TlsAcceptor::from(Arc::new(config)), mail_core::network::fingerprint(&der)))
}

async fn listen(port: u16) -> io::Result<(TcpListener, u16)> {
    let l = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
    let p = l.local_addr()?.port();
    Ok((l, p))
}

#[derive(Clone, Copy)]
enum Kind {
    Imap,
    Smtp,
    Pop3,
    Sieve,
}

fn serve(listener: TcpListener, shared: Arc<Shared>, kind: Kind, implicit_tls: bool) {
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            if shared.is_offline() {
                drop(tcp);
                continue;
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                let _ = tcp.set_nodelay(true);
                let delay = *shared.delay_ms.lock().unwrap_or_else(|e| e.into_inner());
                if delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                let io: Io = if implicit_tls {
                    let Some(acceptor) = shared.tls.clone() else { return };
                    match acceptor.accept(tcp).await {
                        Ok(s) => Box::new(s),
                        Err(_) => return,
                    }
                } else {
                    Box::new(tcp) as Io
                };
                let session = async {
                    match kind {
                        Kind::Imap => imap::session(io, shared.clone(), implicit_tls).await,
                        Kind::Smtp => smtp::session(io, shared.clone(), implicit_tls).await,
                        Kind::Pop3 => {
                            let _ = pop3::session(io, shared.clone()).await;
                        }
                        Kind::Sieve => {
                            let _ = sieve::session(io, shared.clone()).await;
                        }
                    }
                };
                tokio::select! {
                    _ = session => {}
                    _ = shared.until_offline() => {}
                }
            });
        }
    });
}

/// Checks a SASL XOAUTH2 response (`user=…^Aauth=Bearer …^A^A`) against the identity provider.
pub(crate) fn check_xoauth2(shared: &Shared, decoded: &str) -> bool {
    let mut user = "";
    let mut token = "";
    for part in decoded.split('\x01') {
        if let Some(u) = part.strip_prefix("user=") {
            user = u;
        } else if let Some(t) = part.strip_prefix("auth=Bearer ") {
            token = t;
        }
    }
    user.eq_ignore_ascii_case(&shared.user) && shared.oauth.lock().unwrap_or_else(|e| e.into_inner()).valid(token)
}

/// Starts the server on loopback ports.
pub async fn start(opts: Options) -> io::Result<Running> {
    let (tls, fingerprint) = if opts.tls {
        let (a, f) = tls_acceptor()?;
        (Some(a), Some(f))
    } else {
        (None, None)
    };
    // Gmail (`X-GM-EXT-1`): its folders below „[Gmail]“ (in German) and labels as folders.
    let fake = if opts.caps.gmail {
        let mut f = FakeImap::gmail();
        f.caps = opts.caps;
        f
    } else {
        FakeImap::new(opts.caps).with_all_folders()
    };
    let shared = Arc::new(Shared {
        fake: Mutex::new(fake),
        sent: Mutex::default(),
        password: Mutex::new(opts.password.clone()),
        user: opts.user.clone(),
        files_sent: opts.files_sent,
        changes: watch::channel(0).0,
        offline: watch::channel(false).0,
        tls,
        connections: Mutex::new(0),
        ports: Mutex::new((0, 0)),
        control: Mutex::new(0),
        delay_ms: Mutex::new(0),
        oauth: Mutex::new(mail_core::oauth::fake::FakeOAuth::new(&opts.user)),
        smtp_auth_disabled: Mutex::new(false),
        smtp_8bitmime: Mutex::new(true),
        jmap: Mutex::new(mail_core::jmap::fake::FakeJmapState::new(&opts.user, opts.jmap_push)),
        web: Mutex::new(vec![]),
        ai: Mutex::new(mail_core::ai::fake::FakeAi::default()),
        sieve: Mutex::new(mail_core::sieve::fake::FakeSieve::new(&opts.user, &opts.password)),
        carddav: Mutex::new(mail_core::carddav::fake::FakeCardDav::new(&opts.user, &opts.password)),
        pop3_sessions: Mutex::new(0),
        caldav: Mutex::new(mail_core::calendar::caldav::fake::FakeCalDav::new(&opts.user, &opts.password)),
        graph_calendar: Mutex::default(),
        peer: Mutex::new(peer::Peer::new()),
    });
    let (imap_l, imap) = listen(opts.imap_port).await?;
    let (smtp_l, smtp) = listen(opts.smtp_port).await?;
    let (control_l, control) = listen(opts.control_port).await?;
    let (pop3_l, pop3) = listen(opts.pop3_port).await?;
    let (sieve_l, sieve) = listen(opts.sieve_port).await?;
    serve(pop3_l, shared.clone(), Kind::Pop3, false);
    serve(sieve_l, shared.clone(), Kind::Sieve, false);
    *shared.ports.lock().unwrap_or_else(|e| e.into_inner()) = (imap, smtp);
    serve(imap_l, shared.clone(), Kind::Imap, false);
    serve(smtp_l, shared.clone(), Kind::Smtp, false);
    let (mut imaps, mut smtps) = (None, None);
    if opts.tls {
        let (l, p) = listen(0).await?;
        serve(l, shared.clone(), Kind::Imap, true);
        imaps = Some(p);
        let (l, p) = listen(0).await?;
        serve(l, shared.clone(), Kind::Smtp, true);
        smtps = Some(p);
    }
    *shared.control.lock().unwrap_or_else(|e| e.into_inner()) = control;
    control::serve(control_l, shared.clone());
    Ok(Running { imap, smtp, imaps, smtps, control, pop3, sieve, fingerprint, shared })
}

/// The TLS acceptor of a server, for STARTTLS.
pub(crate) fn acceptor(shared: &Shared) -> Option<TlsAcceptor> {
    shared.tls.clone()
}

/// Upgrades a plain connection (STARTTLS).
pub(crate) async fn upgrade(io: Io, shared: &Shared) -> Option<Io> {
    let acceptor = acceptor(shared)?;
    acceptor.accept(io).await.ok().map(|s| Box::new(s) as Io)
}

/// `16-Oct-2026 09:30:00 +0000` for Unix seconds.
pub(crate) fn internal_date(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil_from_days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!(
        "{:02}-{}-{:04} {:02}:{:02}:{:02} +0000",
        d,
        MONTHS[(m - 1) as usize],
        y,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn internal_dates_round_trip() {
        let s = super::internal_date(1_792_135_800);
        assert_eq!(s, "16-Oct-2026 07:30:00 +0000");
        assert_eq!(mail_core::imap::client::parse_internal_date(&s), Some(1_792_135_800));
    }
}
