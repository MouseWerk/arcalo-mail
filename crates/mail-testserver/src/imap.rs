//! The IMAP side: commands are read with their literals, cut into tokens and answered from the
//! fake's state, as a real server would (sequence numbers, literals, response codes).

use std::sync::Arc;

use mail_core::imap::{Flags, QresyncParams, Search, utf7};
use mail_parser::{MessageParser, MimeHeaders, PartType};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::{Io, Shared, internal_date, upgrade};

#[derive(Debug, Clone)]
enum Tok {
    Atom(String),
    Str(Vec<u8>),
    List(Vec<Tok>),
}

impl Tok {
    fn text(&self) -> String {
        match self {
            Tok::Atom(a) => a.clone(),
            Tok::Str(s) => String::from_utf8_lossy(s).into_owned(),
            Tok::List(l) => l.iter().map(Tok::text).collect::<Vec<_>>().join(" "),
        }
    }

    fn upper(&self) -> String {
        self.text().to_ascii_uppercase()
    }
}

const MARK: char = '\u{1}';

/// Tokens of a command; literals were replaced by `\u{1}<index>\u{1}`.
fn tokens(s: &str, lits: &[Vec<u8>]) -> Vec<Tok> {
    fn list(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, lits: &[Vec<u8>], until: Option<char>) -> Vec<Tok> {
        let mut out = vec![];
        while let Some(&c) = chars.peek() {
            match c {
                ' ' => {
                    chars.next();
                }
                ')' if until == Some(')') => {
                    chars.next();
                    return out;
                }
                '(' => {
                    chars.next();
                    out.push(Tok::List(list(chars, lits, Some(')'))));
                }
                '"' => {
                    chars.next();
                    let mut v = String::new();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => v.extend(chars.next()),
                            '"' => break,
                            c => v.push(c),
                        }
                    }
                    out.push(Tok::Str(v.into_bytes()));
                }
                MARK => {
                    chars.next();
                    let n: String = chars.by_ref().take_while(|c| *c != MARK).collect();
                    out.push(Tok::Str(lits.get(n.parse::<usize>().unwrap_or(0)).cloned().unwrap_or_default()));
                }
                _ => {
                    let mut v = String::new();
                    let mut depth = 0;
                    while let Some(&c) = chars.peek() {
                        if depth == 0 && (c == ' ' || c == ')' || c == '(') {
                            break;
                        }
                        if c == '[' {
                            depth += 1;
                        }
                        if c == ']' {
                            depth -= 1;
                        }
                        v.push(c);
                        chars.next();
                    }
                    out.push(Tok::Atom(v));
                }
            }
        }
        out
    }
    list(&mut s.chars().peekable(), lits, None)
}

struct Session {
    shared: Arc<Shared>,
    io: BufReader<Io>,
    authed: bool,
    tls: bool,
    selected: Option<String>,
    qresync: bool,
}

