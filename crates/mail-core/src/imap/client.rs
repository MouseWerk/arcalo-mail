//! The IMAP client: our own commands over a stream from [`crate::network`], with imap-proto
//! parsing the answers. Covers what the engine needs (RFC 3501 plus IDLE, UIDPLUS, MOVE,
//! CONDSTORE/QRESYNC, SPECIAL-USE, ENABLE, LITERAL+), so nothing of the protocol is hidden
//! behind a library that cannot say `SELECT … (QRESYNC …)`.
//!
//! One [`ImapSession`] is one authenticated connection. Commands run one at a time; untagged
//! answers that belong to no command (EXISTS during a FETCH) are noted, and IDLE ends on them.

use std::time::Duration;

use base64::Engine;
use imap_proto::{
    AttributeValue, BodyStructure, Capability, MailboxDatum, MessageSection, NameAttribute, Response, ResponseCode,
    SectionPath, Status,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

use super::*;
use crate::account::{Credential, Server};
use crate::error::{Error, Result};
use crate::network::{self, IO_TIMEOUT, MailStream, Problem, auth_problem};
use crate::tr;

/// The answer to a command: its untagged data and how it ended.
struct Done {
    data: Vec<Response<'static>>,
    status: Status,
    code: Option<ResponseCode<'static>>,
    text: String,
}

/// One line (or a line with literals) from the server.
enum Reply {
    Data(Response<'static>),
    Tagged {
        tag: String,
        status: Status,
        code: Option<ResponseCode<'static>>,
        text: String,
    },
    /// `+ …` with its text (a SASL challenge or error).
    Continue(String),
    Bye(String),
    /// An untagged line imap-proto does not know (an extension's answer); passed over.
    Unknown,
}

pub struct ImapSession {
    stream: MailStream,
    buf: Vec<u8>,
    tag: u32,
    caps: Capabilities,
    raw_caps: Vec<String>,
    /// Untagged answers that arrived outside a command (or during one that did not ask).
    changed: bool,
    host: String,
}

impl std::fmt::Debug for ImapSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapSession").field("host", &self.host).field("caps", &self.caps).finish()
    }
}

/// `"name"` with `\` and `"` escaped; a literal when it cannot be quoted.
fn quoted(s: &str) -> Option<String> {
    if s.bytes().any(|b| b == b'\r' || b == b'\n' || b >= 0x80) {
        return None;
    }
    Some(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
}

fn mailbox_arg(name: &str) -> String {
    quoted(&utf7::encode(name)).unwrap_or_else(|| format!("\"{}\"", utf7::encode(name)))
}

fn flags_of(list: &[std::borrow::Cow<'_, str>]) -> Flags {
    let mut f = Flags::default();
    for flag in list {
        match flag.to_ascii_lowercase().as_str() {
            "\\seen" => f.seen = true,
            "\\answered" => f.answered = true,
            "\\flagged" => f.flagged = true,
            "\\deleted" => f.deleted = true,
            "\\draft" => f.draft = true,
            "\\recent" => {}
            _ => f.keywords.push(flag.to_string()),
        }
    }
    f
}

/// `16-Oct-2026 09:30:00 +0200` → Unix seconds.
pub fn parse_internal_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once(' ')?;
    let (time, zone) = rest.trim().split_once(' ')?;
    let mut d = date.trim().split('-');
    let day: i64 = d.next()?.trim().parse().ok()?;
    let month_name = d.next()?;
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
        .iter()
        .position(|m| m.eq_ignore_ascii_case(month_name))? as i64
        + 1;
    let year: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':').map(|x| x.parse::<i64>());
    let (h, mi, se) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let z: i64 = zone.trim_start_matches(['+', '-']).parse().ok()?;
    let offset = sign * ((z / 100) * 3600 + (z % 100) * 60);
    Some(days_from_civil(year, month, day) * 86400 + h * 3600 + mi * 60 + se - offset)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn uid_map(code: &Option<ResponseCode<'_>>) -> Vec<(u32, u32)> {
    let Some(ResponseCode::CopyUid(_, from, to)) = code else { return vec![] };
    let expand = |set: &Vec<imap_proto::rfc4315::UidSetMember>| -> Vec<u32> {
        set.iter()
            .flat_map(|m| match m {
                imap_proto::rfc4315::UidSetMember::Uid(u) => vec![*u],
                imap_proto::rfc4315::UidSetMember::UidRange(r) => r.clone().collect(),
            })
            .collect()
    };
    let (a, b) = (expand(from), expand(to));
    if a.len() == b.len() { a.into_iter().zip(b).collect() } else { vec![] }
}

/// A header value as mail-parser decodes it (RFC 2047 words in BODYSTRUCTURE file names).
fn decode_words(s: &str) -> String {
    if !s.contains("=?") {
        return s.to_string();
    }
    let raw = format!("Subject: {s}\r\n\r\n");
    mail_parser::MessageParser::default()
        .parse(raw.as_bytes())
        .and_then(|m| m.subject().map(str::to_string))
        .unwrap_or_else(|| s.to_string())
}

/// RFC 2231 `filename*=utf-8''Gr%C3%BC%C3%9Fe.pdf`.
fn decode_2231(s: &str) -> String {
    let value = s.splitn(3, '\'').nth(2).unwrap_or(s);
    let mut bytes = vec![];
    let mut it = value.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex: String = it.by_ref().take(2).map(char::from).collect();
            if let Ok(v) = u8::from_str_radix(&hex, 16) {
                bytes.push(v);
                continue;
            }
        }
        bytes.push(b);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn param(params: &imap_proto::BodyParams<'_>, name: &str) -> Option<String> {
    let list = params.as_ref()?;
    if let Some((_, v)) = list.iter().find(|(k, _)| k.eq_ignore_ascii_case(&format!("{name}*"))) {
        return Some(decode_2231(v));
    }
    list.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| decode_words(v))
}

