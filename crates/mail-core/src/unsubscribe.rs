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
use serde::{Deserialize, Serialize};

use crate::compose::Draft;
use crate::error::{Error, Result};
use crate::network::{self, Service};
use crate::store::{Store, now};
use crate::{tr, trf};

/// One way out of a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// A `mailto:` address with one recipient (RFC 6068), its subject and body.
fn mailto(url: &url::Url) -> Option<Method> {
    let to = decode(url.path()).trim().to_string();
    if to.matches('@').count() != 1 || to.contains([',', ';', '<', '>', '"', ' ']) {
        return None;
    }
    let mut subject = String::new();
    let mut body = String::new();
    for pair in url.query().unwrap_or_default().split('&') {
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

/// The addresses of a `List-Unsubscribe` header in the order sent: each one inside its angle
/// brackets, whitespace in it ignored (folding), read with the URL parser.
pub fn methods(header: &str, one_click: bool) -> Vec<Method> {
    let mut out = vec![];
    for part in header.split('<').skip(1) {
        let Some(inner) = part.split_once('>').map(|(a, _)| a) else { continue };
        let addr: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
        if addr.contains('\\') {
            continue;
        }
        let Ok(url) = url::Url::parse(&addr) else { continue };
        match url.scheme() {
            "mailto" => out.extend(mailto(&url)),
            "https" | "http" => {
                // The POST only to a public `https://` address; a page (opened in the browser
                // once the user confirmed it) also over http, never on the local network.
                if one_click && network::check_mail_url(&addr, false).is_ok() {
                    out.push(Method::OneClick { url: addr });
                } else if network::check_mail_url(&addr, true).is_ok() {
                    out.push(Method::Link { url: addr });
                }
            }
            _ => {}
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

/// What „Abbestellen“ on one message would do: shown to the user, and done only as shown
/// (the backend reads the same message again and compares).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub message_id: i64,
    pub account_id: i64,
    pub sender: String,
    pub method: Method,
}

fn no_way_out() -> Error {
    Error::NotFound(
        tr!("Für diesen Absender gibt es keine Abmeldeadresse.", "This sender gives no way to unsubscribe.").into(),
    )
}

/// The way out of the list one message offers (its own `List-Unsubscribe`, never another
/// message's of the same sender).
pub fn plan(store: &Store, message_id: i64) -> Result<Plan> {
    let (account_id, sender, header, one_click): (i64, Option<String>, Option<String>, bool) = store
        .conn()
        .query_row(
            "SELECT account_id, from_email, unsubscribe, one_click FROM messages WHERE id = ?1",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| no_way_out())?;
    let sender = sender.unwrap_or_default().trim().to_ascii_lowercase();
    let method = header.and_then(|h| best(&h, one_click)).ok_or_else(no_way_out)?;
    Ok(Plan { message_id, account_id, sender, method })
}

/// The plan of `message_id` when it is still `confirmed` (what the user saw and agreed to).
pub fn confirmed_plan(store: &Store, message_id: i64, confirmed: &Method) -> Result<Plan> {
    let p = plan(store, message_id)?;
    if p.method != *confirmed {
        return Err(Error::Invalid(
            tr!(
                "Die Abmeldung ist nicht mehr die, die du bestätigt hast. Versuch es noch einmal.",
                "The way to unsubscribe is not the one you confirmed. Try again."
            )
            .into(),
        ));
    }
    Ok(p)
}

/// Sends the one-click request (RFC 8058, section 3.1).
pub async fn post_one_click(url: &str) -> Result<()> {
    network::check_mail_url(url, false)?;
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

/// When the user unsubscribed from a sender, if they did.
pub fn unsubscribed_at(store: &Store, sender: &str) -> Result<Option<i64>> {
    use rusqlite::OptionalExtension;
    Ok(store
        .conn()
        .query_row("SELECT at FROM unsubscribes WHERE sender = ?1", [sender.trim()], |r| r.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ways_out_in_order() {
        network::allow_loopback_for_tests();
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
        assert_eq!(
            best("<http://127.0.0.1:8025/u>", true).map(|m| m.kind()),
            Some("one_click"),
            "this computer (tests)"
        );
        assert_eq!(best("<ftp://x>, garbage", true), None);
        assert_eq!(mailto_draft(1, "a@b.example", "unsubscribe", "").text, "unsubscribe");
        let (store, _, _) = crate::store::tests::store_with_inbox();
        assert_eq!(unsubscribed_at(&store, "news@x.example").unwrap(), None);
        record(&store, "News@X.example", "mailto").unwrap();
        assert!(unsubscribed_at(&store, "news@x.example").unwrap().is_some());
    }

    #[test]
    fn the_header_is_read_as_addresses() {
        // A space after `<` is still the mailto address; it is never sent without the user
        // seeing it (the plan), and a list of recipients is no unsubscribe address.
        assert_eq!(
            best("< mailto:premium@evil.example?subject=SUBSCRIBE%20ALL&body=yes>", true),
            Some(Method::Mailto { to: "premium@evil.example".into(), subject: "SUBSCRIBE ALL".into(), body: "yes".into() })
        );
        assert_eq!(best("<mailto:a@x.example,b@y.example>", false), None);
        assert_eq!(best("<mailto:a@x.example?cc=b@y.example>", false).map(|m| m.kind()), Some("mailto"));
        // Plain HTTP to another host hidden behind a backslash, local network targets: no POST.
        assert_eq!(best("<http://evil.example\\@127.0.0.1/unsub>", true), None);
        assert_eq!(best("<https://192.168.1.1/unsub>", true), None);
        assert_eq!(best("<https://user:pw@news.example/u>", true), None);
        assert_eq!(
            best("<https://news.example/u?id=7>", true),
            Some(Method::OneClick { url: "https://news.example/u?id=7".into() })
        );
    }

    #[test]
    fn the_plan_is_the_viewed_messages() {
        let (store, _, inbox) = crate::store::tests::store_with_inbox();
        let raw = |date: &str, header: &str| {
            format!("From: News <news@x.example>\r\nTo: me@example.org\r\nSubject: Ausgabe\r\nDate: {date}\r\nList-Unsubscribe: {header}\r\n\r\nText\r\n")
        };
        let real = raw("Mon, 1 Jun 2026 10:00:00 +0000", "<https://x.example/u?id=1>");
        let p = crate::parse::parse(real.as_bytes()).unwrap();
        let id = store.insert_message(inbox, &crate::store::tests::header(1, real.as_bytes()), &p).unwrap();
        // A forged message of the same sender, dated in the future, says something else.
        let forged = raw("Fri, 1 Jan 2100 10:00:00 +0000", "<mailto:premium@evil.example?subject=JOIN>");
        let p = crate::parse::parse(forged.as_bytes()).unwrap();
        store.insert_message(inbox, &crate::store::tests::header(2, forged.as_bytes()), &p).unwrap();
        let plan = plan(&store, id).unwrap();
        assert_eq!(plan.sender, "news@x.example");
        assert_eq!(plan.method, Method::Link { url: "https://x.example/u?id=1".into() });
    }

    #[tokio::test]
    async fn one_click_posts_the_form_and_reads_the_answer() {
        network::allow_loopback_for_tests();
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
