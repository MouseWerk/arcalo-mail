//! The JMAP side: the same mailboxes as IMAP through `mail_core::jmap::fake` (session, API,
//! download, upload) on the control port, with push over EventSource (`/jmap/events`) that
//! sends a `state` event whenever the mailboxes change. Submitted messages land where SMTP
//! would put them: in the list of sent mail, and in the inbox when they are addressed to the
//! account.

use std::sync::Arc;

use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::{SentMail, Shared};

pub(crate) async fn handle(
    r: BufReader<TcpStream>,
    method: &str,
    target: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
    shared: Arc<Shared>,
) -> std::io::Result<()> {
    let mut stream = r.into_inner();
    let path = target.split('?').next().unwrap_or(target);
    let host = headers.iter().find(|(k, _)| k == "host").map(|(_, v)| v.clone()).unwrap_or_else(|| "127.0.0.1".into());
    let base = format!("http://{host}");
    let password = shared.password.lock().unwrap().clone();
    let open = path == "/.well-known/jmap" && method == "GET" && !headers.iter().any(|(k, _)| k == "authorization");
    if shared.is_offline() {
        drop(stream);
        return Ok(());
    }
    if !open && !mail_core::jmap::fake::authorized(headers, &shared.user, &password) {
        let resp = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"jmap\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        stream.write_all(resp.as_bytes()).await?;
        return stream.flush().await;
    }
    if open {
        // Discovery: the well-known address asks to sign in (RFC 8620, 2.2).
        let resp = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"jmap\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        stream.write_all(resp.as_bytes()).await?;
        return stream.flush().await;
    }
    if path == "/jmap/events" {
        return events(stream, shared).await;
    }
    let reply = {
        let mut fake = shared.fake.lock().unwrap();
        let mut jmap = shared.jmap.lock().unwrap();
        jmap.handle(&mut fake, method, target, &body, &base)
    };
    let submitted: Vec<_> = std::mem::take(&mut shared.jmap.lock().unwrap().submitted);
    for s in submitted {
        let subject = mail_core::parse::parse(&s.raw).map(|p| p.subject).unwrap_or_default();
        if s.to.iter().any(|t| t.eq_ignore_ascii_case(&shared.user)) {
            let now =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            shared.with(|f| f.deliver("INBOX", &s.raw, now));
        }
        let size = s.raw.len();
        shared.sent.lock().unwrap().push(SentMail { from: s.from, to: s.to, subject, raw: s.raw, size });
    }
    if method != "GET" {
        // A change through JMAP wakes IDLE connections and push streams.
        shared.changed();
    }
    let head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&reply.body).await?;
    stream.flush().await
}

/// The EventSource stream: a `state` event now and after every change, a ping every 20 s.
async fn events(mut stream: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")
        .await?;
    let mut changes = shared.subscribe_changes();
    let mut last = String::new();
    loop {
        let event = {
            let fake = shared.fake.lock().unwrap();
            shared.jmap.lock().unwrap().event(&fake)
        };
        if event != last {
            stream.write_all(event.as_bytes()).await?;
            stream.flush().await?;
            last = event;
        }
        tokio::select! {
            r = changes.changed() => if r.is_err() { return Ok(()) },
            _ = tokio::time::sleep(std::time::Duration::from_secs(20)) => {
                stream.write_all(b": ping\n\n").await?;
            }
            _ = shared.until_offline() => return Ok(()),
        }
    }
}