/// The leaves of a BODYSTRUCTURE with their section numbers.
pub fn parts_of(structure: &BodyStructure<'_>) -> Vec<BodyPart> {
    fn walk(s: &BodyStructure<'_>, prefix: &str, out: &mut Vec<BodyPart>) {
        let (common, other) = match s {
            BodyStructure::Multipart { bodies, .. } => {
                for (i, b) in bodies.iter().enumerate() {
                    let section = if prefix.is_empty() { (i + 1).to_string() } else { format!("{prefix}.{}", i + 1) };
                    walk(b, &section, out);
                }
                return;
            }
            BodyStructure::Basic { common, other, .. }
            | BodyStructure::Text { common, other, .. }
            | BodyStructure::Message { common, other, .. } => (common, other),
        };
        let mime_type = format!("{}/{}", common.ty.ty, common.ty.subtype).to_ascii_lowercase();
        let disposition = common.disposition.as_ref();
        let filename = disposition
            .and_then(|d| param(&d.params, "filename"))
            .or_else(|| param(&common.ty.params, "name"))
            .or_else(|| (mime_type == "message/rfc822").then(|| "message.eml".to_string()));
        let inline = disposition.is_none_or(|d| !d.ty.eq_ignore_ascii_case("attachment"));
        out.push(BodyPart {
            section: if prefix.is_empty() { "1".into() } else { prefix.into() },
            mime_type,
            filename,
            size: other.octets as u64,
            content_id: other.id.as_ref().map(|c| c.trim().trim_matches(['<', '>']).to_string()),
            inline,
        });
    }
    let mut out = vec![];
    walk(structure, "", &mut out);
    out
}

impl ImapSession {
    /// Connects, signs in (AUTHENTICATE PLAIN, else LOGIN) and switches on QRESYNC or
    /// CONDSTORE when offered.
    pub async fn connect(server: &Server, username: &str, password: &str) -> Result<Self> {
        Self::connect_with(server, username, &Credential::Password(password.to_string())).await
    }

