//! Sending: lettre speaks SMTP on a connection [`crate::network::connect_smtp`] opened (TLS on
//! 465, STARTTLS on 587, plain only to this computer). AUTH PLAIN or LOGIN with the account's
//! password; the envelope comes from the message's own sender and recipients.

use lettre::address::{Address as SmtpAddress, Envelope};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::AsyncSmtpConnection;
use lettre::transport::smtp::extension::ClientId;

use crate::account::{Security, Server};
use crate::error::{Error, Result};
use crate::network::{self, Problem, auth_problem};
use crate::trf;

fn problem(e: lettre::transport::smtp::Error) -> Error {
    if e.is_timeout() {
        return Problem::Timeout { host: String::new(), port: 0 }.into();
    }
    if e.is_transient() || e.is_transport_shutdown() || !e.is_response() {
        // 4xx, a dropped connection: worth another try later.
        return Error::Network(e.to_string());
    }
    Error::Invalid(trf!("Der Server hat die Nachricht abgelehnt: {e}", "The server refused the message: {e}"))
}

async fn connect(server: &Server, username: &str, password: &str) -> Result<AsyncSmtpConnection> {
    let stream = network::connect_smtp(server).await?;
    let hello = ClientId::Domain(network::hello_name().into());
    let mut conn = AsyncSmtpConnection::connect_with_transport(Box::new(stream), &hello).await.map_err(problem)?;
    // A local bridge without AUTH takes mail as it is; everyone else signs in.
    let offers_auth = conn.server_info().supports_auth_mechanism(Mechanism::Plain)
        || conn.server_info().supports_auth_mechanism(Mechanism::Login);
    if offers_auth || server.security != Security::Plain {
        conn.auth(&[Mechanism::Plain, Mechanism::Login], &Credentials::new(username.into(), password.into()))
            .await
            .map_err(|e| if e.is_permanent() { Error::Connection(auth_problem(&e.to_string())) } else { problem(e) })?;
    }
    Ok(conn)
}

/// Signs in and leaves again (the setup's check).
pub async fn check(server: &Server, username: &str, password: &str) -> Result<()> {
    let mut conn = connect(server, username, password).await?;
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
    let rcpt = to.iter().map(|t| address(t)).collect::<Result<Vec<_>>>()?;
    let envelope = Envelope::new(Some(address(from)?), rcpt).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut conn = connect(server, username, password).await?;
    conn.send(&envelope, raw).await.map_err(problem)?;
    let _ = conn.quit().await;
    Ok(())
}
