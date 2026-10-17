//! Server rules in Sieve (RFC 5228 with `fileinto`, `imap4flags` RFC 5232 and `copy` RFC 3894),
//! uploaded over ManageSieve (RFC 5804). The app writes its rules as one marked block into the
//! account's active script and leaves everything else in that script alone (a vacation reply
//! made in the provider's web mail keeps working); without an active script it uploads one of
//! its own, `arcalo-mail`, and activates it.
//!
//! [`script`] makes the block from the rules, [`merge`] puts it into a script, [`Client`] speaks
//! the protocol over a stream from [`network::connect_sieve`].

use base64::Engine;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::account::{Security, Server, Transport};
use crate::error::{Error, Result};
use crate::network::{self, MailStream, Problem};
use crate::rules::{Action, Condition, Field, Op, Rule};
use crate::{tr, trf};

/// ManageSieve's port (RFC 5804).
pub const PORT: u16 = 4190;
/// The name of the script the app makes when the account has no active one.
pub const SCRIPT_NAME: &str = "arcalo-mail";
pub const BEGIN: &str = "# BEGIN Arcalo Mail";
pub const END: &str = "# END Arcalo Mail";

/// The ManageSieve server of an IMAP account: the IMAP host on port 4190 with STARTTLS (plain
/// text to this computer, as every connection).
pub fn default_server(transport: &Transport) -> Option<Server> {
    match transport {
        Transport::Imap { imap, .. } => {
            let security = if imap.security == Security::Plain { Security::Plain } else { Security::StartTls };
            Some(Server { host: imap.host.clone(), port: PORT, security, trusted_cert: imap.trusted_cert.clone() })
        }
        _ => None,
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Why a rule cannot run on a Sieve server (`None`: it can).
pub fn unsupported(rule: &Rule) -> Option<String> {
    for c in &rule.body.conditions {
        match c.field {
            Field::Age => {
                return Some(tr!("Sieve kennt kein Alter von Nachrichten.", "Sieve has no age of messages.").into());
            }
            Field::Category => {
                return Some(
                    tr!(
                        "Die Bereiche des Posteingangs gibt es nur in der App.",
                        "The inbox tabs exist in the app only."
                    )
                    .into(),
                );
            }
            _ => {}
        }
    }
    for a in &rule.body.actions {
        match a {
            Action::Pin => {
                return Some(tr!("Anheften gibt es nur in der App.", "Pinning exists in the app only.").into());
            }
            Action::Category { .. } => {
                return Some(
                    tr!(
                        "Die Bereiche des Posteingangs gibt es nur in der App.",
                        "The inbox tabs exist in the app only."
                    )
                    .into(),
                );
            }
            _ => {}
        }
    }
    None
}

fn test_of(c: &Condition) -> String {
    let v = c.value.trim();
    let header = |name: &str| match c.op {
        Op::Is => format!("header :is {} {}", quote(name), quote(v)),
        Op::StartsWith => format!(
            "header :matches {} {}",
            quote(name),
            quote(&format!("{}*", v.replace('*', "\\\\*").replace('?', "\\\\?")))
        ),
        Op::NotContains => format!("not header :contains {} {}", quote(name), quote(v)),
        _ => format!("header :contains {} {}", quote(name), quote(v)),
    };
    let address = |name: &str| match c.op {
        Op::Is => format!("address :is {} {}", quote(name), quote(v)),
        _ => header(name),
    };
    match c.field {
        Field::From => address("from"),
        Field::To => address("to"),
        Field::Cc => address("cc"),
        Field::Subject => header("subject"),
        Field::List => header("list-id"),
        Field::HasAttachment => {
            let t = "header :contains \"content-type\" \"multipart/mixed\"".to_string();
            if matches!(v, "false" | "no" | "0") { format!("not {t}") } else { t }
        }
        Field::Size => {
            format!("size {} {}K", if c.op == Op::Under { ":under" } else { ":over" }, v.parse::<u64>().unwrap_or(0))
        }
        Field::Age | Field::Category => "false".into(),
    }
}

/// The extensions a block needs and the block itself: one `if` per enabled server rule of the
/// account, in order. `trash`, `archive`: the account's folders for „delete“ and „skip the
/// inbox“.
pub fn script(rules: &[Rule], trash: &str, archive: &str) -> (Vec<&'static str>, String) {
    let mut requires: Vec<&'static str> = vec![];
    let mut need = |x: &'static str| {
        if !requires.contains(&x) {
            requires.push(x);
        }
    };
    let mut out =
        vec![BEGIN.to_string(), "# Rules made in Arcalo Mail; this block is written again when they change.".into()];
    for r in rules.iter().filter(|r| r.enabled && unsupported(r).is_none()) {
        let tests: Vec<String> = r.body.conditions.iter().map(test_of).collect();
        let cond = if tests.len() == 1 {
            tests[0].clone()
        } else {
            format!("{} ({})", if r.body.match_all { "allof" } else { "anyof" }, tests.join(", "))
        };
        out.push(format!("# {}", r.name.replace(['\r', '\n'], " ")));
        out.push(format!("if {cond} {{"));
        let mut leave: Option<String> = None;
        for a in &r.body.actions {
            match a {
                Action::Label { keyword } => {
                    need("imap4flags");
                    out.push(format!("    addflag {};", quote(keyword.trim())));
                }
                Action::MarkRead => {
                    need("imap4flags");
                    out.push("    addflag \"\\\\Seen\";".into());
                }
                Action::Flag => {
                    need("imap4flags");
                    out.push("    addflag \"\\\\Flagged\";".into());
                }
                Action::Forward { to } => {
                    need("copy");
                    out.push(format!("    redirect :copy {};", quote(to.trim())));
                }
                Action::Move { folder } => leave = Some(folder.trim().to_string()),
                Action::Delete => leave = Some(trash.to_string()),
                Action::SkipInbox => leave = Some(archive.to_string()),
                Action::Pin | Action::Category { .. } => {}
            }
        }
        if let Some(f) = leave {
            need("fileinto");
            out.push(format!("    fileinto {};", quote(&f)));
        }
        if r.body.stop || out.last().is_some_and(|l| l.contains("fileinto")) {
            out.push("    stop;".into());
        }
        out.push("}".into());
    }
    out.push(END.into());
    requires.sort();
    (requires, out.join("\n") + "\n")
}

/// The extensions of a `require` statement (`require "x";`, `require ["x", "y"];`).
fn require_list(stmt: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = stmt;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else { break };
        out.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

/// Puts the block into a script: the old block goes, the `require`s are merged and come first,
/// everything else stays as it was.
pub fn merge(existing: Option<&str>, requires: &[&str], block: &str) -> String {
    let existing = existing.unwrap_or_default().replace("\r\n", "\n");
    // Without the old block.
    let mut kept: Vec<&str> = vec![];
    let mut inside = false;
    for line in existing.lines() {
        if line.trim() == BEGIN {
            inside = true;
            continue;
        }
        if inside {
            if line.trim() == END {
                inside = false;
            }
            continue;
        }
        kept.push(line);
    }
    // The leading requires (and the comments among them).
    let mut all: Vec<String> = requires.iter().map(|s| s.to_string()).collect();
    let mut head_comments: Vec<&str> = vec![];
    let mut i = 0;
    let mut stmt = String::new();
    while i < kept.len() {
        let t = kept[i].trim();
        if !stmt.is_empty() {
            stmt.push_str(t);
            if t.contains(';') {
                all.extend(require_list(&stmt));
                stmt.clear();
            }
        } else if t.is_empty() || t.starts_with('#') {
            if !t.is_empty() {
                head_comments.push(kept[i]);
            }
        } else if t.starts_with("require") {
            if t.contains(';') {
                all.extend(require_list(t));
            } else {
                stmt.push_str(t);
            }
        } else {
            break;
        }
        i += 1;
    }
    all.sort();
    all.dedup();
    let mut out = String::new();
    if !all.is_empty() {
        out.push_str(&format!("require [{}];\n", all.iter().map(|r| quote(r)).collect::<Vec<_>>().join(", ")));
    }
    for c in head_comments {
        out.push_str(c);
        out.push('\n');
    }
    out.push_str(block);
    let rest = kept[i..].join("\n");
    if !rest.trim().is_empty() {
        out.push('\n');
        out.push_str(rest.trim_end());
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------- the protocol

/// A response: `OK`, `NO` or `BYE` with its text, and the lines before it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Response {
    ok: bool,
    text: String,
    lines: Vec<String>,
}

/// What the server offers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub implementation: String,
    pub sasl: Vec<String>,
    pub sieve: Vec<String>,
}

/// A signed-in ManageSieve connection.
pub struct Client {
    io: BufReader<MailStream>,
    pub caps: Capabilities,
}

fn protocol(detail: impl Into<String>) -> Error {
    Problem::Protocol { detail: detail.into() }.into()
}

impl Client {
    /// Connects, reads the capabilities and signs in with PLAIN.
    pub async fn connect(server: &Server, user: &str, password: &str) -> Result<Client> {
        let stream = network::connect_sieve(server).await?;
        let mut c = Client { io: BufReader::new(stream), caps: Capabilities::default() };
        let greeting = c.response().await?;
        c.caps = parse_caps(&greeting.lines);
        if !greeting.ok {
            return Err(protocol(greeting.text));
        }
        if !c.caps.sasl.iter().any(|m| m.eq_ignore_ascii_case("PLAIN")) {
            return Err(protocol(tr!(
                "der Server bietet keine Anmeldung mit PLAIN an",
                "the server offers no PLAIN sign-in"
            )));
        }
        let token = base64::engine::general_purpose::STANDARD.encode(format!("\0{user}\0{password}"));
        let r = c.command(&format!("AUTHENTICATE \"PLAIN\" {}", quote(&token))).await?;
        if !r.ok {
            return Err(Problem::Auth { detail: r.text }.into());
        }
        Ok(c)
    }

    async fn line(&mut self) -> Result<String> {
        let mut s = String::new();
        let n = tokio::time::timeout(network::IO_TIMEOUT, self.io.read_line(&mut s))
            .await
            .map_err(|_| Error::from(Problem::Timeout { host: String::new(), port: PORT }))?
            .map_err(|e| Error::from(network::io_problem(&e)))?;
        if n == 0 {
            return Err(protocol(tr!("Verbindung geschlossen", "connection closed")));
        }
        Ok(s.trim_end_matches(['\r', '\n']).to_string())
    }

    /// Lines up to `OK`/`NO`/`BYE`; a literal (`{n}`) is read whole into its line.
    async fn response(&mut self) -> Result<Response> {
        let mut lines = vec![];
        loop {
            let mut l = self.line().await?;
            if let Some(n) = literal_len(&l) {
                let mut buf = vec![0u8; n];
                tokio::time::timeout(network::IO_TIMEOUT, self.io.read_exact(&mut buf))
                    .await
                    .map_err(|_| Error::from(Problem::Timeout { host: String::new(), port: PORT }))?
                    .map_err(|e| Error::from(network::io_problem(&e)))?;
                l = String::from_utf8_lossy(&buf).into_owned();
                // The CRLF after the literal.
                let _ = self.line().await?;
                lines.push(l);
                continue;
            }
            let upper = l.to_ascii_uppercase();
            for (word, ok) in [("OK", true), ("NO", false), ("BYE", false)] {
                if upper == word || upper.starts_with(&format!("{word} ")) {
                    let text = l[word.len()..].trim().trim_matches('"').to_string();
                    return Ok(Response { ok, text, lines });
                }
            }
            lines.push(l);
        }
    }

    async fn command(&mut self, cmd: &str) -> Result<Response> {
        self.send(format!("{cmd}\r\n").as_bytes()).await?;
        self.response().await
    }

    async fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let s = self.io.get_mut();
        s.write_all(bytes).await.map_err(|e| Error::from(network::io_problem(&e)))?;
        s.flush().await.map_err(|e| Error::from(network::io_problem(&e)))
    }

    /// The scripts and which one is active.
    pub async fn list(&mut self) -> Result<Vec<(String, bool)>> {
        let r = self.command("LISTSCRIPTS").await?;
        if !r.ok {
            return Err(protocol(r.text));
        }
        Ok(r.lines
            .iter()
            .filter_map(|l| {
                let name = l.strip_prefix('"')?.split('"').next()?.to_string();
                Some((name, l.to_ascii_uppercase().ends_with("ACTIVE")))
            })
            .collect())
    }

    pub async fn get(&mut self, name: &str) -> Result<String> {
        let r = self.command(&format!("GETSCRIPT {}", quote(name))).await?;
        if !r.ok {
            return Err(protocol(r.text));
        }
        Ok(r.lines.join("\n"))
    }

    /// Uploads a script; the server checks it first and says what is wrong.
    pub async fn put(&mut self, name: &str, script: &str) -> Result<()> {
        let data = script.replace("\r\n", "\n").replace('\n', "\r\n");
        self.send(format!("PUTSCRIPT {} {{{}+}}\r\n", quote(name), data.len()).as_bytes()).await?;
        self.send(data.as_bytes()).await?;
        let r = self.command("").await?;
        if !r.ok {
            return Err(Error::Invalid(trf!(
                "Der Server hat die Regeln nicht angenommen: {}",
                "The server did not take the rules: {}",
                r.text
            )));
        }
        Ok(())
    }

    pub async fn activate(&mut self, name: &str) -> Result<()> {
        let r = self.command(&format!("SETACTIVE {}", quote(name))).await?;
        if !r.ok {
            return Err(protocol(r.text));
        }
        Ok(())
    }

    pub async fn logout(mut self) {
        let _ = self.command("LOGOUT").await;
    }
}

fn literal_len(line: &str) -> Option<usize> {
    let inner = line.strip_suffix('}')?.rsplit_once('{')?.1;
    if line.trim_start().starts_with('{') { inner.trim_end_matches('+').parse().ok() } else { None }
}

fn parse_caps(lines: &[String]) -> Capabilities {
    let mut c = Capabilities::default();
    for l in lines {
        let parts: Vec<&str> = l.split('"').collect();
        let (Some(key), value) = (parts.get(1), parts.get(3).copied().unwrap_or_default()) else { continue };
        match key.to_ascii_uppercase().as_str() {
            "IMPLEMENTATION" => c.implementation = value.to_string(),
            "SASL" => c.sasl = value.split_whitespace().map(str::to_owned).collect(),
            "SIEVE" => c.sieve = value.split_whitespace().map(|s| s.to_ascii_lowercase()).collect(),
            _ => {}
        }
    }
    c
}

/// Signs in and says which Sieve extensions the server has (the editor's „Serverregeln prüfen“).
pub async fn probe(server: &Server, user: &str, password: &str) -> Result<Capabilities> {
    let c = Client::connect(server, user, password).await?;
    let caps = c.caps.clone();
    c.logout().await;
    Ok(caps)
}

/// Writes the rules into the account's active script (or a script of their own). Returns the
/// script as uploaded.
pub async fn upload(server: &Server, user: &str, password: &str, requires: &[&str], block: &str) -> Result<String> {
    let mut c = Client::connect(server, user, password).await?;
    if let Some(missing) = requires.iter().find(|r| !c.caps.sieve.iter().any(|x| x == *r)) {
        c.logout().await;
        return Err(Error::Invalid(trf!(
            "Der Server kann diese Regel nicht ausführen (ihm fehlt die Sieve-Erweiterung „{missing}“).",
            "The server cannot run this rule (it lacks the Sieve extension “{missing}”)."
        )));
    }
    let scripts = c.list().await?;
    let active = scripts.iter().find(|(_, a)| *a).map(|(n, _)| n.clone());
    let (name, existing) = match &active {
        Some(n) => (n.clone(), Some(c.get(n).await?)),
        None => (SCRIPT_NAME.to_string(), None),
    };
    let merged = merge(existing.as_deref(), requires, block);
    c.put(&name, &merged).await?;
    if active.is_none() {
        c.activate(&name).await?;
    }
    c.logout().await;
    Ok(merged)
}

#[cfg(any(test, feature = "fake"))]
pub mod fake {
    //! A ManageSieve server's state machine, without sockets: the core's tests and the test
    //! server (`crates/mail-testserver`) put it on a port.

    use std::collections::BTreeMap;

    use base64::Engine;

    #[derive(Debug, Clone)]
    pub struct FakeSieve {
        pub user: String,
        pub password: String,
        pub scripts: BTreeMap<String, String>,
        pub active: Option<String>,
        pub extensions: Vec<String>,
        pub authed: bool,
    }

    impl FakeSieve {
        pub fn new(user: &str, password: &str) -> Self {
            FakeSieve {
                user: user.into(),
                password: password.into(),
                scripts: BTreeMap::new(),
                active: None,
                extensions: ["fileinto", "imap4flags", "copy", "vacation", "envelope"].map(String::from).to_vec(),
                authed: false,
            }
        }

        pub fn greeting(&self) -> String {
            format!(
                "\"IMPLEMENTATION\" \"Arcalo Mail test server\"\r\n\"SASL\" \"PLAIN\"\r\n\"SIEVE\" \"{}\"\r\n\"VERSION\" \"1.0\"\r\nOK \"ready\"\r\n",
                self.extensions.join(" ")
            )
        }

        fn args(line: &str) -> Vec<String> {
            line.split('"').skip(1).step_by(2).map(str::to_owned).collect()
        }

        /// One command (with the literal that came with it); the answer to write back, and
        /// whether the connection ends.
        pub fn handle(&mut self, line: &str, literal: Option<&str>) -> (String, bool) {
            let cmd = line.split_whitespace().next().unwrap_or_default().to_ascii_uppercase();
            let args = Self::args(line);
            if cmd == "LOGOUT" {
                return ("OK \"bye\"\r\n".into(), true);
            }
            if cmd == "AUTHENTICATE" {
                let token = args.get(1).cloned().unwrap_or_default();
                let decoded = base64::engine::general_purpose::STANDARD.decode(token).unwrap_or_default();
                let parts: Vec<String> = String::from_utf8_lossy(&decoded).split('\0').map(str::to_owned).collect();
                if parts.len() == 3 && parts[1].eq_ignore_ascii_case(&self.user) && parts[2] == self.password {
                    self.authed = true;
                    return ("OK \"signed in\"\r\n".into(), false);
                }
                return ("NO \"wrong password\"\r\n".into(), false);
            }
            if !self.authed {
                return ("NO \"sign in first\"\r\n".into(), false);
            }
            match cmd.as_str() {
                "LISTSCRIPTS" => {
                    let mut out = String::new();
                    for name in self.scripts.keys() {
                        let active = if self.active.as_deref() == Some(name) { " ACTIVE" } else { "" };
                        out.push_str(&format!("\"{name}\"{active}\r\n"));
                    }
                    out.push_str("OK\r\n");
                    (out, false)
                }
                "GETSCRIPT" => match args.first().and_then(|n| self.scripts.get(n)) {
                    Some(s) => (format!("{{{}}}\r\n{s}\r\nOK\r\n", s.len()), false),
                    None => ("NO \"no such script\"\r\n".into(), false),
                },
                "PUTSCRIPT" => {
                    let script = literal.unwrap_or_default().to_string();
                    if script.matches('{').count() != script.matches('}').count() {
                        return ("NO \"syntax error: unbalanced braces\"\r\n".into(), false);
                    }
                    if let Some(missing) = Self::required(&script).into_iter().find(|r| !self.extensions.contains(r)) {
                        return (format!("NO \"unsupported extension {missing}\"\r\n"), false);
                    }
                    self.scripts.insert(args.first().cloned().unwrap_or_default(), script.replace("\r\n", "\n"));
                    ("OK\r\n".into(), false)
                }
                "SETACTIVE" => {
                    let name = args.first().cloned().unwrap_or_default();
                    if name.is_empty() || self.scripts.contains_key(&name) {
                        self.active = (!name.is_empty()).then_some(name);
                        ("OK\r\n".into(), false)
                    } else {
                        ("NO \"no such script\"\r\n".into(), false)
                    }
                }
                _ => ("NO \"unknown command\"\r\n".into(), false),
            }
        }

        fn required(script: &str) -> Vec<String> {
            script.lines().filter(|l| l.trim_start().starts_with("require")).flat_map(Self::args).collect()
        }

        /// Whether a command line announces a literal (`{12+}`): its length.
        pub fn literal(line: &str) -> Option<usize> {
            let inner = line.trim_end().strip_suffix("+}")?.rsplit_once('{')?.1;
            inner.parse().ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    use super::fake::FakeSieve;
    use super::*;
    use crate::rules::RuleBody;
    use crate::rules::tests::cond;

    fn rule(name: &str, conditions: Vec<Condition>, actions: Vec<Action>) -> Rule {
        Rule {
            id: 1,
            account_id: Some(1),
            name: name.into(),
            place: crate::rules::Place::Server,
            enabled: true,
            position: 0,
            body: RuleBody { match_all: true, conditions, actions, stop: false },
            server_ref: None,
        }
    }

    async fn serve(fake: Arc<Mutex<FakeSieve>>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else { return };
                let fake = fake.clone();
                tokio::spawn(async move {
                    let mut io = BufReader::new(s);
                    let greeting = fake.lock().unwrap().greeting();
                    let _ = io.get_mut().write_all(greeting.as_bytes()).await;
                    loop {
                        let mut line = String::new();
                        if io.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let mut literal = None;
                        if let Some(n) = FakeSieve::literal(&line) {
                            let mut buf = vec![0u8; n];
                            io.read_exact(&mut buf).await.unwrap();
                            let mut end = String::new();
                            io.read_line(&mut end).await.unwrap();
                            literal = Some(String::from_utf8(buf).unwrap());
                        }
                        let (answer, bye) = fake.lock().unwrap().handle(line.trim_end(), literal.as_deref());
                        let _ = io.get_mut().write_all(answer.as_bytes()).await;
                        if bye {
                            return;
                        }
                    }
                });
            }
        });
        port
    }

    #[test]
    fn scripts_from_rules() {
        let rules = vec![
            rule(
                "Rechnungen",
                vec![cond(Field::Subject, Op::Contains, "Rechnung"), cond(Field::From, Op::Is, "billing@shop.example")],
                vec![
                    Action::Label { keyword: "$Rechnung".into() },
                    Action::MarkRead,
                    Action::Move { folder: "Rechnungen".into() },
                ],
            ),
            rule(
                "Groß",
                vec![cond(Field::Size, Op::Over, "5000")],
                vec![Action::Forward { to: "archiv@example.org".into() }],
            ),
            rule("Alt", vec![cond(Field::Age, Op::Over, "30")], vec![Action::Delete]),
            Rule { enabled: false, ..rule("Aus", vec![cond(Field::Subject, Op::Contains, "x")], vec![Action::Delete]) },
        ];
        assert!(unsupported(&rules[2]).is_some());
        let (requires, block) = script(&rules, "Trash", "Archive");
        assert_eq!(requires, ["copy", "fileinto", "imap4flags"]);
        assert!(block.starts_with(BEGIN) && block.trim_end().ends_with(END));
        assert!(block.contains(
            "if allof (header :contains \"subject\" \"Rechnung\", address :is \"from\" \"billing@shop.example\") {\n    addflag \"$Rechnung\";\n    addflag \"\\\\Seen\";\n    fileinto \"Rechnungen\";\n    stop;\n}"
        ), "{block}");
        assert!(block.contains("if size :over 5000K {\n    redirect :copy \"archiv@example.org\";\n}"), "{block}");
        assert!(!block.contains("# Alt") && !block.contains("# Aus"));
    }

    #[test]
    fn merging_keeps_the_rest_of_the_script() {
        let theirs = "require [\"vacation\"];\n# made in web mail\nvacation :days 7 \"Bin im Urlaub\";\n";
        let (requires, block) =
            script(&[rule("R", vec![cond(Field::Subject, Op::Contains, "x")], vec![Action::Flag])], "Trash", "Archive");
        let once = merge(Some(theirs), &requires, &block);
        assert!(
            once.starts_with("require [\"imap4flags\", \"vacation\"];\n# made in web mail\n# BEGIN Arcalo Mail"),
            "{once}"
        );
        assert!(once.contains("vacation :days 7"));
        // Again with other rules: the old block goes, nothing doubles.
        let (r2, b2) = script(
            &[rule("S", vec![cond(Field::Subject, Op::Contains, "y")], vec![Action::SkipInbox])],
            "Trash",
            "Archive",
        );
        let twice = merge(Some(&once), &r2, &b2);
        assert_eq!(twice.matches(BEGIN).count(), 1);
        assert!(!twice.contains("\"x\"") && twice.contains("fileinto \"Archive\""), "{twice}");
        assert!(twice.starts_with("require [\"fileinto\", \"imap4flags\", \"vacation\"];"), "{twice}");
        assert_eq!(
            merge(None, &[], "# BEGIN Arcalo Mail\n# END Arcalo Mail\n"),
            "# BEGIN Arcalo Mail\n# END Arcalo Mail\n"
        );
    }

    #[tokio::test]
    async fn uploads_into_the_active_script_or_one_of_its_own() {
        let fake = Arc::new(Mutex::new(FakeSieve::new("maria@example.org", "geheim")));
        let port = serve(fake.clone()).await;
        let server = Server::new("127.0.0.1", port, Security::Plain);
        let caps = probe(&server, "maria@example.org", "geheim").await.unwrap();
        assert!(caps.sieve.contains(&"fileinto".to_string()));
        assert!(matches!(
            probe(&server, "maria@example.org", "falsch").await,
            Err(Error::Connection(Problem::Auth { .. }))
        ));
        let (requires, block) = script(
            &[rule("R", vec![cond(Field::From, Op::Contains, "shop")], vec![Action::Move { folder: "Shop".into() }])],
            "Trash",
            "Archive",
        );
        upload(&server, "maria@example.org", "geheim", &requires, &block).await.unwrap();
        {
            let f = fake.lock().unwrap();
            assert_eq!(f.active.as_deref(), Some(SCRIPT_NAME));
            assert!(f.scripts[SCRIPT_NAME].contains("fileinto \"Shop\""));
        }
        // A script of the web mail is active: the block goes into it.
        {
            let mut f = fake.lock().unwrap();
            f.scripts.insert("roundcube".into(), "require \"vacation\";\nvacation \"weg\";\n".into());
            f.active = Some("roundcube".into());
        }
        upload(&server, "maria@example.org", "geheim", &requires, &block).await.unwrap();
        {
            let f = fake.lock().unwrap();
            assert_eq!(f.active.as_deref(), Some("roundcube"));
            let s = &f.scripts["roundcube"];
            assert!(s.contains("vacation \"weg\";") && s.contains("fileinto \"Shop\""), "{s}");
        }
        // An extension the server lacks.
        fake.lock().unwrap().extensions.retain(|e| e != "fileinto");
        let e = upload(&server, "maria@example.org", "geheim", &requires, &block).await.unwrap_err().to_string();
        assert!(e.contains("“fileinto”"), "{e}");
    }

    #[test]
    fn servers_and_literals() {
        let t = crate::account::preset(crate::account::Provider::Imap, "a@mailbox.org").unwrap();
        let s = default_server(&t).unwrap();
        assert_eq!((s.host.as_str(), s.port, s.security), ("imap.mailbox.org", PORT, Security::StartTls));
        assert_eq!(literal_len("{42}"), Some(42));
        assert_eq!(literal_len("\"x\" {42}"), None);
        assert_eq!(FakeSieve::literal("PUTSCRIPT \"x\" {12+}\r\n"), Some(12));
    }
}
