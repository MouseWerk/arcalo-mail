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

async fn handle(stream: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
    let mut r = BufReader::new(stream);
    let mut first = String::new();
    r.read_line(&mut first).await?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).await? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    r.read_exact(&mut body).await?;
    let (path, q) = query(&target);
    let (status, ctype, out): (&str, &str, Vec<u8>) = match (method.as_str(), path.as_str()) {
        ("GET", "/state") => ("200 OK", "application/json", state(&shared).to_string().into_bytes()),
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