    /// Connects and signs in with a password or an OAuth access token (AUTHENTICATE XOAUTH2).
    pub async fn connect_with(server: &Server, username: &str, credential: &Credential) -> Result<Self> {
        let t = network::connect_imap(server).await?;
        let mut s = ImapSession {
            stream: t.stream,
            buf: Vec::with_capacity(16 * 1024),
            tag: 0,
            caps: Capabilities::default(),
            raw_caps: vec![],
            changed: false,
            host: server.host.clone(),
        };
        if !t.greeted {
            match s.read_reply(IO_TIMEOUT).await? {
                Reply::Data(Response::Data { status: Status::Ok | Status::PreAuth, outcome }) => {
                    if let Some(ResponseCode::Capabilities(c)) = outcome.code {
                        s.set_caps(&c);
                    }
                }
                Reply::Bye(text) => return Err(Problem::Protocol { detail: text }.into()),
                _ => {
                    return Err(
                        Problem::Protocol { detail: tr!("kein IMAP-Server", "not an IMAP server").into() }.into()
                    );
                }
            }
        }
        if s.raw_caps.is_empty() {
            s.refresh_caps().await?;
        }
        match credential {
            Credential::Password(password) => s.login(username, password).await?,
            Credential::XOAuth2(token) => s.xoauth2(username, token).await?,
        }
        s.refresh_caps().await?;
        // QRESYNC only once the server switched it on: else every `SELECT … (QRESYNC …)` would
        // be refused.
        let qresync = s.caps.qresync && s.has("ENABLE") && s.command("ENABLE QRESYNC").await?.status == Status::Ok;
        if !qresync {
            s.caps.qresync = false;
            if s.caps.condstore && s.has("ENABLE") {
                s.command("ENABLE CONDSTORE").await?;
            }
        }
        Ok(s)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    fn has(&self, cap: &str) -> bool {
        self.raw_caps.iter().any(|c| c.eq_ignore_ascii_case(cap))
    }

    fn set_caps(&mut self, list: &[Capability<'_>]) {
        self.raw_caps = list
            .iter()
            .map(|c| match c {
                Capability::Imap4rev1 => "IMAP4rev1".to_string(),
                Capability::Auth(a) => format!("AUTH={a}"),
                Capability::Atom(a) => a.to_string(),
            })
            .collect();
        self.caps = Capabilities::parse(self.raw_caps.iter().map(String::as_str));
    }

    async fn refresh_caps(&mut self) -> Result<()> {
        let done = self.command("CAPABILITY").await?;
        for d in &done.data {
            if let Response::Capabilities(c) = d {
                self.set_caps(c);
            }
        }
        Ok(())
    }

    async fn login(&mut self, username: &str, password: &str) -> Result<()> {
        if self.has("LOGINDISABLED") && !self.stream.is_encrypted() {
            return Err(Problem::Tls { host: self.host.clone(), detail: "LOGINDISABLED".into() }.into());
        }
        let done = if self.has("AUTH=PLAIN") || !self.has("IMAP4rev1") && !self.has("IMAP4") {
            let token = base64::engine::general_purpose::STANDARD.encode(format!("\0{username}\0{password}"));
            let tag = self.next_tag();
            self.write(format!("{tag} AUTHENTICATE PLAIN\r\n").as_bytes()).await?;
            loop {
                match self.read_reply(IO_TIMEOUT).await? {
                    Reply::Continue(_) => break,
                    Reply::Tagged { status, text, .. } => {
                        return if status == Status::Ok { Ok(()) } else { Err(auth_problem(&text).into()) };
                    }
                    Reply::Bye(text) => return Err(Problem::Protocol { detail: text }.into()),
                    Reply::Data(_) | Reply::Unknown => {}
                }
            }
            self.write(format!("{token}\r\n").as_bytes()).await?;
            self.finish(&tag, IO_TIMEOUT).await?
        } else {
            let mut parts: Vec<Arg> = vec![Arg::Text("LOGIN ".into())];
            parts.push(Arg::string(username));
            parts.push(Arg::Text(" ".into()));
            parts.push(Arg::string(password));
            self.command_with(parts).await?
        };
        match done.status {
            Status::Ok => {
                if let Some(ResponseCode::Capabilities(c)) = &done.code {
                    self.set_caps(c);
                }
                Ok(())
            }
            _ => Err(auth_problem(&done.text).into()),
        }
    }

    /// SASL XOAUTH2 (Microsoft, Google): the token as the initial response (SASL-IR) or after
    /// the continuation. A refusal comes as a continuation with a JSON error first, answered
    /// with an empty line, then `NO`.
    async fn xoauth2(&mut self, username: &str, token: &str) -> Result<()> {
        let ir = crate::oauth::xoauth2(username, token);
        let tag = self.next_tag();
        let sasl_ir = self.has("SASL-IR");
        if sasl_ir {
            self.write(format!("{tag} AUTHENTICATE XOAUTH2 {ir}\r\n").as_bytes()).await?;
        } else {
            self.write(format!("{tag} AUTHENTICATE XOAUTH2\r\n").as_bytes()).await?;
        }
        let mut sent = sasl_ir;
        let mut error_json = String::new();
        loop {
            match self.read_reply(IO_TIMEOUT).await? {
                Reply::Continue(_) if !sent => {
                    self.write(format!("{ir}\r\n").as_bytes()).await?;
                    sent = true;
                }
                Reply::Continue(t) => {
                    error_json = base64::engine::general_purpose::STANDARD
                        .decode(t.trim())
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    self.write(b"\r\n").await?;
                }
                Reply::Tagged { tag: t, status, code, text } if t == tag => {
                    if status == Status::Ok {
                        if let Some(ResponseCode::Capabilities(c)) = &code {
                            self.set_caps(c);
                        }
                        return Ok(());
                    }
                    let detail = if error_json.is_empty() { text } else { format!("{text} {error_json}") };
                    return Err(Problem::Auth { detail: detail.trim().to_string() }.into());
                }
                Reply::Tagged { .. } | Reply::Data(_) | Reply::Unknown => {}
                Reply::Bye(text) => return Err(Problem::Protocol { detail: text }.into()),
            }
        }
    }

    fn next_tag(&mut self) -> String {
        self.tag += 1;
        format!("a{}", self.tag)
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        let r = tokio::time::timeout(IO_TIMEOUT, async {
            self.stream.write_all(data).await?;
            self.stream.flush().await
        })
        .await;
        match r {
            Err(_) => Err(Problem::Timeout { host: self.host.clone(), port: 0 }.into()),
            Ok(Err(e)) => Err(network::io_problem(&e).into()),
            Ok(Ok(())) => Ok(()),
        }
    }

    /// Reads the next reply. Cancel-safe: bytes are only taken from the socket when a read
    /// completes, so IDLE can wait on it next to a timer.
    async fn read_reply(&mut self, timeout: Duration) -> Result<Reply> {
        loop {
            if let Some(r) = self.parse_buffered()? {
                return Ok(r);
            }
            let mut tmp = [0u8; 16 * 1024];
            let n = match tokio::time::timeout(timeout, self.stream.read(&mut tmp)).await {
                Err(_) => return Err(Problem::Timeout { host: self.host.clone(), port: 0 }.into()),
                Ok(Err(e)) => return Err(network::io_problem(&e).into()),
                Ok(Ok(n)) => n,
            };
            if n == 0 {
                return Err(
                    Problem::Protocol { detail: tr!("Verbindung geschlossen", "connection closed").into() }.into()
                );
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }

    /// One reply from the buffer, `None` when more bytes are needed. A line imap-proto cannot
    /// read is passed over (untagged) or read by hand (tagged), so an unknown extension never
    /// breaks the connection.
    fn parse_buffered(&mut self) -> Result<Option<Reply>> {
        if self.buf.is_empty() {
            return Ok(None);
        }
        match Response::parse(&self.buf) {
            Ok((rest, resp)) => {
                let used = self.buf.len() - rest.len();
                let owned = resp.into_owned();
                self.buf.drain(..used);
                Ok(Some(match owned {
                    Response::Continue(o) => Reply::Continue(o.information.map(|c| c.to_string()).unwrap_or_default()),
                    Response::Done { tag, status, outcome } => Reply::Tagged {
                        tag: tag.0,
                        status,
                        code: outcome.code,
                        text: outcome.information.map(|c| c.to_string()).unwrap_or_default(),
                    },
                    Response::Data { status: Status::Bye, outcome } => {
                        Reply::Bye(outcome.information.map(|c| c.to_string()).unwrap_or_default())
                    }
                    other => Reply::Data(other),
                }))
            }
            Err(nom::Err::Incomplete(_)) => Ok(None),
            Err(_) => {
                let Some(end) = self.buf.windows(2).position(|w| w == b"\r\n") else { return Ok(None) };
                let line: Vec<u8> = self.buf.drain(..end + 2).collect();
                let line = String::from_utf8_lossy(&line).trim_end().to_string();
                if line.starts_with('*') {
                    return Ok(Some(Reply::Unknown));
                }
                if let Some(rest) = line.strip_prefix('+') {
                    return Ok(Some(Reply::Continue(rest.trim().to_string())));
                }
                let mut words = line.splitn(3, ' ');
                let tag = words.next().unwrap_or_default().to_string();
                let status = match words.next().unwrap_or_default().to_ascii_uppercase().as_str() {
                    "OK" => Status::Ok,
                    "NO" => Status::No,
                    _ => Status::Bad,
                };
                Ok(Some(Reply::Tagged { tag, status, code: None, text: words.next().unwrap_or_default().to_string() }))
            }
        }
    }

    /// Reads until the tagged answer of `tag`; untagged data is collected.
    async fn finish(&mut self, tag: &str, timeout: Duration) -> Result<Done> {
        let mut data = vec![];
        loop {
            match self.read_reply(timeout).await? {
                Reply::Data(d) => {
                    if matches!(d, Response::Expunge(_) | Response::MailboxData(MailboxDatum::Exists(_))) {
                        self.changed = true;
                    }
                    data.push(d)
                }
                Reply::Continue(_) | Reply::Unknown => {}
                Reply::Bye(text) => return Err(Problem::Protocol { detail: text }.into()),
                Reply::Tagged { tag: t, status, code, text } if t == tag => {
                    return Ok(Done { data, status, code, text });
                }
                Reply::Tagged { .. } => {}
            }
        }
    }

    async fn command(&mut self, cmd: &str) -> Result<Done> {
        self.command_with(vec![Arg::Text(cmd.to_string())]).await
    }

    /// Sends a command made of text and literals (each literal waits for the server's `+`,
    /// or not with LITERAL+), and reads its answer.
    async fn command_with(&mut self, parts: Vec<Arg>) -> Result<Done> {
        let tag = self.next_tag();
        let plus = self.has("LITERAL+");
        let mut line = format!("{tag} ").into_bytes();
        for part in parts {
            match part {
                Arg::Text(t) => line.extend_from_slice(t.as_bytes()),
                Arg::Literal(bytes) => {
                    line.extend_from_slice(
                        format!("{{{}{}}}\r\n", bytes.len(), if plus { "+" } else { "" }).as_bytes(),
                    );
                    self.write(&line).await?;
                    line.clear();
                    if !plus {
                        loop {
                            match self.read_reply(IO_TIMEOUT).await? {
                                Reply::Continue(_) => break,
                                Reply::Tagged { status, text, code, .. } => {
                                    return Ok(Done { data: vec![], status, code, text });
                                }
                                Reply::Bye(text) => return Err(Problem::Protocol { detail: text }.into()),
                                Reply::Data(_) | Reply::Unknown => {}
                            }
                        }
                    }
                    line.extend_from_slice(&bytes);
                }
            }
        }
        line.extend_from_slice(b"\r\n");
        self.write(&line).await?;
        self.finish(&tag, IO_TIMEOUT * 3).await
    }

    /// A command that must succeed.
    async fn ok(&mut self, cmd: &str) -> Result<Done> {
        let done = self.command(cmd).await?;
        check(done, cmd)
    }
}

fn check(done: Done, what: &str) -> Result<Done> {
    match done.status {
        Status::Ok => Ok(done),
        _ => Err(Error::Imap(format!(
            "{} {}: {}",
            what.split(' ').next().unwrap_or(what),
            status_word(&done.status),
            done.text
        ))),
    }
}

fn status_word(s: &Status) -> &'static str {
    match s {
        Status::Ok => "OK",
        Status::No => "NO",
        Status::Bad => "BAD",
        Status::PreAuth => "PREAUTH",
        Status::Bye => "BYE",
    }
}

enum Arg {
    Text(String),
    Literal(Vec<u8>),
}

impl Arg {
    fn string(s: &str) -> Arg {
        match quoted(s) {
            Some(q) => Arg::Text(q),
            None => Arg::Literal(s.as_bytes().to_vec()),
        }
    }
}

fn fetch_attrs<'a>(d: &'a Response<'static>) -> Option<&'a Vec<AttributeValue<'static>>> {
    match d {
        Response::Fetch(_, attrs) => Some(attrs),
        _ => None,
    }
}

fn uid_of(attrs: &[AttributeValue<'_>]) -> Option<u32> {
    attrs.iter().find_map(|a| if let AttributeValue::Uid(u) = a { Some(*u) } else { None })
}

impl ImapClient for ImapSession {
    async fn capabilities(&mut self) -> Result<Capabilities> {
        Ok(self.caps)
    }

    async fn list(&mut self) -> Result<Vec<MailboxEntry>> {
        let cmd = if self.caps.special_use { "LIST \"\" \"*\" RETURN (SPECIAL-USE)" } else { "LIST \"\" \"*\"" };
        let mut done = self.command(cmd).await?;
        if done.status != Status::Ok && self.caps.special_use {
            done = self.command("LIST \"\" \"*\"").await?;
        }
        let done = check(done, "LIST")?;
        Ok(done
            .data
            .iter()
            .filter_map(|d| match d {
                Response::MailboxData(MailboxDatum::List(l)) => Some(MailboxEntry {
                    name: utf7::decode(&l.name),
                    delimiter: l.delimiter.as_ref().and_then(|d| d.chars().next()),
                    attributes: l
                        .name_attributes
                        .iter()
                        .map(|a| match a {
                            NameAttribute::NoInferiors => "\\Noinferiors".to_string(),
                            NameAttribute::NoSelect => "\\Noselect".into(),
                            NameAttribute::Marked => "\\Marked".into(),
                            NameAttribute::Unmarked => "\\Unmarked".into(),
                            NameAttribute::All => "\\All".into(),
                            NameAttribute::Archive => "\\Archive".into(),
                            NameAttribute::Drafts => "\\Drafts".into(),
                            NameAttribute::Flagged => "\\Flagged".into(),
                            NameAttribute::Junk => "\\Junk".into(),
                            NameAttribute::Sent => "\\Sent".into(),
                            NameAttribute::Trash => "\\Trash".into(),
                            NameAttribute::Extension(e) => e.to_string(),
                            _ => String::new(),
                        })
                        .collect(),
                }),
                _ => None,
            })
            .collect())
    }

    async fn select(&mut self, mailbox: &str, qresync: Option<QresyncParams>) -> Result<Selected> {
        let mut cmd = format!("SELECT {}", mailbox_arg(mailbox));
        match qresync {
            Some(q) if self.caps.qresync => cmd.push_str(&format!(" (QRESYNC ({} {}))", q.uid_validity, q.modseq)),
            _ if self.caps.condstore => cmd.push_str(" (CONDSTORE)"),
            _ => {}
        }
        let done = check(self.command(&cmd).await?, "SELECT")?;
        let mut info = SelectInfo { uid_validity: 0, uid_next: 0, exists: 0, highest_modseq: None };
        let mut out = Selected { info, vanished: vec![], changed: vec![] };
        let mut have_next = false;
        for d in &done.data {
            match d {
                Response::MailboxData(MailboxDatum::Exists(n)) => info.exists = *n,
                Response::Data { outcome, .. } => match outcome.code {
                    Some(ResponseCode::UidValidity(v)) => info.uid_validity = v,
                    Some(ResponseCode::UidNext(v)) => {
                        info.uid_next = v;
                        have_next = true;
                    }
                    Some(ResponseCode::HighestModSeq(v)) => info.highest_modseq = Some(v),
                    _ => {}
                },
                Response::Vanished { uids, .. } => out.vanished.extend(uids.iter().flat_map(|r| r.clone())),
                Response::Fetch(_, attrs) => {
                    if let Some(uid) = uid_of(attrs) {
                        let mut flags = Flags::default();
                        let mut modseq = None;
                        for a in attrs {
                            match a {
                                AttributeValue::Flags(f) => flags = flags_of(f),
                                AttributeValue::ModSeq(m) => modseq = Some(*m),
                                _ => {}
                            }
                        }
                        out.changed.push(FlagUpdate { uid, flags, modseq });
                    }
                }
                _ => {}
            }
        }
        if !self.caps.condstore {
            info.highest_modseq = None;
        }
        if !have_next {
            // Servers that leave UIDNEXT out: one more than the highest UID.
            info.uid_next = if info.exists == 0 {
                1
            } else {
                let d = check(self.command("UID FETCH * (UID)").await?, "FETCH")?;
                d.data.iter().filter_map(fetch_attrs).filter_map(|a| uid_of(a)).max().unwrap_or(0) + 1
            };
        }
        out.info = info;
        self.changed = false;
        Ok(out)
    }

    async fn uids(&mut self) -> Result<Vec<u32>> {
        let done = check(self.command("UID SEARCH ALL").await?, "SEARCH")?;
        let mut out: Vec<u32> = done
            .data
            .iter()
            .flat_map(|d| match d {
                Response::MailboxData(MailboxDatum::Search(v)) => v.clone(),
                _ => vec![],
            })
            .collect();
        out.sort_unstable();
        Ok(out)
    }

    async fn fetch_headers(&mut self, range: UidRange) -> Result<Vec<FetchedHeader>> {
        let to = range.to.map_or("*".into(), |t| t.to_string());
        let modseq = if self.caps.condstore { " MODSEQ" } else { "" };
        let gmail = if self.caps.gmail { " X-GM-MSGID" } else { "" };
        let cmd = format!(
            "UID FETCH {}:{to} (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER]{modseq}{gmail})",
            range.from
        );
        let done = check(self.command(&cmd).await?, "FETCH")?;
        let mut out = vec![];
        for attrs in done.data.iter().filter_map(fetch_attrs) {
            let Some(uid) = uid_of(attrs) else { continue };
            if !range.contains(uid) {
                continue;
            }
            let mut h = FetchedHeader {
                uid,
                flags: Flags::default(),
                modseq: None,
                internal_date: 0,
                size: 0,
                header: vec![],
                gm_msgid: None,
            };
            // A FETCH the server sent on its own (a flag change) has no header: not this answer's.
            if !attrs.iter().any(|a| matches!(a, AttributeValue::BodySection { .. } | AttributeValue::Rfc822Header(_)))
            {
                continue;
            }
            for a in attrs {
                match a {
                    AttributeValue::Flags(f) => h.flags = flags_of(f),
                    AttributeValue::ModSeq(m) => h.modseq = Some(*m),
                    AttributeValue::GmailMsgId(id) => h.gm_msgid = Some(*id),
                    AttributeValue::InternalDate(d) => h.internal_date = parse_internal_date(d).unwrap_or(0),
                    AttributeValue::Rfc822Size(s) => h.size = *s,
                    AttributeValue::BodySection { data: Some(d), .. } | AttributeValue::Rfc822Header(Some(d)) => {
                        h.header = d.to_vec()
                    }
                    _ => {}
                }
            }
            out.push(h);
        }
        out.sort_by_key(|h| h.uid);
        Ok(out)
    }

    async fn fetch_previews(&mut self, range: UidRange) -> Result<Vec<(u32, Vec<u8>)>> {
        let to = range.to.map_or("*".into(), |t| t.to_string());
        let cmd = format!("UID FETCH {}:{to} (UID BODY.PEEK[TEXT]<0.{PREVIEW_BYTES}>)", range.from);
        let done = check(self.command(&cmd).await?, "FETCH")?;
        let mut out = vec![];
        for attrs in done.data.iter().filter_map(fetch_attrs) {
            let Some(uid) = uid_of(attrs) else { continue };
            if !range.contains(uid) {
                continue;
            }
            for a in attrs {
                if let AttributeValue::BodySection {
                    section: Some(SectionPath::Full(MessageSection::Text)),
                    data,
                    ..
                } = a
                {
                    out.push((uid, data.as_deref().unwrap_or_default().to_vec()));
                }
            }
        }
        Ok(out)
    }

    async fn fetch_flags(&mut self, range: UidRange, changed_since: Option<u64>) -> Result<Vec<FlagUpdate>> {
        let to = range.to.map_or("*".into(), |t| t.to_string());
        let modseq = if self.caps.condstore { " MODSEQ" } else { "" };
        let since = changed_since.map_or(String::new(), |m| format!(" (CHANGEDSINCE {m})"));
        let cmd = format!("UID FETCH {}:{to} (UID FLAGS{modseq}){since}", range.from);
        let done = check(self.command(&cmd).await?, "FETCH")?;
        let mut out = vec![];
        for attrs in done.data.iter().filter_map(fetch_attrs) {
            let Some(uid) = uid_of(attrs) else { continue };
            if !range.contains(uid) {
                continue;
            }
            let mut u = FlagUpdate { uid, flags: Flags::default(), modseq: None };
            for a in attrs {
                match a {
                    AttributeValue::Flags(f) => u.flags = flags_of(f),
                    AttributeValue::ModSeq(m) => u.modseq = Some(*m),
                    _ => {}
                }
            }
            out.push(u);
        }
        Ok(out)
    }

    async fn fetch_message(&mut self, uid: u32) -> Result<Vec<u8>> {
        let done = check(self.command(&format!("UID FETCH {uid} (UID BODY.PEEK[])")).await?, "FETCH")?;
        for attrs in done.data.iter().filter_map(fetch_attrs) {
            if uid_of(attrs) != Some(uid) {
                continue;
            }
            for a in attrs {
                if let AttributeValue::BodySection { data: Some(d), .. } | AttributeValue::Rfc822(Some(d)) = a {
                    return Ok(d.to_vec());
                }
            }
        }
        Err(Error::NotFound(
            tr!("Die Nachricht ist nicht mehr auf dem Server.", "The message is no longer on the server.").into(),
        ))
    }

    async fn fetch_structure(&mut self, uid: u32) -> Result<Vec<BodyPart>> {
        let done = check(self.command(&format!("UID FETCH {uid} (UID BODYSTRUCTURE)")).await?, "FETCH")?;
        for attrs in done.data.iter().filter_map(fetch_attrs) {
            if uid_of(attrs) != Some(uid) {
                continue;
            }
            for a in attrs {
                if let AttributeValue::BodyStructure(s) = a {
                    return Ok(parts_of(s));
                }
            }
        }
        Err(Error::NotFound(
            tr!("Die Nachricht ist nicht mehr auf dem Server.", "The message is no longer on the server.").into(),
        ))
    }

    async fn fetch_section(&mut self, uid: u32, section: &str) -> Result<Vec<u8>> {
        let cmd = format!("UID FETCH {uid} (UID BODY.PEEK[{section}.MIME] BODY.PEEK[{section}])");
        let done = check(self.command(&cmd).await?, "FETCH")?;
        let (mut mime, mut body) = (None, None);
        for attrs in done.data.iter().filter_map(fetch_attrs) {
            if uid_of(attrs) != Some(uid) {
                continue;
            }
            for a in attrs {
                if let AttributeValue::BodySection { section: Some(s), data, .. } = a {
                    match s {
                        SectionPath::Part(_, Some(MessageSection::Mime)) => mime = data.clone(),
                        SectionPath::Part(_, None) => body = data.clone(),
                        _ => {}
                    }
                }
            }
        }
        let body = body.ok_or_else(|| Error::NotFound(tr!("Der Teil fehlt.", "The part is missing.").into()))?;
        let mut out = mime.map(|m| m.to_vec()).unwrap_or_else(|| b"\r\n".to_vec());
        out.extend_from_slice(&body);
        Ok(out)
    }

    async fn idle(&mut self, timeout: Duration, wake: &Notify) -> Result<IdleOutcome> {
        if std::mem::take(&mut self.changed) {
            return Ok(IdleOutcome::Changed);
        }
        if !self.caps.idle {
            return Ok(tokio::select! {
                _ = tokio::time::sleep(timeout) => IdleOutcome::Timeout,
                _ = wake.notified() => IdleOutcome::Interrupted,
            });
        }
        let tag = self.next_tag();
        self.write(format!("{tag} IDLE\r\n").as_bytes()).await?;
        loop {
            match self.read_reply(IO_TIMEOUT).await? {
                Reply::Continue(_) => break,
                Reply::Tagged { text, .. } => return Err(Error::Imap(format!("IDLE: {text}"))),
                Reply::Bye(text) => return Err(Problem::Protocol { detail: text }.into()),
                Reply::Data(_) | Reply::Unknown => {}
            }
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let outcome = loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            tokio::select! {
                r = self.read_reply(left + Duration::from_secs(1)) => match r {
                    Ok(Reply::Data(Response::MailboxData(MailboxDatum::Exists(_))))
                    | Ok(Reply::Data(Response::Expunge(_)))
                    | Ok(Reply::Data(Response::Vanished { .. }))
                    | Ok(Reply::Data(Response::Fetch(..))) => break IdleOutcome::Changed,
                    Ok(Reply::Bye(text)) => return Err(Problem::Protocol { detail: text }.into()),
                    Ok(_) => continue,
                    Err(Error::Connection(Problem::Timeout { .. })) => break IdleOutcome::Timeout,
                    Err(e) => return Err(e),
                },
                _ = tokio::time::sleep(left) => break IdleOutcome::Timeout,
                _ = wake.notified() => break IdleOutcome::Interrupted,
            }
        };
        self.write(b"DONE\r\n").await?;
        self.finish(&tag, IO_TIMEOUT).await?;
        Ok(outcome)
    }

    async fn store(&mut self, uids: &[u32], flags: &[&str], add: bool) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let cmd =
            format!("UID STORE {} {}FLAGS.SILENT ({})", uid_set(uids), if add { "+" } else { "-" }, flags.join(" "));
        check(self.command(&cmd).await?, "STORE").map(drop)
    }

    async fn copy(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let done = check(self.command(&format!("UID COPY {} {}", uid_set(uids), mailbox_arg(to))).await?, "COPY")?;
        Ok(uid_map(&done.code))
    }

    async fn move_to(&mut self, uids: &[u32], to: &str) -> Result<Vec<(u32, u32)>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        if self.caps.moves {
            let done = check(self.command(&format!("UID MOVE {} {}", uid_set(uids), mailbox_arg(to))).await?, "MOVE")?;
            // MOVE sends COPYUID as an untagged OK before the expunges (RFC 6851).
            let untagged = done.data.iter().find_map(|d| match d {
                Response::Data { outcome, .. } if matches!(outcome.code, Some(ResponseCode::CopyUid(..))) => {
                    Some(uid_map(&outcome.code))
                }
                _ => None,
            });
            return Ok(untagged.unwrap_or_else(|| uid_map(&done.code)));
        }
        let map = self.copy(uids, to).await?;
        self.expunge(uids).await?;
        Ok(map)
    }

    /// Without UIDPLUS a plain `EXPUNGE` removes every message marked `\\Deleted`, also those
    /// another client only marked (its „mark as deleted“, undo still possible there): their mark
    /// comes off for the moment and goes back on after (RFC 4315, section 1).
    async fn expunge(&mut self, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        if self.caps.uidplus {
            self.store(uids, &[DELETED], true).await?;
            return check(self.command(&format!("UID EXPUNGE {}", uid_set(uids))).await?, "EXPUNGE").map(drop);
        }
        let done = check(self.command("UID SEARCH DELETED").await?, "SEARCH")?;
        let others: Vec<u32> = done
            .data
            .iter()
            .filter_map(|d| match d {
                Response::MailboxData(MailboxDatum::Search(list)) => Some(list.clone()),
                _ => None,
            })
            .flatten()
            .filter(|u| !uids.contains(u))
            .collect();
        self.store(&others, &[DELETED], false).await?;
        self.store(uids, &[DELETED], true).await?;
        let expunged = check(self.command("EXPUNGE").await?, "EXPUNGE").map(drop);
        let restored = self.store(&others, &[DELETED], true).await;
        expunged.and(restored)
    }

    async fn append(&mut self, mailbox: &str, flags: &[&str], raw: &[u8]) -> Result<Option<u32>> {
        let parts = vec![
            Arg::Text(format!("APPEND {} ({}) ", mailbox_arg(mailbox), flags.join(" "))),
            Arg::Literal(raw.to_vec()),
        ];
        let done = check(self.command_with(parts).await?, "APPEND")?;
        Ok(match done.code {
            Some(ResponseCode::AppendUid(_, set)) => set.first().map(|m| match m {
                imap_proto::rfc4315::UidSetMember::Uid(u) => *u,
                imap_proto::rfc4315::UidSetMember::UidRange(r) => *r.start(),
            }),
            _ => None,
        })
    }

    async fn search(&mut self, query: &Search) -> Result<Vec<u32>> {
        let parts = match query {
            Search::Text(t) => {
                if t.is_ascii() {
                    vec![Arg::Text("UID SEARCH TEXT ".into()), Arg::string(t)]
                } else {
                    vec![Arg::Text("UID SEARCH CHARSET UTF-8 TEXT ".into()), Arg::Literal(t.as_bytes().to_vec())]
                }
            }
            Search::MessageId(id) => {
                vec![
                    Arg::Text("UID SEARCH HEADER Message-ID ".into()),
                    Arg::string(&format!("<{}>", id.trim_matches(['<', '>']))),
                ]
            }
        };
        let done = check(self.command_with(parts).await?, "SEARCH")?;
        let mut out: Vec<u32> = done
            .data
            .iter()
            .flat_map(|d| match d {
                Response::MailboxData(MailboxDatum::Search(v)) => v.clone(),
                _ => vec![],
            })
            .collect();
        out.sort_unstable();
        Ok(out)
    }

    async fn create(&mut self, mailbox: &str) -> Result<()> {
        self.ok(&format!("CREATE {}", mailbox_arg(mailbox))).await.map(drop)
    }

    async fn delete(&mut self, mailbox: &str) -> Result<()> {
        self.ok(&format!("DELETE {}", mailbox_arg(mailbox))).await.map(drop)
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.ok(&format!("RENAME {} {}", mailbox_arg(from), mailbox_arg(to))).await.map(drop)
    }

    async fn logout(&mut self) -> Result<()> {
        let _ = self.command("LOGOUT").await;
        let _ = self.stream.shutdown().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_dates() {
        assert_eq!(parse_internal_date("16-Oct-2026 09:30:00 +0200"), Some(1_792_135_800));
        assert_eq!(parse_internal_date(" 1-Jan-1970 00:00:00 +0000"), Some(0));
        assert_eq!(parse_internal_date("31-Dec-1999 23:00:00 -0100"), Some(946_684_800));
        assert_eq!(parse_internal_date("garbage"), None);
    }

    #[test]
    fn uid_sets() {
        assert_eq!(uid_set(&[5, 1, 2, 3, 9, 7, 8]), "1:3,5,7:9");
        assert_eq!(uid_set(&[4]), "4");
    }

    #[test]
    fn quoting() {
        assert_eq!(quoted("a\"b\\c").as_deref(), Some("\"a\\\"b\\\\c\""));
        assert_eq!(quoted("Grüße"), None);
        assert_eq!(mailbox_arg("Entwürfe"), "\"Entw&APw-rfe\"");
    }

    #[test]
    fn body_structures() {
        let line = b"* 1 FETCH (UID 7 BODYSTRUCTURE ((\"TEXT\" \"PLAIN\" (\"CHARSET\" \"utf-8\") NIL NIL \"7BIT\" 12 1 NIL NIL NIL NIL)\
(\"APPLICATION\" \"PDF\" (\"NAME\" \"=?utf-8?Q?Gr=C3=BC=C3=9Fe.pdf?=\") NIL NIL \"BASE64\" 4000 NIL (\"attachment\" (\"FILENAME\" \"=?utf-8?Q?Gr=C3=BC=C3=9Fe.pdf?=\")) NIL NIL)\
(\"IMAGE\" \"PNG\" NIL \"<logo@x>\" NIL \"BASE64\" 100 NIL (\"inline\" NIL) NIL NIL) \"MIXED\" (\"BOUNDARY\" \"x\") NIL NIL NIL))\r\n";
        let (_, resp) = Response::parse(line).unwrap();
        let Response::Fetch(_, attrs) = resp else { panic!() };
        let s =
            attrs.iter().find_map(|a| if let AttributeValue::BodyStructure(s) = a { Some(s) } else { None }).unwrap();
        let parts = parts_of(s);
        assert_eq!(parts.len(), 3);
        assert_eq!((parts[0].section.as_str(), parts[0].mime_type.as_str()), ("1", "text/plain"));
        assert!(!parts[0].is_attachment());
        assert_eq!(parts[1].filename.as_deref(), Some("Grüße.pdf"));
        assert!(parts[1].is_attachment() && !parts[1].inline);
        assert_eq!(parts[2].content_id.as_deref(), Some("logo@x"));
        assert!(!parts[2].is_attachment(), "an image the HTML shows");
        assert_eq!(decode_2231("utf-8''Gr%C3%BC%C3%9Fe.pdf"), "Grüße.pdf");
    }

    /// Gmail's labels with brackets, quoted, as an atom or as a literal, and its German folders
    /// in modified UTF-7: the names come through whole.
    #[test]
    fn gmail_list_names() {
        let name = |line: &[u8]| {
            let (_, resp) = Response::parse(line).unwrap();
            let Response::MailboxData(MailboxDatum::List(l)) = resp else { panic!("{resp:?}") };
            utf7::decode(&l.name)
        };
        assert_eq!(name(b"* LIST (\\HasNoChildren) \"/\" \"[Notion]\"\r\n"), "[Notion]");
        assert_eq!(name(b"* LIST (\\HasNoChildren) \"/\" [Notion]\r\n"), "[Notion]");
        assert_eq!(name(b"* LIST (\\HasNoChildren) \"/\" {8}\r\n[Notion]\r\n"), "[Notion]");
        assert_eq!(name(b"* LIST (\\HasNoChildren) \"/\" \"[Notion]/Seiten\"\r\n"), "[Notion]/Seiten");
        assert_eq!(name(b"* LIST (\\Drafts \\HasNoChildren) \"/\" \"[Gmail]/Entw&APw-rfe\"\r\n"), "[Gmail]/Entwürfe");
        assert_eq!(name(b"* LIST (\\HasNoChildren) \"/\" \"Entw&APw-rfe\"\r\n"), "Entwürfe");
    }

    /// A server on loopback that answers the login with `login` and `ENABLE` with `enable`.
    async fn scripted(login: &'static str, enable: &'static str) -> u16 {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut io = BufReader::new(sock);
            let caps = "IMAP4rev1 AUTH=PLAIN ENABLE CONDSTORE QRESYNC IDLE";
            io.get_mut().write_all(format!("* OK [CAPABILITY {caps}] ready\r\n").as_bytes()).await.unwrap();
            loop {
                let mut line = String::new();
                if io.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let (tag, cmd) = line.trim_end().split_once(' ').unwrap_or(("*", ""));
                let out = match cmd.to_ascii_uppercase().as_str() {
                    "CAPABILITY" => format!("* CAPABILITY {caps}\r\n{tag} OK done\r\n"),
                    "AUTHENTICATE PLAIN" => {
                        io.get_mut().write_all(b"+ \r\n").await.unwrap();
                        let mut token = String::new();
                        io.read_line(&mut token).await.unwrap();
                        format!("{tag} {login}\r\n")
                    }
                    c if c.starts_with("ENABLE") => format!("{tag} {enable}\r\n"),
                    "LOGOUT" => format!("* BYE\r\n{tag} OK bye\r\n"),
                    _ => format!("{tag} BAD what\r\n"),
                };
                io.get_mut().write_all(out.as_bytes()).await.unwrap();
            }
        });
        port
    }

