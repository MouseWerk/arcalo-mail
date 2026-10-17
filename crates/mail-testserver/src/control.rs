//! The control port (HTTP on loopback) for tests and the screenshot generator:
//!
//! * `GET /state`: the folders with their messages (UID, subject, sender, flags)
//! * `GET /sent`: what the SMTP server took; `GET /sent/raw?i=0`: one message as sent
//! * `POST /deliver?mailbox=INBOX&seen=1&flagged=1&date=<unix>` with the message as the body
//! * `POST /flags?mailbox=INBOX&uid=3&seen=1&flagged=0`: as another client would
//! * `POST /expunge?mailbox=INBOX&uid=3`
//! * `POST /offline`, `POST /online`: refuse and drop every connection, or take them again
//! * `POST /fail?cmd=MOVE`: the next command of this kind fails with `NO`
//! * `POST /password?value=…`: the account's password changes (a refused login)
//! * `POST /folder?name=…`: a new folder
//! * `POST /delay?ms=…`: new connections wait this long before the greeting (a slow server)
//! * `GET /autoconfig?host=127.0.0.1`: an autoconfig file (config-v1.1) for this server, so the
//!   app's setup finds it as it would a provider's
//! * `GET /oauth/authorize`, `POST /oauth/token`: an identity provider (Microsoft's endpoints as
//!   far as the app uses them) whose access tokens IMAP and SMTP take with XOAUTH2;
//!   `POST /oauth/fail?mode=admin_consent|access_denied[&at=token]`, `POST /oauth/revoke`,
//!   `POST /oauth/expire`, `GET /oauth/state` (token requests so far)
//! * `POST /smtp-auth?disabled=1`: SMTP refuses XOAUTH2 with `535 5.7.139`, as Exchange Online
//!   does when SMTP AUTH is off for the mailbox
//! * `POST /smtp-8bitmime?offered=0`: SMTP offers only AUTH and SMTPUTF8 (as GreenMail) and
//!   refuses 8-bit messages
//! * `GET /.well-known/jmap`, `/jmap/…`: the same mailboxes over JMAP (`jmap.rs`)
//! * `/unsubscribe…`, `/track…`, `/img…` (any method): a sender's web side, as a newsletter's
//!   one-click unsubscribe address (RFC 8058), its tracking pixel or its pictures; every request
//!   is noted (pictures get a 1×1 GIF) and `GET /web` lists them
//! * `/ai/v1/…`: an OpenAI-compatible AI provider (`mail_core::ai::fake`), streamed piece by
//!   piece; `GET /ai-requests` lists what it was sent, `POST /ai-answer?task=summary` (the text
//!   as the body) sets an answer, `POST /ai-fail?status=500` fails the next one,
//!   `POST /ai-key?value=…` asks for a key
//! * `/dav/…`: a CardDAV address book with the account's user name and password;
//!   `POST /dav-card?name=lea` (a vCard as the body) adds a card
//! * `GET /sieve`: the ManageSieve server's scripts and the active one
//! * `/caldav/…`, `/.well-known/caldav`: two CalDAV calendars (`work`, `home`) with the account's
//!   user name and password; `POST /caldav-event?calendar=work&name=a.ics` (iCalendar as the body)
//!   puts an appointment as another client would, `GET /caldav-state` lists the objects and the
//!   writes the app made
//! * `/graph/v1.0/…`: Microsoft Graph's calendar endpoints (`calendar::graph::fake`) for an
//!   account signed in through the identity provider; `POST /graph-event?calendar=C1` (Graph's
//!   event JSON) puts one
//! * `GET /pgp/protected?email=…`: a new secret key for that address, protected with the
//!   passphrase `geheim` (as GnuPG exports keys)
//! * `GET /pgp/public`: Jonas's OpenPGP key; `GET /pgp/sent?i=0`: what Jonas reads in sent
//!   message 0; `POST /pgp/deliver?subject=…&date=…` (text as the body): a signed and encrypted
//!   message from Jonas into the inbox

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::Shared;

