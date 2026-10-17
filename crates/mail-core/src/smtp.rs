//! Sending: lettre speaks SMTP on a connection [`crate::network::connect_smtp`] opened (TLS on
//! 465, STARTTLS on 587, plain only to this computer). AUTH PLAIN or LOGIN with the account's
//! password, AUTH XOAUTH2 with an OAuth access token (Microsoft); the envelope comes from the
//! message's own sender and recipients.

use lettre::address::{Address as SmtpAddress, Envelope};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::AsyncSmtpConnection;
use lettre::transport::smtp::commands::{Data, Mail, Rcpt};
use lettre::transport::smtp::extension::{ClientId, Extension, MailBodyParameter, MailParameter};

use crate::account::{Credential, Security, Server};
use crate::error::{Error, Result};
use crate::network::{self, Problem, auth_problem};
use crate::trf;

fn problem(e: lettre::transport::smtp::Error) -> Error {
    if e.is_timeout() {
        return Problem::Timeout { host: String::new(), port: 0 }.into();
    }
    if e.is_permanent() || e.is_client() {
        return Error::Invalid(trf!(
            "Der Server hat die Nachricht abgelehnt: {e}",
            "The server refused the message: {e}"
        ));
    }
    // 4xx, a dropped or broken connection: worth another try later.
    Error::Network(e.to_string())
}

/// The answer to the message itself went missing (a timeout, a dropped connection): the server
/// had the whole message and may have delivered it.
fn unconfirmed(e: lettre::transport::smtp::Error) -> Error {
    if e.status().is_some() {
        return problem(e);
    }
    Error::Unconfirmed(trf!(
        "Die Verbindung brach ab, nachdem die Nachricht beim Server war, und vor seiner Antwort ({e}). Vielleicht ist sie schon angekommen: Prüfe das, bevor du sie erneut sendest.",
        "The connection broke after the message reached the server and before its answer ({e}). It may have arrived already: check before you send it again."
    ))
}

async fn connect(server: &Server, username: &str, credential: &Credential) -> Result<AsyncSmtpConnection> {
    let stream = network::connect_smtp(server).await?;
    let hello = ClientId::Domain(network::hello_name().into());
    let mut conn = AsyncSmtpConnection::connect_with_transport(Box::new(stream), &hello).await.map_err(problem)?;
    let refused = |e: lettre::transport::smtp::Error| {
        if e.is_permanent() { Error::Connection(auth_problem(&e.to_string())) } else { problem(e) }
    };
    match credential {
        Credential::Password(password) => {
            // A local bridge without AUTH takes mail as it is; everyone else signs in.
            let offers_auth = conn.server_info().supports_auth_mechanism(Mechanism::Plain)
                || conn.server_info().supports_auth_mechanism(Mechanism::Login);
            if offers_auth || server.security != Security::Plain {
                conn.auth(&[Mechanism::Plain, Mechanism::Login], &Credentials::new(username.into(), password.into()))
                    .await
                    .map_err(refused)?;
            }
        }
        Credential::XOAuth2(token) => {
            conn.auth(&[Mechanism::Xoauth2], &Credentials::new(username.into(), token.into()))
                .await
                .map_err(refused)?;
        }
    }
    Ok(conn)
}

/// Signs in and leaves again (the setup's check).
pub async fn check(server: &Server, username: &str, password: &str) -> Result<()> {
    check_with(server, username, &Credential::Password(password.into())).await
}

pub async fn check_with(server: &Server, username: &str, credential: &Credential) -> Result<()> {
    let mut conn = connect(server, username, credential).await?;
    let _ = conn.quit().await;
    Ok(())
}

fn address(s: &str) -> Result<SmtpAddress> {
    s.trim()
        .parse()
        .map_err(|_| Error::Invalid(trf!("„{s}“ ist keine gültige Adresse.", "“{s}” is not a valid address.")))
}

/// Sends a finished message (`raw`, RFC 5322) from `from` to `to` (everyone: To, Cc and Bcc).
pub async fn send(
    server: &Server,
    username: &str,
    password: &str,
    from: &str,
    to: &[String],
    raw: &[u8],
) -> Result<()> {
    send_with(server, username, &Credential::Password(password.into()), from, to, raw).await
}

