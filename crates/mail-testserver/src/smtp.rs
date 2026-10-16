//! The SMTP side: EHLO, STARTTLS, AUTH PLAIN and LOGIN, MAIL, RCPT, DATA. A message to the
//! account's own address lands in its inbox; every message is kept for the tests to read.

use std::sync::Arc;

use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{Io, SentMail, Shared, upgrade};

struct Session {
    shared: Arc<Shared>,
    io: BufReader<Io>,
    tls: bool,
    authed: bool,
    from: Option<String>,
    to: Vec<String>,
}

fn b64(s: &str) -> String {
    String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(s.trim()).unwrap_or_default())
        .into_owned()
}

fn angle(s: &str) -> String {
    let s = s.trim();
    let inner = s.split_once('<').and_then(|(_, r)| r.split_once('>')).map_or(s, |(a, _)| a);
    inner.trim().to_string()
}

impl Session {
    async fn reply(&mut self, s: &str) -> std::io::Result<()> {
        self.io.get_mut().write_all(s.as_bytes()).await?;
        self.io.get_mut().flush().await
    }

    async fn line(&mut self) -> std::io::Result<Option<String>> {
        let mut l = String::new();
        if self.io.read_line(&mut l).await? == 0 {
            return Ok(None);
        }
        Ok(Some(l.trim_end_matches(['\r', '\n']).to_string()))
    }

    async fn data(&mut self) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let mut l = Vec::new();
            if self.io.read_until(b'\n', &mut l).await? == 0 {
                return Ok(out);
            }
            if l == b".\r\n" || l == b".\n" {
                return Ok(out);
            }
            // Dot-stuffing (RFC 5321, 4.5.2).
            let l = if l.starts_with(b"..") { l[1..].to_vec() } else { l };
            out.extend_from_slice(&l);
        }
    }

    fn deliver(&mut self, raw: Vec<u8>) {
        let from = self.from.take().unwrap_or_default();
        let to = std::mem::take(&mut self.to);
        let subject = mail_core::parse::parse(&raw).map(|p| p.subject).unwrap_or_default();
        let user = self.shared.user.clone();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        if to.iter().any(|t| t.eq_ignore_ascii_case(&user)) {
            self.shared.with(|f| f.deliver("INBOX", &raw, now));
        }
        if self.shared.files_sent && from.eq_ignore_ascii_case(&user) {
            self.shared.with(|f| {
                let uid = f.deliver("Sent", &raw, now);
                f.set_flags("Sent", uid, mail_core::imap::Flags { seen: true, ..Default::default() });
            });
        }
        let size = raw.len();
        self.shared.sent.lock().unwrap().push(SentMail { from, to, subject, raw, size });
    }

    /// Runs the session; `true` when the client asked for STARTTLS.
    async fn run(&mut self) -> std::io::Result<bool> {
        while let Some(line) = self.line().await? {
            let upper = line.to_ascii_uppercase();
            let verb = upper.split_whitespace().next().unwrap_or_default().to_string();
            match verb.as_str() {
                "EHLO" | "HELO" => {
                    let starttls = !self.tls && crate::acceptor(&self.shared).is_some();
                    let mut r = String::from("250-localhost greets you\r\n250-8BITMIME\r\n250-SIZE 52428800\r\n");
                    if starttls {
                        r.push_str("250-STARTTLS\r\n");
                    }
                    r.push_str("250 AUTH PLAIN LOGIN\r\n");
                    self.reply(&r).await?;
                }
                "STARTTLS" => {
                    if self.tls || crate::acceptor(&self.shared).is_none() {
                        self.reply("502 STARTTLS not available\r\n").await?;
                    } else {
                        self.reply("220 Ready to start TLS\r\n").await?;
                        return Ok(true);
                    }
                }
                "AUTH" => {
                    let words: Vec<&str> = line.split_whitespace().collect();
                    let mech = words.get(1).map(|m| m.to_ascii_uppercase()).unwrap_or_default();
                    let ok = match mech.as_str() {
                        "PLAIN" => {
                            let token = match words.get(2) {
                                Some(t) => t.to_string(),
                                None => {
                                    self.reply("334 \r\n").await?;
                                    self.line().await?.unwrap_or_default()
                                }
                            };
                            let decoded = b64(&token);
                            let parts: Vec<&str> = decoded.split('\0').collect();
                            parts.len() == 3 && self.shared.check_login(parts[1], parts[2])
                        }
                        "LOGIN" => {
                            self.reply("334 VXNlcm5hbWU6\r\n").await?;
                            let user = b64(&self.line().await?.unwrap_or_default());
                            self.reply("334 UGFzc3dvcmQ6\r\n").await?;
                            let pw = b64(&self.line().await?.unwrap_or_default());
                            self.shared.check_login(&user, &pw)
                        }
                        _ => false,
                    };
                    if ok {
                        self.authed = true;
                        self.reply("235 2.7.0 Authentication successful\r\n").await?;
                    } else {
                        self.reply("535 5.7.8 Authentication credentials invalid\r\n").await?;
                    }
                }
                "MAIL" => {
                    if !self.authed {
                        self.reply("530 5.7.0 Authentication required\r\n").await?;
                        continue;
                    }
                    self.from = Some(angle(line.split_once(':').map_or("", |(_, r)| r)));
                    self.to.clear();
                    self.reply("250 2.1.0 OK\r\n").await?;
                }
                "RCPT" => {
                    self.to.push(angle(line.split_once(':').map_or("", |(_, r)| r)));
                    self.reply("250 2.1.5 OK\r\n").await?;
                }
                "DATA" => {
                    if self.from.is_none() || self.to.is_empty() {
                        self.reply("503 5.5.1 MAIL and RCPT first\r\n").await?;
                        continue;
                    }
                    self.reply("354 End data with <CR><LF>.<CR><LF>\r\n").await?;
                    let raw = self.data().await?;
                    self.deliver(raw);
                    self.reply("250 2.0.0 OK queued\r\n").await?;
                }
                "RSET" => {
                    self.from = None;
                    self.to.clear();
                    self.reply("250 2.0.0 OK\r\n").await?;
                }
                "NOOP" => self.reply("250 2.0.0 OK\r\n").await?,
                "QUIT" => {
                    self.reply("221 2.0.0 Bye\r\n").await?;
                    return Ok(false);
                }
                _ => self.reply("502 5.5.2 Command not recognized\r\n").await?,
            }
        }
        Ok(false)
    }
}

pub(crate) async fn session(io: Io, shared: Arc<Shared>, tls: bool) {
    let mut s = Session { shared: shared.clone(), io: BufReader::new(io), tls, authed: false, from: None, to: vec![] };
    if s.reply("220 localhost ESMTP Arcalo Mail test server\r\n").await.is_err() {
        return;
    }
    loop {
        match s.run().await {
            Ok(true) => {
                let Some(upgraded) = upgrade(s.io.into_inner(), &shared).await else { return };
                s = Session {
                    shared: shared.clone(),
                    io: BufReader::new(upgraded),
                    tls: true,
                    authed: false,
                    from: None,
                    to: vec![],
                };
            }
            _ => return,
        }
    }
}