    /// RFC 5530: a login refused for a reason of the server's (its authentication backend is
    /// down, too many sessions) is no wrong password.
    #[tokio::test]
    async fn temporary_login_failures_are_no_wrong_password() {
        for code in [
            "[UNAVAILABLE] Temporary authentication failure",
            "[INUSE] Mailbox in use",
            "[LIMIT] Too many sessions",
            "[SERVERBUG] Oops",
        ] {
            let port = scripted(Box::leak(format!("NO {code}").into_boxed_str()), "OK").await;
            let server = Server::new("127.0.0.1", port, crate::account::Security::Plain);
            let e = ImapSession::connect(&server, "maria", "pw").await.err().unwrap();
            let sign_in = matches!(&e, Error::Connection(p) if p.needs_sign_in());
            assert!(!sign_in && crate::engine::is_transient(&e), "{code}: {e:?}");
        }
        let port = scripted("NO [AUTHENTICATIONFAILED] Invalid credentials", "OK").await;
        let server = Server::new("127.0.0.1", port, crate::account::Security::Plain);
        let e = ImapSession::connect(&server, "maria", "pw").await.err().unwrap();
        assert!(matches!(&e, Error::Connection(p) if p.needs_sign_in()), "{e:?}");
    }

    /// A server that offers QRESYNC but refuses to switch it on: no `SELECT … (QRESYNC …)`,
    /// which it would refuse for every folder.
    #[tokio::test]
    async fn a_refused_enable_turns_qresync_off() {
        let port = scripted("OK signed in", "NO not today").await;
        let server = Server::new("127.0.0.1", port, crate::account::Security::Plain);
        let mut c = ImapSession::connect(&server, "maria", "pw").await.unwrap();
        let caps = c.capabilities().await.unwrap();
        assert!(!caps.qresync, "{caps:?}");
        assert!(caps.condstore && caps.idle);
        let port = scripted("OK signed in", "OK").await;
        let server = Server::new("127.0.0.1", port, crate::account::Security::Plain);
        let mut c = ImapSession::connect(&server, "maria", "pw").await.unwrap();
        assert!(c.capabilities().await.unwrap().qresync);
    }
}