/// UIDs of a set (`1:3,5,7:*`) among the existing ones.
fn uids_in(set: &str, existing: &[u32]) -> Vec<u32> {
    let max = existing.last().copied().unwrap_or(0);
    let mut out = vec![];
    for part in set.split(',') {
        let (a, b) = part.split_once(':').unwrap_or((part, part));
        let n = |x: &str| if x == "*" { Some(max) } else { x.parse::<u32>().ok() };
        let (Some(a), Some(b)) = (n(a), n(b)) else { continue };
        let (lo, hi) = (a.min(b), a.max(b));
        out.extend(existing.iter().copied().filter(|u| *u >= lo && *u <= hi));
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn flags_text(f: &Flags) -> String {
    let mut v: Vec<String> = vec![];
    for (on, name) in [
        (f.seen, "\\Seen"),
        (f.answered, "\\Answered"),
        (f.flagged, "\\Flagged"),
        (f.deleted, "\\Deleted"),
        (f.draft, "\\Draft"),
    ] {
        if on {
            v.push(name.into());
        }
    }
    v.extend(f.keywords.iter().cloned());
    format!("({})", v.join(" "))
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn nstring(s: Option<&str>) -> String {
    s.map_or("NIL".into(), quote)
}

fn part_by_section<'a>(m: &'a mail_parser::Message<'a>, section: &str) -> Option<&'a mail_parser::MessagePart<'a>> {
    let mut id = 0u32;
    for (depth, n) in section.split('.').enumerate() {
        let n: usize = n.parse().ok()?;
        match &m.part(id)?.body {
            PartType::Multipart(children) => id = *children.get(n.checked_sub(1)?)?,
            _ if depth == 0 && n == 1 => {}
            _ => return None,
        }
    }
    m.part(id)
}

/// BODYSTRUCTURE of a parsed message (RFC 3501, 7.4.2), with the extension data clients read.
fn body_structure(m: &mail_parser::Message<'_>, id: u32) -> String {
    let Some(p) = m.part(id) else { return "NIL".into() };
    if let PartType::Multipart(children) = &p.body {
        let inner: String = children.iter().map(|c| body_structure(m, *c)).collect();
        let sub = p.content_type().and_then(|c| c.subtype()).unwrap_or("mixed").to_ascii_uppercase();
        let boundary = p.content_type().and_then(|c| c.attribute("boundary")).unwrap_or("b");
        return format!("({inner} {} ({} {}) NIL NIL NIL)", quote(&sub), quote("BOUNDARY"), quote(boundary));
    }
    let ct = p.content_type();
    let ty = ct.map_or("TEXT".to_string(), |c| c.ctype().to_ascii_uppercase());
    let sub = ct.and_then(|c| c.subtype()).map_or("PLAIN".to_string(), |s| s.to_ascii_uppercase());
    let mut params = vec![];
    if let Some(cs) = ct.and_then(|c| c.attribute("charset")) {
        params.push(format!("{} {}", quote("CHARSET"), quote(cs)));
    }
    if let Some(name) = ct.and_then(|c| c.attribute("name")) {
        params.push(format!("{} {}", quote("NAME"), quote(name)));
    }
    let params = if params.is_empty() { "NIL".into() } else { format!("({})", params.join(" ")) };
    let id_ = p.content_id().map(|c| format!("<{}>", c.trim_matches(['<', '>'])));
    let enc = match p.encoding {
        mail_parser::Encoding::Base64 => "BASE64",
        mail_parser::Encoding::QuotedPrintable => "QUOTED-PRINTABLE",
        _ => "7BIT",
    };
    let size = p.offset_end.saturating_sub(p.offset_body);
    let raw = m.raw_message();
    let body = raw.get(p.offset_body as usize..p.offset_end as usize).unwrap_or_default();
    let disposition = match p.content_disposition() {
        Some(d) => {
            let fname = d.attribute("filename").map(|f| format!("({} {})", quote("FILENAME"), quote(f)));
            format!("({} {})", quote(d.ctype()), fname.unwrap_or_else(|| "NIL".into()))
        }
        None => "NIL".into(),
    };
    let common =
        format!("{} {} {params} {} NIL {} {size}", quote(&ty), quote(&sub), nstring(id_.as_deref()), quote(enc));
    if ty == "TEXT" {
        let lines = body.iter().filter(|b| **b == b'\n').count();
        format!("({common} {lines} NIL {disposition} NIL NIL)")
    } else {
        format!("({common} NIL {disposition} NIL NIL)")
    }
}

/// A literal for a FETCH answer.
fn literal(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(format!("{{{}}}\r\n", data.len()).as_bytes());
    out.extend_from_slice(data);
}

impl Session {
    async fn write(&mut self, s: &str) -> std::io::Result<()> {
        self.io.get_mut().write_all(s.as_bytes()).await?;
        self.io.get_mut().flush().await
    }

    async fn write_bytes(&mut self, b: &[u8]) -> std::io::Result<()> {
        self.io.get_mut().write_all(b).await?;
        self.io.get_mut().flush().await
    }

    /// One command line with its literals, `None` at the end of the connection.
    async fn read_command(&mut self) -> std::io::Result<Option<(String, Vec<Vec<u8>>)>> {
        let mut text = String::new();
        let mut lits = vec![];
        loop {
            let mut line = Vec::new();
            if self.io.read_until(b'\n', &mut line).await? == 0 {
                return Ok(None);
            }
            let l = String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string();
            if let Some(open) = l.rfind('{')
                && l.ends_with('}')
            {
                let spec = &l[open + 1..l.len() - 1];
                let plus = spec.ends_with('+');
                if let Ok(n) = spec.trim_end_matches('+').parse::<usize>() {
                    text.push_str(&l[..open]);
                    text.push_str(&format!("{MARK}{}{MARK}", lits.len()));
                    if !plus {
                        self.write("+ Ready for literal data\r\n").await?;
                    }
                    let mut buf = vec![0u8; n];
                    self.io.read_exact(&mut buf).await?;
                    lits.push(buf);
                    continue;
                }
            }
            text.push_str(&l);
            return Ok(Some((text, lits)));
        }
    }

    fn caps_line(&self) -> String {
        let c = self.shared.fake.lock().unwrap().caps;
        let mut v = vec!["IMAP4rev1", "LITERAL+", "ENABLE", "ID", "UNSELECT", "AUTH=PLAIN", "AUTH=XOAUTH2", "SASL-IR"];
        if !self.tls && crate::acceptor(&self.shared).is_some() {
            v.push("STARTTLS");
        }
        for (on, name) in [
            (c.idle, "IDLE"),
            (c.condstore, "CONDSTORE"),
            (c.qresync, "QRESYNC"),
            (c.uidplus, "UIDPLUS"),
            (c.moves, "MOVE"),
            (c.special_use, "SPECIAL-USE"),
            (c.gmail, "X-GM-EXT-1"),
        ] {
            if on {
                v.push(name);
            }
        }
        v.join(" ")
    }

    fn uids_of_selected(&self) -> Vec<u32> {
        let f = self.shared.fake.lock().unwrap();
        self.selected
            .as_ref()
            .and_then(|s| f.mailboxes.get(s))
            .map(|m| m.messages.keys().copied().collect())
            .unwrap_or_default()
    }

    /// Runs `f` on the fake with this connection's folder selected.
    fn fake<R>(&mut self, f: impl FnOnce(&mut mail_core::imap::fake::FakeImap) -> R) -> R {
        let mut g = self.shared.fake.lock().unwrap();
        g.selected = self.selected.clone();
        let r = f(&mut g);
        self.selected = g.selected.take();
        r
    }

    /// `* n EXPUNGE` (or VANISHED with QRESYNC) for UIDs about to go, highest first.
    fn expunge_lines(&self, uids: &[u32]) -> String {
        if uids.is_empty() {
            return String::new();
        }
        if self.qresync {
            return format!("* VANISHED {}\r\n", mail_core::imap::uid_set(uids));
        }
        let existing = self.uids_of_selected();
        let mut seqs: Vec<usize> =
            uids.iter().filter_map(|u| existing.iter().position(|e| e == u).map(|p| p + 1)).collect();
        seqs.sort_unstable();
        // Each EXPUNGE renumbers the ones after it: report from the highest down.
        seqs.iter().rev().map(|s| format!("* {s} EXPUNGE\r\n")).collect()
    }

    async fn fetch(&mut self, tag: &str, args: &[Tok], by_uid: bool) -> std::io::Result<()> {
        let set = args.first().map(Tok::text).unwrap_or_default();
        let items: Vec<String> = match args.get(1) {
            Some(Tok::List(l)) => l.iter().map(Tok::upper).collect(),
            Some(t) => vec![t.upper()],
            None => vec![],
        };
        let changed_since = match args.get(2) {
            Some(Tok::List(l)) if l.first().map(Tok::upper).as_deref() == Some("CHANGEDSINCE") => {
                l.get(1).and_then(|t| t.text().parse::<u64>().ok())
            }
            _ => None,
        };
        let existing = self.uids_of_selected();
        let uids = if by_uid {
            uids_in(&set, &existing)
        } else {
            // Sequence numbers (FETCH * (UID)).
            let seqs: Vec<u32> = (1..=existing.len() as u32).collect();
            uids_in(&set, &seqs).into_iter().filter_map(|s| existing.get(s as usize - 1).copied()).collect()
        };
        let condstore = self.shared.fake.lock().unwrap().caps.condstore;
        let headers = items.iter().any(|i| i.starts_with("BODY.PEEK[HEADER") || i == "ENVELOPE");
        let body = items.iter().any(|i| {
            i == "RFC822"
                || (i.starts_with("BODY") && i.contains('[') && !i.contains("[HEADER") && !i.contains(".MIME]"))
        });
        if headers && self.cut_now() {
            // A sync cut short: the connection drops instead of the next batch.
            self.shared.set_offline(true);
            return Err(std::io::Error::other("cut"));
        }
        let wait = self.slowness(headers, body, &uids);
        if wait > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        }
        let mut out = Vec::new();
        let mut sent = vec![];
        for uid in uids {
            let seq = existing.iter().position(|u| *u == uid).map_or(0, |p| p + 1);
            let msg = {
                let f = self.shared.fake.lock().unwrap();
                self.selected.as_ref().and_then(|s| f.mailboxes.get(s)).and_then(|m| m.messages.get(&uid)).cloned()
            };
            let Some(msg) = msg else { continue };
            if changed_since.is_some_and(|c| msg.modseq <= c) {
                continue;
            }
            out.extend_from_slice(format!("* {seq} FETCH (UID {uid}").as_bytes());
            sent.push(uid);
            let parsed = MessageParser::default().parse(&msg.raw);
            let mut seen_now = false;
            for item in &items {
                match item.as_str() {
                    "UID" => {}
                    "FLAGS" => out.extend_from_slice(format!(" FLAGS {}", flags_text(&msg.flags)).as_bytes()),
                    "INTERNALDATE" => out.extend_from_slice(
                        format!(" INTERNALDATE \"{}\"", internal_date(msg.internal_date)).as_bytes(),
                    ),
                    "RFC822.SIZE" => out.extend_from_slice(format!(" RFC822.SIZE {}", msg.raw.len()).as_bytes()),
                    "MODSEQ" => out.extend_from_slice(format!(" MODSEQ ({})", msg.modseq).as_bytes()),
                    "X-GM-MSGID" => {
                        if let Some(id) = msg.gm_msgid {
                            out.extend_from_slice(format!(" X-GM-MSGID {id}").as_bytes());
                        }
                    }
                    "BODYSTRUCTURE" => {
                        let s = parsed.as_ref().map_or("NIL".into(), |m| body_structure(m, 0));
                        out.extend_from_slice(format!(" BODYSTRUCTURE {s}").as_bytes());
                    }
                    other if other.starts_with("BODY") => {
                        let peek = other.starts_with("BODY.PEEK");
                        // `BODY.PEEK[TEXT]<0.2048>`: a part of the section (RFC 3501, 6.4.5).
                        let (item, partial) = match other.split_once("]<") {
                            Some((item, range)) => {
                                let (from, len) = range.trim_end_matches('>').split_once('.').unwrap_or(("0", "0"));
                                (
                                    format!("{item}]"),
                                    Some((from.parse::<usize>().unwrap_or(0), len.parse::<usize>().unwrap_or(0))),
                                )
                            }
                            None => (other.to_string(), None),
                        };
                        let section = item.split_once('[').map(|(_, r)| r.trim_end_matches(']')).unwrap_or("");
                        let data: Vec<u8> = match section {
                            "" => msg.raw.clone(),
                            "HEADER" => {
                                let end =
                                    msg.raw.windows(4).position(|w| w == b"\r\n\r\n").map_or(msg.raw.len(), |p| p + 4);
                                msg.raw[..end].to_vec()
                            }
                            "TEXT" => {
                                let end =
                                    msg.raw.windows(4).position(|w| w == b"\r\n\r\n").map_or(msg.raw.len(), |p| p + 4);
                                msg.raw[end..].to_vec()
                            }
                            s => {
                                let (num, mime) = match s.strip_suffix(".MIME") {
                                    Some(n) => (n, true),
                                    None => (s, false),
                                };
                                parsed
                                    .as_ref()
                                    .and_then(|m| part_by_section(m, num))
                                    .map(|p| {
                                        let (a, b) = if mime {
                                            (p.offset_header, p.offset_body)
                                        } else {
                                            (p.offset_body, p.offset_end)
                                        };
                                        msg.raw.get(a as usize..b as usize).unwrap_or_default().to_vec()
                                    })
                                    .unwrap_or_default()
                            }
                        };
                        match partial {
                            Some((from, len)) => {
                                let part =
                                    data.get(from.min(data.len())..(from + len).min(data.len())).unwrap_or_default();
                                out.extend_from_slice(format!(" BODY[{section}]<{from}> ").as_bytes());
                                literal(&mut out, part);
                            }
                            None => {
                                out.extend_from_slice(format!(" BODY[{section}] ").as_bytes());
                                literal(&mut out, &data);
                            }
                        }
                        if !peek && !msg.flags.seen {
                            seen_now = true;
                        }
                    }
                    _ => {}
                }
            }
            if condstore && !items.iter().any(|i| i == "MODSEQ") && changed_since.is_some() {
                out.extend_from_slice(format!(" MODSEQ ({})", msg.modseq).as_bytes());
            }
            out.extend_from_slice(b")\r\n");
            if seen_now {
                let _ = self.fake(|f| f.do_store(&[uid], &["\\Seen"], true));
                self.shared.changed();
            }
        }
        self.write_bytes(&out).await?;
        if headers {
            self.served(&sent);
        }
        self.write(&format!("{tag} OK FETCH completed\r\n")).await
    }

    /// How long a FETCH waits on the slow server (milliseconds): header FETCHes, and bodies of
    /// old, large messages.
    fn slowness(&self, headers: bool, body: bool, uids: &[u32]) -> u64 {
        let mut wait = 0;
        if headers {
            wait += *self.shared.slow_headers_ms.lock().unwrap();
        }
        let slow = *self.shared.slow_old_body_ms.lock().unwrap();
        if body && slow > 0 {
            let now = crate::now();
            let f = self.shared.fake.lock().unwrap();
            let m = self.selected.as_ref().and_then(|s| f.mailboxes.get(s));
            let old_large = uids
                .iter()
                .filter_map(|u| m.and_then(|m| m.messages.get(u)))
                .any(|x| x.raw.len() >= crate::gmail::LARGE && now - x.internal_date > crate::gmail::OLD);
            if old_large {
                wait += slow;
            }
        }
        wait
    }

    /// Notes the headers sent.
    fn served(&self, uids: &[u32]) {
        let folder = self.selected.clone().unwrap_or_default();
        let mut served = self.shared.served.lock().unwrap();
        let s = served.entry(folder).or_default();
        s.batches += 1;
        s.headers += uids.len();
        s.uids.extend(uids.iter().copied());
        if let Some(n) = self.shared.cut_after_header_batches.lock().unwrap().as_mut() {
            *n = n.saturating_sub(1);
        }
    }

    /// Whether the connection drops instead of this header FETCH (`cut_after_header_batches`).
    fn cut_now(&self) -> bool {
        *self.shared.cut_after_header_batches.lock().unwrap() == Some(0)
    }

    async fn select(&mut self, tag: &str, args: &[Tok], cmd: &str) -> std::io::Result<()> {
        let name = utf7::decode(&args.first().map(Tok::text).unwrap_or_default());
        let mut qresync = None;
        if let Some(Tok::List(l)) = args.get(1)
            && l.first().map(Tok::upper).as_deref() == Some("QRESYNC")
            && let Some(Tok::List(p)) = l.get(1)
        {
            let v = p.first().and_then(|t| t.text().parse().ok()).unwrap_or(0);
            let m = p.get(1).and_then(|t| t.text().parse().ok()).unwrap_or(0);
            qresync = Some(QresyncParams { uid_validity: v, modseq: m });
        }
        let caps = self.shared.fake.lock().unwrap().caps;
        if qresync.is_some() && !self.qresync {
            return self.write(&format!("{tag} BAD QRESYNC not enabled\r\n")).await;
        }
        let r = self.fake(|f| f.do_select(&name, qresync));
        let sel = match r {
            Ok(s) => s,
            Err(_) => {
                self.selected = None;
                return self.write(&format!("{tag} NO [NONEXISTENT] No such folder\r\n")).await;
            }
        };
        let mut out = String::new();
        out.push_str("* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft $Junk)\r\n");
        out.push_str("* OK [PERMANENTFLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft $Junk \\*)] Flags\r\n");
        out.push_str(&format!("* {} EXISTS\r\n* 0 RECENT\r\n", sel.info.exists));
        out.push_str(&format!("* OK [UIDVALIDITY {}] UIDs valid\r\n", sel.info.uid_validity));
        out.push_str(&format!("* OK [UIDNEXT {}] Predicted next UID\r\n", sel.info.uid_next));
        if caps.condstore {
            out.push_str(&format!("* OK [HIGHESTMODSEQ {}] Highest\r\n", sel.info.highest_modseq.unwrap_or(1)));
        }
        if !sel.vanished.is_empty() {
            out.push_str(&format!("* VANISHED (EARLIER) {}\r\n", mail_core::imap::uid_set(&sel.vanished)));
        }
        let existing = self.uids_of_selected();
        for c in &sel.changed {
            let seq = existing.iter().position(|u| *u == c.uid).map_or(0, |p| p + 1);
            out.push_str(&format!(
                "* {seq} FETCH (UID {} FLAGS {} MODSEQ ({}))\r\n",
                c.uid,
                flags_text(&c.flags),
                c.modseq.unwrap_or(1)
            ));
        }
        let mode = if cmd == "EXAMINE" { "READ-ONLY" } else { "READ-WRITE" };
        out.push_str(&format!("{tag} OK [{mode}] {cmd} completed\r\n"));
        self.write(&out).await
    }

    async fn search(&mut self, tag: &str, args: &[Tok]) -> std::io::Result<()> {
        let mut i = 0;
        if args.first().map(Tok::upper).as_deref() == Some("CHARSET") {
            i = 2;
        }
        let key = args.get(i).map(Tok::upper).unwrap_or_default();
        let r = match key.as_str() {
            "ALL" | "" => self.fake(|f| f.do_uids()),
            "TEXT" | "BODY" | "SUBJECT" => {
                let q = args.get(i + 1).map(Tok::text).unwrap_or_default();
                self.fake(|f| f.do_search(&Search::Text(q)))
            }
            "HEADER" => {
                let q = args.get(i + 2).map(Tok::text).unwrap_or_default();
                self.fake(|f| f.do_search(&Search::MessageId(q)))
            }
            // Messages marked for removal (another client's „mark as deleted“).
            "DELETED" => {
                let f = self.shared.fake.lock().unwrap();
                let m = self.selected.as_ref().and_then(|s| f.mailboxes.get(s));
                Ok(m.map(|m| m.messages.iter().filter(|(_, x)| x.flags.deleted).map(|(u, _)| *u).collect())
                    .unwrap_or_default())
            }
            _ => Ok(vec![]),
        };
        match r {
            Ok(uids) => {
                let list: Vec<String> = uids.iter().map(u32::to_string).collect();
                let sep = if list.is_empty() { "" } else { " " };
                self.write(&format!("* SEARCH{sep}{}\r\n{tag} OK SEARCH completed\r\n", list.join(" "))).await
            }
            Err(e) => self.write(&format!("{tag} NO {e}\r\n")).await,
        }
    }

    async fn idle(&mut self, tag: &str) -> std::io::Result<()> {
        let mut changes = self.shared.subscribe_changes();
        changes.borrow_and_update();
        self.write("+ idling\r\n").await?;
        let mut line = Vec::new();
        loop {
            tokio::select! {
                r = self.io.read_until(b'\n', &mut line) => {
                    if r? == 0 {
                        return Ok(());
                    }
                    if String::from_utf8_lossy(&line).trim().eq_ignore_ascii_case("DONE") {
                        return self.write(&format!("{tag} OK IDLE terminated\r\n")).await;
                    }
                    line.clear();
                }
                c = changes.changed() => {
                    if c.is_err() {
                        return Ok(());
                    }
                    let exists = self.uids_of_selected().len();
                    self.write(&format!("* {exists} EXISTS\r\n")).await?;
                }
            }
        }
    }

    async fn result(&mut self, tag: &str, cmd: &str, r: mail_core::Result<String>) -> std::io::Result<()> {
        match r {
            Ok(code) => {
                self.shared.changed();
                self.write(&format!("{tag} OK {code}{cmd} completed\r\n")).await
            }
            Err(e) => {
                let text = e.to_string();
                let text = text.strip_prefix("NO ").unwrap_or(&text).to_string();
                self.write(&format!("{tag} NO {text}\r\n")).await
            }
        }
    }

    /// Runs commands until the connection ends; returns the connection when the client asked
    /// for STARTTLS.
    async fn run(&mut self) -> std::io::Result<bool> {
        loop {
            let Some((text, lits)) = self.read_command().await? else { return Ok(false) };
            let toks = tokens(&text, &lits);
            let tag = toks.first().map(Tok::text).unwrap_or_else(|| "*".into());
            let mut cmd = toks.get(1).map(Tok::upper).unwrap_or_default();
            let mut args: Vec<Tok> = toks.iter().skip(2).cloned().collect();
            let by_uid = cmd == "UID";
            if by_uid {
                cmd = args.first().map(Tok::upper).unwrap_or_default();
                args.remove(0);
            }
            let tag = tag.as_str();
            let needs_auth = !matches!(
                cmd.as_str(),
                "CAPABILITY" | "NOOP" | "LOGOUT" | "LOGIN" | "AUTHENTICATE" | "STARTTLS" | "ID"
            );
            if needs_auth && !self.authed {
                self.write(&format!("{tag} BAD Sign in first\r\n")).await?;
                continue;
            }
            match cmd.as_str() {
                "CAPABILITY" => {
                    let caps = self.caps_line();
                    self.write(&format!("* CAPABILITY {caps}\r\n{tag} OK CAPABILITY completed\r\n")).await?;
                }
                "NOOP" | "CHECK" => self.write(&format!("{tag} OK NOOP completed\r\n")).await?,
                "ID" => self.write(&format!("* ID NIL\r\n{tag} OK ID completed\r\n")).await?,
                "LOGOUT" => {
                    self.write(&format!("* BYE Logging out\r\n{tag} OK LOGOUT completed\r\n")).await?;
                    return Ok(false);
                }
                "STARTTLS" => {
                    if self.tls || crate::acceptor(&self.shared).is_none() {
                        self.write(&format!("{tag} BAD STARTTLS not available\r\n")).await?;
                    } else {
                        self.write(&format!("{tag} OK Begin TLS negotiation now\r\n")).await?;
                        return Ok(true);
                    }
                }
                "LOGIN" => {
                    let user = args.first().map(Tok::text).unwrap_or_default();
                    let pw = args.get(1).map(Tok::text).unwrap_or_default();
                    if self.shared.check_login(&user, &pw) {
                        self.authed = true;
                        let caps = self.caps_line();
                        self.write(&format!("{tag} OK [CAPABILITY {caps}] Logged in\r\n")).await?;
                    } else {
                        self.write(&format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")).await?;
                    }
                }
                "AUTHENTICATE" if args.first().map(Tok::upper).as_deref() == Some("XOAUTH2") => {
                    use base64::Engine;
                    let token = match args.get(1) {
                        Some(t) => t.text(),
                        None => {
                            self.write("+ \r\n").await?;
                            let mut line = String::new();
                            self.io.read_line(&mut line).await?;
                            line.trim().to_string()
                        }
                    };
                    let decoded =
                        base64::engine::general_purpose::STANDARD.decode(token.as_bytes()).unwrap_or_default();
                    if crate::check_xoauth2(&self.shared, &String::from_utf8_lossy(&decoded)) {
                        self.authed = true;
                        self.write(&format!("{tag} OK Logged in\r\n")).await?;
                    } else {
                        // As Gmail and Exchange Online: a JSON error as a challenge, then NO.
                        let err = base64::engine::general_purpose::STANDARD
                            .encode(r#"{"status":"401","schemes":"Bearer","scope":"https://outlook.office.com/IMAP.AccessAsUser.All"}"#);
                        self.write(&format!("+ {err}\r\n")).await?;
                        let mut line = String::new();
                        self.io.read_line(&mut line).await?;
                        self.write(&format!("{tag} NO [AUTHENTICATIONFAILED] AUTHENTICATE failed.\r\n")).await?;
                    }
                }
                "AUTHENTICATE" => {
                    use base64::Engine;
                    let token = match args.get(1) {
                        Some(t) => t.text(),
                        None => {
                            self.write("+ \r\n").await?;
                            let mut line = String::new();
                            self.io.read_line(&mut line).await?;
                            line.trim().to_string()
                        }
                    };
                    let decoded =
                        base64::engine::general_purpose::STANDARD.decode(token.as_bytes()).unwrap_or_default();
                    let parts: Vec<String> =
                        decoded.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
                    let ok = parts.len() == 3 && self.shared.check_login(&parts[1], &parts[2]);
                    if ok {
                        self.authed = true;
                        self.write(&format!("{tag} OK Logged in\r\n")).await?;
                    } else {
                        self.write(&format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")).await?;
                    }
                }
                "ENABLE" => {
                    let what: Vec<String> = args.iter().map(Tok::upper).collect();
                    let caps = self.shared.fake.lock().unwrap().caps;
                    let mut on = vec![];
                    if what.iter().any(|w| w == "QRESYNC") && caps.qresync {
                        self.qresync = true;
                        on.push("QRESYNC");
                    }
                    if what.iter().any(|w| w == "CONDSTORE") && caps.condstore {
                        on.push("CONDSTORE");
                    }
                    self.write(&format!("* ENABLED {}\r\n{tag} OK ENABLE completed\r\n", on.join(" "))).await?;
                }
                "LIST" | "LSUB" => {
                    let entries = self.fake(|f| f.do_list());
                    let special = self.shared.fake.lock().unwrap().caps.special_use;
                    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
                    let mut out = String::new();
                    for e in entries {
                        let parent =
                            names.iter().any(|n| n.strip_prefix(e.name.as_str()).is_some_and(|r| r.starts_with('/')));
                        let attrs: Vec<&str> = e
                            .attributes
                            .iter()
                            .map(String::as_str)
                            .filter(|a| special || !a.starts_with('\\') || a.eq_ignore_ascii_case("\\Noselect"))
                            .collect();
                        let mut all = vec![];
                        if !attrs.iter().any(|a| a.eq_ignore_ascii_case("\\HasChildren")) {
                            all.push(if parent { "\\HasChildren" } else { "\\HasNoChildren" });
                        }
                        all.extend(attrs);
                        out.push_str(&format!(
                            "* {cmd} ({}) \"/\" {}\r\n",
                            all.join(" "),
                            quote(&utf7::encode(&e.name))
                        ));
                    }
                    out.push_str(&format!("{tag} OK {cmd} completed\r\n"));
                    self.write(&out).await?;
                }
                "SELECT" | "EXAMINE" => self.select(tag, &args, &cmd).await?,
                "UNSELECT" | "CLOSE" => {
                    self.selected = None;
                    self.write(&format!("{tag} OK {cmd} completed\r\n")).await?;
                }
                "FETCH" => self.fetch(tag, &args, by_uid).await?,
                "SEARCH" => self.search(tag, &args).await?,
                "STORE" => {
                    let existing = self.uids_of_selected();
                    let uids = uids_in(&args.first().map(Tok::text).unwrap_or_default(), &existing);
                    let mode = args.get(1).map(Tok::upper).unwrap_or_default();
                    let flags: Vec<String> = match args.get(2) {
                        Some(Tok::List(l)) => l.iter().map(Tok::text).collect(),
                        Some(t) => vec![t.text()],
                        None => vec![],
                    };
                    let refs: Vec<&str> = flags.iter().map(String::as_str).collect();
                    let r = if mode.starts_with('-') {
                        self.fake(|f| f.do_store(&uids, &refs, false))
                    } else if mode.starts_with('+') {
                        self.fake(|f| f.do_store(&uids, &refs, true))
                    } else {
                        // FLAGS: replace; clear the system flags first.
                        self.fake(|f| {
                            f.do_store(&uids, &["\\Seen", "\\Flagged", "\\Answered", "\\Draft", "\\Deleted"], false)?;
                            f.do_store(&uids, &refs, true)
                        })
                    };
                    self.result(tag, "STORE ", r.map(|_| String::new())).await?;
                }
                "COPY" | "MOVE" => {
                    let existing = self.uids_of_selected();
                    let uids = uids_in(&args.first().map(Tok::text).unwrap_or_default(), &existing);
                    let to = utf7::decode(&args.get(1).map(Tok::text).unwrap_or_default());
                    let expunged = if cmd == "MOVE" { self.expunge_lines(&uids) } else { String::new() };
                    let r = self.fake(|f| f.do_copy(&uids, &to, cmd == "MOVE"));
                    let validity = self.shared.fake.lock().unwrap().mailboxes.get(&to).map_or(0, |m| m.uid_validity);
                    match r {
                        Ok(map) => {
                            let code = if map.is_empty() {
                                String::new()
                            } else {
                                let (a, b): (Vec<u32>, Vec<u32>) = map.into_iter().unzip();
                                format!(
                                    "[COPYUID {validity} {} {}] ",
                                    mail_core::imap::uid_set(&a),
                                    mail_core::imap::uid_set(&b)
                                )
                            };
                            self.shared.changed();
                            if cmd == "MOVE" {
                                let ok = if code.is_empty() { String::new() } else { format!("* OK {code}Moved\r\n") };
                                self.write(&format!("{ok}{expunged}{tag} OK MOVE completed\r\n")).await?;
                            } else {
                                self.write(&format!("{tag} OK {code}COPY completed\r\n")).await?;
                            }
                        }
                        Err(e) => self.result(tag, &cmd, Err(e)).await?,
                    }
                }
                "EXPUNGE" => {
                    let existing = self.uids_of_selected();
                    let deleted: Vec<u32> = {
                        let f = self.shared.fake.lock().unwrap();
                        let m = self.selected.as_ref().and_then(|s| f.mailboxes.get(s));
                        existing
                            .iter()
                            .copied()
                            .filter(|u| m.and_then(|m| m.messages.get(u)).is_some_and(|x| x.flags.deleted))
                            .collect()
                    };
                    let uids = if by_uid {
                        let set = uids_in(&args.first().map(Tok::text).unwrap_or_default(), &existing);
                        deleted.into_iter().filter(|u| set.contains(u)).collect()
                    } else {
                        deleted
                    };
                    let lines = self.expunge_lines(&uids);
                    let r = self.fake(|f| f.do_expunge(&uids));
                    match r {
                        Ok(()) => {
                            self.shared.changed();
                            self.write(&format!("{lines}{tag} OK EXPUNGE completed\r\n")).await?;
                        }
                        Err(e) => self.result(tag, "EXPUNGE", Err(e)).await?,
                    }
                }
                "APPEND" => {
                    let name = utf7::decode(&args.first().map(Tok::text).unwrap_or_default());
                    let flags: Vec<String> = match args.get(1) {
                        Some(Tok::List(l)) => l.iter().map(Tok::text).collect(),
                        _ => vec![],
                    };
                    let raw = match args.last() {
                        Some(Tok::Str(s)) => s.clone(),
                        _ => vec![],
                    };
                    let refs: Vec<&str> = flags.iter().map(String::as_str).collect();
                    let r = self.fake(|f| f.do_append(&name, &refs, &raw));
                    let validity = self.shared.fake.lock().unwrap().mailboxes.get(&name).map_or(0, |m| m.uid_validity);
                    let r = r.map(|uid| uid.map_or(String::new(), |u| format!("[APPENDUID {validity} {u}] ")));
                    self.result(tag, "APPEND ", r).await?;
                }
                "CREATE" => {
                    let name = utf7::decode(&args.first().map(Tok::text).unwrap_or_default());
                    let r = self.fake(|f| f.do_create(&name)).map(|_| String::new());
                    self.result(tag, "CREATE ", r).await?;
                }
                "DELETE" => {
                    let name = utf7::decode(&args.first().map(Tok::text).unwrap_or_default());
                    let r = self.fake(|f| f.do_delete(&name)).map(|_| String::new());
                    self.result(tag, "DELETE ", r).await?;
                }
                "RENAME" => {
                    let from = utf7::decode(&args.first().map(Tok::text).unwrap_or_default());
                    let to = utf7::decode(&args.get(1).map(Tok::text).unwrap_or_default());
                    let r = self.fake(|f| f.do_rename(&from, &to)).map(|_| String::new());
                    self.result(tag, "RENAME ", r).await?;
                }
                "SUBSCRIBE" | "UNSUBSCRIBE" => self.write(&format!("{tag} OK {cmd} completed\r\n")).await?,
                "IDLE" => {
                    let idle = self.shared.fake.lock().unwrap().caps.idle;
                    if idle {
                        self.idle(tag).await?;
                    } else {
                        self.write(&format!("{tag} BAD IDLE not supported\r\n")).await?;
                    }
                }
                _ => self.write(&format!("{tag} BAD Unknown command\r\n")).await?,
            }
        }
    }
}

pub(crate) async fn session(io: Io, shared: Arc<Shared>, tls: bool) {
    *shared.connections.lock().unwrap() += 1;
    let mut s =
        Session { shared: shared.clone(), io: BufReader::new(io), authed: false, tls, selected: None, qresync: false };
    if s.write("* OK [CAPABILITY IMAP4rev1] Arcalo Mail test server ready\r\n").await.is_err() {
        return;
    }
    loop {
        match s.run().await {
            Ok(true) => {
                let Session { io, authed, selected, qresync, .. } = s;
                let Some(upgraded) = upgrade(io.into_inner(), &shared).await else { return };
                s = Session {
                    shared: shared.clone(),
                    io: BufReader::new(upgraded),
                    authed,
                    tls: true,
                    selected,
                    qresync,
                };
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_commands() {
        let t = tokens("a1 UID FETCH 1:* (UID FLAGS BODY.PEEK[HEADER]) (CHANGEDSINCE 5)", &[]);
        assert_eq!(t.len(), 6);
        let Tok::List(items) = &t[4] else { panic!() };
        assert_eq!(items[2].text(), "BODY.PEEK[HEADER]");
        let t = tokens(&format!("a2 APPEND \"Entw&APw-rfe\" (\\Draft) {MARK}0{MARK}"), &[b"raw".to_vec()]);
        assert_eq!(t[2].text(), "Entw&APw-rfe");
        assert!(matches!(&t[4], Tok::Str(s) if s == b"raw"));
        assert_eq!(uids_in("1:3,7:*", &[1, 2, 4, 7, 9]), [1, 2, 7, 9]);
        assert_eq!(uids_in("5:*", &[1, 2]), [2], "n:* includes the highest UID (RFC 3501)");
    }
}