/// Sends with a password or an OAuth access token.
pub async fn send_with(
    server: &Server,
    username: &str,
    credential: &Credential,
    from: &str,
    to: &[String],
    raw: &[u8],
) -> Result<()> {
    let rcpt = to.iter().map(|t| address(t)).collect::<Result<Vec<_>>>()?;
    let envelope = Envelope::new(Some(address(from)?), rcpt).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut conn = connect(server, username, credential).await?;
    let info = conn.server_info();
    // Addresses with other characters need SMTPUTF8 (and lettre then wants 8BITMIME as well).
    let utf8 = info.supports_feature(Extension::SmtpUtfEight) && info.supports_feature(Extension::EightBitMime);
    if let Some(a) = std::iter::once(from).chain(to.iter().map(String::as_str)).find(|a| !a.is_ascii())
        && !utf8
    {
        let _ = conn.quit().await;
        return Err(crate::sevenbit::needs_smtputf8(a.trim()));
    }
    // A server without 8BITMIME takes only 7-bit messages.
    let raw = if raw.is_ascii() || info.supports_feature(Extension::EightBitMime) {
        std::borrow::Cow::Borrowed(raw)
    } else {
        match crate::sevenbit::downgrade(raw) {
            Ok(r) => std::borrow::Cow::Owned(r),
            Err(e) => {
                let _ = conn.quit().await;
                return Err(e);
            }
        }
    };
    // The steps of `send` one by one: up to `DATA` a failure means nothing went out; after the
    // message only its answer says whether it did.
    let mut params = vec![];
    if !from.is_ascii() || to.iter().any(|t| !t.is_ascii()) {
        params.push(MailParameter::SmtpUtfEight);
    }
    if !raw.is_ascii() {
        params.push(MailParameter::Body(MailBodyParameter::EightBitMime));
    }
    let before = async {
        conn.command(Mail::new(envelope.from().cloned(), params)).await?;
        for t in envelope.to() {
            conn.command(Rcpt::new(t.clone(), vec![])).await?;
        }
        conn.command(Data).await
    };
    if let Err(e) = before.await {
        conn.abort().await;
        return Err(problem(e));
    }
    if let Err(e) = conn.message(&raw).await {
        conn.abort().await;
        return Err(unconfirmed(e));
    }
    let _ = conn.quit().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::*;

    /// Where the test server drops the connection.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Cut {
        /// At the `DATA` command: the message never went over.
        AtData,
        /// After the whole message, instead of the reply: closed.
        AfterMessage,
        /// After the whole message: reset (RST).
        ResetAfterMessage,
    }

    /// An SMTP server on loopback that drops the connection at `cut`.
    async fn dropping(cut: Cut) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut io = BufReader::new(sock);
            io.get_mut().write_all(b"220 test ESMTP\r\n").await.unwrap();
            loop {
                let mut line = String::new();
                if io.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let cmd = line.trim_end().to_ascii_uppercase();
                if cmd.starts_with("EHLO") {
                    io.get_mut().write_all(b"250-test\r\n250 8BITMIME\r\n").await.unwrap();
                } else if cmd == "DATA" {
                    if cut == Cut::AtData {
                        return;
                    }
                    io.get_mut().write_all(b"354 go on\r\n").await.unwrap();
                    let mut body = String::new();
                    loop {
                        let mut l = String::new();
                        if io.read_line(&mut l).await.unwrap_or(0) == 0 {
                            return;
                        }
                        body.push_str(&l);
                        if body.ends_with("\r\n.\r\n") {
                            // Taken (and maybe delivered), but the reply never comes.
                            if cut == Cut::ResetAfterMessage {
                                io.get_ref().set_zero_linger().unwrap();
                            }
                            return;
                        }
                    }
                } else {
                    io.get_mut().write_all(b"250 ok\r\n").await.unwrap();
                }
            }
        });
        port
    }

    /// The connection broke after the whole message went over: it may have been delivered, so
    /// it is not simply sent again later. Broken before: another try is fine.
    #[tokio::test]
    async fn a_lost_reply_after_the_message_is_no_reason_to_send_it_again() {
        let raw =
            b"From: maria@example.org\r\nTo: jonas@example.org\r\nSubject: x\r\nMessage-ID: <q@x>\r\n\r\nHallo\r\n";
        let to = vec!["jonas@example.org".to_string()];
        for cut in [Cut::AfterMessage, Cut::ResetAfterMessage] {
            let server = Server::new("127.0.0.1", dropping(cut).await, Security::Plain);
            let e = send(&server, "maria@example.org", "pw", "maria@example.org", &to, raw).await.unwrap_err();
            assert!(!crate::engine::is_transient(&e) && unconfirmed(&e), "{e:?}");
        }
        let server = Server::new("127.0.0.1", dropping(Cut::AtData).await, Security::Plain);
        let e = send(&server, "maria@example.org", "pw", "maria@example.org", &to, raw).await.unwrap_err();
        assert!(crate::engine::is_transient(&e), "{e:?}");
    }

    fn unconfirmed(e: &Error) -> bool {
        matches!(e, Error::Unconfirmed(_))
    }
}
