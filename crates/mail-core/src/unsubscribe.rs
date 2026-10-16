//! Unsubscribing from a newsletter with the address its `List-Unsubscribe` header gives
//! (RFC 2369), in this order:
//!
//! 1. **One click** (RFC 8058): with `List-Unsubscribe-Post: List-Unsubscribe=One-Click` and an
//!    `https://` address, one POST with `List-Unsubscribe=One-Click` as its body, through
//!    [`network::client_for`] (no cookies, no referrer, nothing else sent).
//! 2. **E-mail**: a `mailto:` address gets a message from the user's account, through the
//!    offline queue like any other.
//! 3. **Link**: otherwise the address opens in the browser after the user confirmed it.
//!
//! Only the user starts this; the app never calls an unsubscribe address by itself.

use rusqlite::params;
use serde::Serialize;

use crate::compose::Draft;
use crate::error::{Error, Result};
use crate::network::{self, Service};
use crate::store::{Store, now};
use crate::{tr, trf};

/// One way out of a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Method {
    /// RFC 8058: a POST to this `https://` address.
    OneClick { url: String },
    /// A message to this address.
    Mailto { to: String, subject: String, body: String },
    /// A page to open in the browser.
    Link { url: String },
}

impl Method {
    pub fn kind(&self) -> &'static str {
        match self {
            Method::OneClick { .. } => "one_click",
            Method::Mailto { .. } => "mailto",
            Method::Link { .. } => "link",
        }
    }
}

/// The percent-decoded value of a `mailto:` field.
fn decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn mailto(addr: &str) -> Option<Method> {
    let rest = addr.get(7..)?;
    let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
    let to = decode(to);
    if !to.contains('@') {
        return None;
    }
    let mut subject = String::new();
    let mut body = String::new();
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k.to_ascii_lowercase().as_str() {
            "subject" => subject = decode(v),
            "body" => body = decode(v),
            _ => {}
        }
    }
    if subject.trim().is_empty() {
        subject = "unsubscribe".into();
    }
    Some(Method::Mailto { to, subject, body })
}

/// The addresses of a `List-Unsubscribe` header, in the order sent.
pub fn methods(header: &str, one_click: bool) -> Vec<Method> {
    let mut out = vec![];
    for part in header.split('<').skip(1) {
        let Some(addr) = part.split('>').next().map(str::trim) else { continue };
        let lower = addr.to_ascii_lowercase();
        if lower.starts_with("mailto:") {
            out.extend(mailto(addr));
        } else if lower.starts_with("https://") {
            out.push(if one_click { Method::OneClick { url: addr.into() } } else { Method::Link { url: addr.into() } });
        } else if lower.starts_with("http://") {
            out.push(Method::Link { url: addr.into() });
        }
    }
    out
}

/// The best way out: one click, else e-mail, else the link.
pub fn best(header: &str, one_click: bool) -> Option<Method> {
    let all = methods(header, one_click);
    let pick = |kind: &str| all.iter().find(|m| m.kind() == kind).cloned();
    pick("one_click").or_else(|| pick("mailto")).or_else(|| pick("link"))
}

/// Sends the one-click request (RFC 8058, section 3.1).
pub async fn post_one_click(url: &str) -> Result<()> {
    network::check_https_url(url)?;
    let client = network::client_for(Service::Unsubscribe)?;
    let r = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("List-Unsubscribe=One-Click")
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = r.status();
    if status.is_success() {
        return Ok(());
    }
    Err(Error::Network(trf!(
        "Der Absender hat die Abmeldung nicht angenommen (HTTP {status}).",
        "The sender did not accept the unsubscribe request (HTTP {status}).",
        status = status.as_u16()
    )))
}

/// The newest unsubscribe header of a sender: the account it came to, the header and whether it
/// takes one click.
pub fn latest(store: &Store, sender: &str) -> Result<(i64, String, bool)> {
    store
        .conn()
        .query_row(
            "SELECT m.account_id, m.unsubscribe, m.one_click FROM messages m
             WHERE m.from_email = ?1 COLLATE NOCASE AND m.unsubscribe IS NOT NULL
             ORDER BY COALESCE(m.date, m.internal_date) DESC, m.id DESC LIMIT 1",
            [sender.trim()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| {
            Error::NotFound(
                tr!("Für diesen Absender gibt es keine Abmeldeadresse.", "This sender gives no way to unsubscribe.")
                    .into(),
            )
        })
}

/// The message that unsubscribes by e-mail, from the account the newsletter came to.
pub fn mailto_draft(account_id: i64, to: &str, subject: &str, body: &str) -> Draft {
    Draft {
        account_id,
        to: vec![to.to_string()],
        subject: subject.to_string(),
        text: if body.trim().is_empty() { "unsubscribe".into() } else { body.to_string() },
        ..Draft::default()
    }
}

/// Notes that the user unsubscribed from a sender.
pub fn record(store: &Store, sender: &str, method: &str) -> Result<()> {
    store.conn().execute(
        "INSERT INTO unsubscribes (sender, method, at) VALUES (?1, ?2, ?3)
         ON CONFLICT (sender) DO UPDATE SET method = excluded.method, at = excluded.at",
        params![sender.trim(), method, now()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ways_out_in_order() {
        let h = "<mailto:leave@news.example?subject=Abmelden%20bitte&body=raus>, <https://news.example/u?id=7>";
        assert_eq!(
            best(h, true),
            Some(Method::OneClick { url: "https://news.example/u?id=7".into() }),
            "one click comes first"
        );
        assert_eq!(
            best(h, false),
            Some(Method::Mailto {
                to: "leave@news.example".into(),
                subject: "Abmelden bitte".into(),
                body: "raus".into()
            })
        );
        assert_eq!(
            best("<https://news.example/u>", false),
            Some(Method::Link { url: "https://news.example/u".into() })
        );
        assert_eq!(
            best("<mailto:aus@verein.example>", true),
            Some(Method::Mailto {
                to: "aus@verein.example".into(),
                subject: "unsubscribe".into(),
                body: String::new()
            })
        );
        assert_eq!(best("<http://news.example/u>", true).map(|m| m.kind()), Some("link"), "no POST over http");
        assert_eq!(best("<ftp://x>, garbage", true), None);
        assert_eq!(mailto_draft(1, "a@b.example", "unsubscribe", "").text, "unsubscribe");
    }

    #[tokio::test]
    async fn one_click_posts_the_form_and_reads_the_answer() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut got = vec![];
            for status in ["200 OK", "404 Not Found"] {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let mut req = String::new();
                while !req.contains("One-Click") {
                    let n = s.read(&mut buf).await.unwrap();
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                got.push(req);
                let resp = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                s.write_all(resp.as_bytes()).await.unwrap();
            }
            got
        });
        let url = format!("http://127.0.0.1:{port}/unsubscribe?id=7");
        post_one_click(&url).await.unwrap();
        assert!(post_one_click(&url).await.is_err(), "a refusal is an error");
        let got = server.await.unwrap();
        assert!(got[0].starts_with("POST /unsubscribe?id=7 "), "{}", got[0]);
        assert!(got[0].to_lowercase().contains("content-type: application/x-www-form-urlencoded"));
        assert!(got[0].ends_with("List-Unsubscribe=One-Click"));
        assert!(post_one_click("http://news.example/u").await.is_err(), "plain http only on this computer");
    }
}