fn decode(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(())
                {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query(path: &str) -> (String, BTreeMap<String, String>) {
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let map = q
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect();
    (p.to_string(), map)
}

fn flag(q: &BTreeMap<String, String>, k: &str) -> Option<bool> {
    q.get(k).map(|v| v == "1" || v == "true")
}

fn state(shared: &Shared) -> serde_json::Value {
    let f = shared.fake.lock().unwrap();
    let mut boxes = serde_json::Map::new();
    for (name, m) in &f.mailboxes {
        let list: Vec<serde_json::Value> = m
            .messages
            .iter()
            .map(|(uid, msg)| {
                let p = mail_core::parse::parse(&msg.raw).unwrap_or_default();
                json!({
                    "uid": uid,
                    "subject": p.subject,
                    "from": p.from.map(|a| a.email),
                    "message_id": p.message_id,
                    "seen": msg.flags.seen,
                    "flagged": msg.flags.flagged,
                    "answered": msg.flags.answered,
                    "draft": msg.flags.draft,
                    "deleted": msg.flags.deleted,
                    "keywords": msg.flags.keywords,
                })
            })
            .collect();
        boxes.insert(name.clone(), serde_json::Value::Array(list));
    }
    json!({ "mailboxes": boxes, "offline": shared.is_offline(), "connections": *shared.connections.lock().unwrap() })
}

fn autoconfig(host: &str, imap: u16, smtp: u16) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="example.org">
    <domain>example.org</domain>
    <displayName>Example Mail</displayName>
    <incomingServer type="imap">
      <hostname>{host}</hostname><port>{imap}</port><socketType>plain</socketType>
      <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>{host}</hostname><port>{smtp}</port><socketType>plain</socketType>
      <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication>
    </outgoingServer>
  </emailProvider>
</clientConfig>
"#
    )
}

/// A transparent 1×1 GIF.
const GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff,
    0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02,
    0x02, 0x44, 0x01, 0x00, 0x3b,
];

async fn handle(stream: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
    let mut r = BufReader::new(stream);
    let mut first = String::new();
    r.read_line(&mut first).await?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut length = 0usize;
    let mut headers: Vec<(String, String)> = vec![];
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).await? == 0 || h.trim().is_empty() {
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
    r.read_exact(&mut body).await?;
    let (path, q) = query(&target);
    // The identity provider: `GET /oauth/authorize` signs in at once, `POST /oauth/token`.
    if path == "/oauth/authorize" || path == "/oauth/token" {
        let reply = shared.oauth.lock().unwrap().handle(&method, &target, &body);
        let mut stream = r.into_inner();
        let location = reply.location.map(|l| format!("Location: {l}\r\n")).unwrap_or_default();
        let head = format!(
            "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n",
            reply.status,
            reply.content_type,
            reply.body.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&reply.body).await?;
        return stream.flush().await;
    }
    if ["/unsubscribe", "/track", "/img"].iter().any(|p| path.starts_with(p)) {
        shared.web.lock().unwrap().push(crate::WebRequest {
            method: method.clone(),
            path: target.clone(),
            content_type: headers.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.clone()),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
        let (ctype, out): (&str, Vec<u8>) = if path.starts_with("/unsubscribe") {
            ("text/plain", b"unsubscribed".to_vec())
        } else {
            ("image/gif", GIF.to_vec())
        };
        let mut stream = r.into_inner();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            out.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&out).await?;
        return stream.flush().await;
    }
    if path.starts_with("/ai/") {
        let reply = shared.ai.lock().unwrap().handle(&method, &target, &headers, &body);
        let mut stream = r.into_inner();
        let head =
            format!("HTTP/1.1 {} X\r\nContent-Type: {}\r\nConnection: close\r\n\r\n", reply.status, reply.content_type);
        stream.write_all(head.as_bytes()).await?;
        for c in reply.chunks {
            stream.write_all(&c).await?;
            stream.flush().await?;
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        return stream.flush().await;
    }
    if path.starts_with("/dav") && method != "POST" {
        let auth = headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.clone());
        let (status, text) =
            shared.carddav.lock().unwrap().handle(&method, &target, auth.as_deref(), &String::from_utf8_lossy(&body));
        let mut stream = r.into_inner();
        let head = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            text.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(text.as_bytes()).await?;
        return stream.flush().await;
    }
    if path == "/caldav" || path.starts_with("/caldav/") || path == "/.well-known/caldav" {
        let auth = headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.clone());
        let (status, extra, text) = shared.caldav.lock().unwrap().handle(
            &method,
            &target,
            auth.as_deref(),
            &headers,
            &String::from_utf8_lossy(&body),
        );
        let mut stream = r.into_inner();
        let more: String = extra.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        let head = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/xml; charset=utf-8\r\nContent-Length: {}\r\n{more}Connection: close\r\n\r\n",
            text.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(text.as_bytes()).await?;
        return stream.flush().await;
    }
    if path.starts_with("/graph/v1.0/") {
        let authorized = headers.iter().any(|(k, v)| k == "authorization" && v.starts_with("Bearer "));
        let base = format!("http://127.0.0.1:{}/graph/v1.0", *shared.control.lock().unwrap());
        let (status, text) = if authorized {
            shared.graph_calendar.lock().unwrap().handle(
                &method,
                target.trim_start_matches("/graph"),
                &headers,
                &body,
                &base,
            )
        } else {
            (401, "{}".into())
        };
        let mut stream = r.into_inner();
        let head = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            text.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(text.as_bytes()).await?;
        return stream.flush().await;
    }
    if path.starts_with("/jmap") || path == "/.well-known/jmap" {
        return crate::jmap::handle(r, &method, &target, &headers, body, shared).await;
    }
    let (status, ctype, out): (&str, &str, Vec<u8>) = match (method.as_str(), path.as_str()) {
        ("POST", "/oauth/fail") => {
            let mode = q.get("mode").cloned().unwrap_or_default();
            let mut o = shared.oauth.lock().unwrap();
            match q.get("at").map(String::as_str) {
                Some("token") => o.token_error = Some(mode).filter(|m| !m.is_empty()),
                _ => o.next_error = Some(mode).filter(|m| !m.is_empty()),
            }
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/oauth/revoke") => {
            shared.oauth.lock().unwrap().revoke_all();
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/oauth/expire") => {
            shared.oauth.lock().unwrap().expire_access();
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("GET", "/oauth/state") => {
            let n = shared.oauth.lock().unwrap().token_requests;
            ("200 OK", "application/json", json!({ "token_requests": n }).to_string().into_bytes())
        }
        ("POST", "/smtp-auth") => {
            *shared.smtp_auth_disabled.lock().unwrap() = flag(&q, "disabled").unwrap_or(false);
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/smtp-8bitmime") => {
            *shared.smtp_8bitmime.lock().unwrap() = flag(&q, "offered").unwrap_or(true);
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("GET", "/state") => ("200 OK", "application/json", state(&shared).to_string().into_bytes()),
        ("GET", "/ai-requests") => {
            let list = shared.ai.lock().unwrap().requests.clone();
            ("200 OK", "application/json", serde_json::to_vec(&list).unwrap_or_default())
        }
        ("POST", "/ai-answer") => {
            let task = q.get("task").cloned().unwrap_or_default();
            shared.ai.lock().unwrap().answers.insert(task, String::from_utf8_lossy(&body).into_owned());
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/ai-fail") => {
            shared.ai.lock().unwrap().fail = q.get("status").and_then(|v| v.parse().ok());
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/ai-key") => {
            shared.ai.lock().unwrap().key = q.get("value").cloned().filter(|v| !v.is_empty());
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/dav-card") => {
            let name = q.get("name").cloned().unwrap_or_else(|| "card".into());
            shared.carddav.lock().unwrap().put(&name, &String::from_utf8_lossy(&body));
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/caldav-event") => {
            let cal =
                format!("/caldav/calendars/user/{}/", q.get("calendar").cloned().unwrap_or_else(|| "work".into()));
            let name = q.get("name").cloned().unwrap_or_else(|| "event.ics".into());
            let mut c = shared.caldav.lock().unwrap();
            if c.calendars.contains_key(&cal) {
                let href = c.put(&cal, &name, &String::from_utf8_lossy(&body));
                ("200 OK", "application/json", json!({ "href": href }).to_string().into_bytes())
            } else {
                ("404 Not Found", "text/plain", b"no such calendar".to_vec())
            }
        }
        ("GET", "/caldav-state") => {
            let c = shared.caldav.lock().unwrap();
            let calendars: serde_json::Map<String, serde_json::Value> = c
                .calendars
                .iter()
                .map(|(k, cal)| {
                    let objects: serde_json::Map<String, serde_json::Value> =
                        cal.objects.iter().map(|(h, (_, d))| (h.clone(), json!(d))).collect();
                    (k.clone(), serde_json::Value::Object(objects))
                })
                .collect();
            let writes: Vec<serde_json::Value> = c.writes.iter().map(|(m, p)| json!([m, p])).collect();
            ("200 OK", "application/json", json!({ "calendars": calendars, "writes": writes }).to_string().into_bytes())
        }
        ("POST", "/graph-event") => {
            let cal = q.get("calendar").cloned().unwrap_or_else(|| "C1".into());
            match serde_json::from_slice::<serde_json::Value>(&body) {
                Ok(e) => {
                    let id = shared.graph_calendar.lock().unwrap().put(&cal, e);
                    ("200 OK", "application/json", json!({ "id": id }).to_string().into_bytes())
                }
                Err(e) => ("400 Bad Request", "text/plain", e.to_string().into_bytes()),
            }
        }
        ("GET", "/pgp/public") => ("200 OK", "text/plain", shared.peer.lock().unwrap().public().as_bytes().to_vec()),
        ("GET", "/pgp/protected") => {
            let email = q.get("email").cloned().unwrap_or_else(|| shared.user.clone());
            match mail_core::crypto::pgp::generate_with("", &email, Some("geheim")) {
                Ok((secret, _)) => ("200 OK", "text/plain", secret.into_bytes()),
                Err(e) => ("500 Internal Server Error", "text/plain", e.to_string().into_bytes()),
            }
        }
        ("GET", "/pgp/sent") => {
            let i: usize = q.get("i").and_then(|v| v.parse().ok()).unwrap_or(0);
            let sent = shared.sent.lock().unwrap().get(i).cloned();
            match sent {
                Some(m) => {
                    let v = shared.peer.lock().unwrap().read(&m.raw, &m.from);
                    ("200 OK", "application/json", v.to_string().into_bytes())
                }
                None => ("404 Not Found", "text/plain", b"no such message".to_vec()),
            }
        }
        ("POST", "/pgp/deliver") => {
            let subject = q.get("subject").cloned().unwrap_or_else(|| "Verschlüsselt".into());
            let sent: Vec<(String, Vec<u8>)> =
                shared.sent.lock().unwrap().iter().map(|m| (m.from.clone(), m.raw.clone())).collect();
            // `date` (Unix seconds): the screenshots' fixed clock.
            let now = q.get("date").and_then(|v| v.parse().ok()).unwrap_or_else(|| {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
            });
            let letter = {
                let peer = shared.peer.lock().unwrap();
                peer.learn_from(&sent);
                peer.letter(&shared.user, &subject, &String::from_utf8_lossy(&body), now)
            };
            match letter {
                Ok(raw) => {
                    let uid = shared.with(|f| f.deliver("INBOX", &raw, now));
                    ("200 OK", "application/json", json!({ "uid": uid }).to_string().into_bytes())
                }
                Err(e) => ("409 Conflict", "text/plain", e.into_bytes()),
            }
        }
        ("GET", "/sieve") => {
            let s = shared.sieve.lock().unwrap();
            ("200 OK", "application/json", json!({ "scripts": s.scripts, "active": s.active }).to_string().into_bytes())
        }
        ("GET", "/web") => {
            let web = shared.web.lock().unwrap().clone();
            ("200 OK", "application/json", serde_json::to_vec(&web).unwrap_or_default())
        }
        ("GET", "/autoconfig") => {
            let (imap, smtp) = *shared.ports.lock().unwrap();
            let host = q.get("host").map_or("127.0.0.1", String::as_str);
            ("200 OK", "text/xml", autoconfig(host, imap, smtp).into_bytes())
        }
        ("POST", "/delay") => {
            *shared.delay_ms.lock().unwrap() = q.get("ms").and_then(|v| v.parse().ok()).unwrap_or(0);
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("GET", "/sent") => {
            let sent = shared.sent.lock().unwrap().clone();
            ("200 OK", "application/json", serde_json::to_vec(&sent).unwrap_or_default())
        }
        ("GET", "/sent/raw") => {
            let i: usize = q.get("i").and_then(|v| v.parse().ok()).unwrap_or(0);
            match shared.sent.lock().unwrap().get(i) {
                Some(m) => ("200 OK", "message/rfc822", m.raw.clone()),
                None => ("404 Not Found", "text/plain", b"no such message".to_vec()),
            }
        }
        ("POST", "/deliver") => {
            let mailbox = q.get("mailbox").cloned().unwrap_or_else(|| "INBOX".into());
            let date: i64 = q.get("date").and_then(|v| v.parse().ok()).unwrap_or_else(|| {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
            });
            let exists = shared.fake.lock().unwrap().mailboxes.contains_key(&mailbox);
            if !exists {
                ("404 Not Found", "text/plain", b"no such folder".to_vec())
            } else {
                let uid = shared.with(|f| {
                    let uid = f.deliver(&mailbox, &body, date);
                    let flags = mail_core::imap::Flags {
                        seen: flag(&q, "seen").unwrap_or(false),
                        flagged: flag(&q, "flagged").unwrap_or(false),
                        answered: flag(&q, "answered").unwrap_or(false),
                        draft: flag(&q, "draft").unwrap_or(false),
                        ..Default::default()
                    };
                    f.set_flags(&mailbox, uid, flags);
                    uid
                });
                ("200 OK", "application/json", json!({ "uid": uid }).to_string().into_bytes())
            }
        }
        ("POST", "/flags") => {
            let mailbox = q.get("mailbox").cloned().unwrap_or_else(|| "INBOX".into());
            let uid: u32 = q.get("uid").and_then(|v| v.parse().ok()).unwrap_or(0);
            let ok = shared.with(|f| {
                let Some(m) = f.mailboxes.get(&mailbox).and_then(|m| m.messages.get(&uid)) else { return false };
                let mut flags = m.flags.clone();
                if let Some(v) = flag(&q, "seen") {
                    flags.seen = v;
                }
                if let Some(v) = flag(&q, "flagged") {
                    flags.flagged = v;
                }
                f.set_flags(&mailbox, uid, flags);
                true
            });
            if ok {
                ("200 OK", "text/plain", b"ok".to_vec())
            } else {
                ("404 Not Found", "text/plain", b"no such message".to_vec())
            }
        }
        ("POST", "/expunge") => {
            let mailbox = q.get("mailbox").cloned().unwrap_or_else(|| "INBOX".into());
            let uid: u32 = q.get("uid").and_then(|v| v.parse().ok()).unwrap_or(0);
            shared.with(|f| f.expunge(&mailbox, uid));
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/offline") => {
            shared.set_offline(true);
            ("200 OK", "text/plain", b"offline".to_vec())
        }
        ("POST", "/online") => {
            shared.set_offline(false);
            ("200 OK", "text/plain", b"online".to_vec())
        }
        ("POST", "/fail") => {
            let cmd = q.get("cmd").cloned().unwrap_or_default();
            shared.fake.lock().unwrap().fail.push(cmd);
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/password") => {
            *shared.password.lock().unwrap() = q.get("value").cloned().unwrap_or_default();
            ("200 OK", "text/plain", b"ok".to_vec())
        }
        ("POST", "/folder") => {
            let name = q.get("name").cloned().unwrap_or_default();
            let r = shared.with(|f| f.do_create(&name));
            match r {
                Ok(()) => ("200 OK", "text/plain", b"ok".to_vec()),
                Err(e) => ("409 Conflict", "text/plain", e.to_string().into_bytes()),
            }
        }
        _ => ("404 Not Found", "text/plain", b"unknown".to_vec()),
    };
    let mut stream = r.into_inner();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        out.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&out).await?;
    stream.flush().await
}

pub(crate) fn serve(listener: TcpListener, shared: Arc<Shared>) {
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else { return };
            let shared = shared.clone();
            tokio::spawn(async move {
                let _ = handle(s, shared).await;
            });
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_queries() {
        let (p, q) = super::query("/deliver?mailbox=Gel%C3%B6scht&seen=1&x=a+b");
        assert_eq!(p, "/deliver");
        assert_eq!(q["mailbox"], "Gelöscht");
        assert_eq!(q["x"], "a b");
    }
}
