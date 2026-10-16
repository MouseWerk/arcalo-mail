//! JMAP accounts (RFC 8620 core, RFC 8621 mail, RFC 8620 7.3 push over EventSource), mapped
//! onto the same store and sync engine as IMAP.
//!
//! [`JmapClient`] speaks the protocol: the session resource (found through
//! `/.well-known/jmap`), method calls with Basic or Bearer authentication, blob download and
//! upload, the EventSource stream. [`JmapSession`] (`session.rs`) implements
//! [`crate::imap::ImapClient`] on top of it: every message in every folder gets a UID of its
//! own and every change a MODSEQ from an account-wide counter, kept in the store (schema 3),
//! so the engine's QRESYNC path (`sync::sync_mailbox`) syncs a JMAP account with
//! `Email/changes` and `Mailbox/changes`, and IDLE is the EventSource push (or polling where
//! the server has none). Sending is `Email/import` and `EmailSubmission/set`
//! ([`JmapConnector`]); the server files the message in Sent.

#[cfg(any(test, feature = "fake"))]
pub mod fake;
mod session;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::account::{Account, Auth, Transport, domain_of};
use crate::error::{Error, Result};
use crate::network::{self, HttpClient, Problem, Service};
use crate::{tr, trf};

pub use session::JmapSession;

pub const CORE: &str = "urn:ietf:params:jmap:core";
pub const MAIL: &str = "urn:ietf:params:jmap:mail";
pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";

/// How often a server without push is asked for changes while the app waits.
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Providers whose JMAP session resource is known (the setup's preset).
pub const PRESETS: &[(&str, &[&str], &str)] = &[(
    "Fastmail",
    &["fastmail.com", "fastmail.fm", "fastmail.cn", "fastmail.to", "messagingengine.com", "sent.com"],
    "https://api.fastmail.com/jmap/session",
)];

/// How a request signs in.
#[derive(Clone, PartialEq, Eq)]
pub enum JmapAuth {
    Basic { user: String, password: String },
    Bearer(String),
}

impl std::fmt::Debug for JmapAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            JmapAuth::Basic { .. } => "Basic(…)",
            JmapAuth::Bearer(_) => "Bearer(…)",
        })
    }
}

impl JmapAuth {
    /// The account's sign-in: its password (or app password) with Basic, its API token with
    /// Bearer.
    pub fn of(auth: &Auth, username: &str, secret: &str) -> JmapAuth {
        match auth {
            Auth::Token => JmapAuth::Bearer(secret.to_string()),
            _ => JmapAuth::Basic { user: username.to_string(), password: secret.to_string() },
        }
    }

    fn header(&self) -> String {
        match self {
            JmapAuth::Basic { user, password } => {
                format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}")))
            }
            JmapAuth::Bearer(t) => format!("Bearer {t}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionResource {
    api_url: String,
    download_url: String,
    upload_url: String,
    #[serde(default)]
    event_source_url: Option<String>,
    #[serde(default)]
    primary_accounts: std::collections::HashMap<String, String>,
    #[serde(default)]
    capabilities: serde_json::Map<String, Value>,
}

/// A signed-in JMAP session.
pub struct JmapClient {
    http: HttpClient,
    auth: JmapAuth,
    api_url: String,
    download_url: String,
    upload_url: String,
    event_source_url: Option<String>,
    pub account_id: String,
    /// `maxObjectsInGet` of the server.
    pub max_get: usize,
    pub can_submit: bool,
}

fn http_error(e: reqwest::Error) -> Error {
    if e.is_timeout() {
        return Problem::Timeout { host: e.url().and_then(|u| u.host_str()).unwrap_or_default().into(), port: 443 }
            .into();
    }
    Error::Network(e.to_string())
}

/// What a non-2xx answer means: a refused sign-in, or the server's trouble.
fn status_problem(status: u16, body: &str) -> Error {
    let detail = format!("HTTP {status} {}", body.chars().take(200).collect::<String>()).trim().to_string();
    match status {
        401 | 403 => Problem::Auth { detail }.into(),
        500.. => Error::Network(detail),
        _ => Problem::Protocol { detail }.into(),
    }
}

/// An `error` answer of a method call (RFC 8620, 3.6.2).
fn method_error(args: &Value) -> Error {
    let t = args.get("type").and_then(Value::as_str).unwrap_or("serverFail");
    let d = args.get("description").and_then(Value::as_str).unwrap_or_default();
    Error::Imap(format!("NO jmap {t} {d}").trim().to_string())
}

/// Whether a method failed because the server cannot say what changed since that state.
pub fn cannot_calculate_changes(e: &Error) -> bool {
    matches!(e, Error::Imap(t) if t.contains("cannotCalculateChanges"))
}

impl JmapClient {
    /// Fetches the session resource and signs in.
    pub async fn connect(session_url: &str, auth: &JmapAuth) -> Result<Self> {
        network::check_https_url(session_url)?;
        let http = network::client_for(Service::Jmap)?;
        let r = http
            .get(session_url)
            .header("Authorization", auth.header())
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(http_error)?;
        let status = r.status().as_u16();
        let text = r.text().await.map_err(http_error)?;
        if !(200..300).contains(&status) {
            return Err(status_problem(status, &text));
        }
        let s: SessionResource = serde_json::from_str(&text)
            .map_err(|e| Error::from(Problem::Protocol { detail: format!("JMAP session: {e}") }))?;
        for u in [&s.api_url, &s.download_url, &s.upload_url] {
            network::check_https_url(u)?;
        }
        let account_id = s.primary_accounts.get(MAIL).cloned().ok_or_else(|| {
            Error::from(Problem::Protocol {
                detail: tr!(
                    "Der Server bietet kein E-Mail-Konto über JMAP an",
                    "The server offers no mail account over JMAP"
                )
                .into(),
            })
        })?;
        let max_get = s
            .capabilities
            .get(CORE)
            .and_then(|c| c.get("maxObjectsInGet"))
            .and_then(Value::as_u64)
            .unwrap_or(500)
            .clamp(1, 1000) as usize;
        let event_source_url = s.event_source_url.filter(|u| network::check_https_url(u).is_ok());
        Ok(JmapClient {
            http,
            auth: auth.clone(),
            api_url: s.api_url,
            download_url: s.download_url,
            upload_url: s.upload_url,
            event_source_url,
            account_id,
            max_get,
            can_submit: s.capabilities.contains_key(SUBMISSION),
        })
    }

    pub fn has_push(&self) -> bool {
        self.event_source_url.is_some()
    }

    /// Several method calls in one request; the answers in order (an `error` answer is an
    /// [`Error::Imap`] in its place).
    pub async fn call_many(&self, calls: Vec<(&str, Value)>) -> Result<Vec<Result<Value>>> {
        let method_calls: Vec<Value> =
            calls.iter().enumerate().map(|(i, (name, args))| json!([name, args, format!("c{i}")])).collect();
        let body = json!({ "using": [CORE, MAIL, SUBMISSION], "methodCalls": method_calls });
        let r = self
            .http
            .post(&self.api_url)
            .header("Authorization", self.auth.header())
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(http_error)?;
        let status = r.status().as_u16();
        let text = r.text().await.map_err(http_error)?;
        if !(200..300).contains(&status) {
            return Err(status_problem(status, &text));
        }
        let v: Value =
            serde_json::from_str(&text).map_err(|e| Error::from(Problem::Protocol { detail: format!("JMAP: {e}") }))?;
        let responses = v.get("methodResponses").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut out = vec![];
        for (i, (name, _)) in calls.iter().enumerate() {
            let id = format!("c{i}");
            let hit = responses.iter().find(|r| r.get(2).and_then(Value::as_str) == Some(id.as_str()));
            out.push(match hit {
                Some(r) if r.get(0).and_then(Value::as_str) == Some("error") => {
                    Err(method_error(r.get(1).unwrap_or(&Value::Null)))
                }
                Some(r) => Ok(r.get(1).cloned().unwrap_or(Value::Null)),
                None => Err(Error::Imap(format!("NO jmap no answer to {name}"))),
            });
        }
        Ok(out)
    }

    /// One method call with `accountId` added.
    pub async fn call(&self, name: &str, mut args: Value) -> Result<Value> {
        if let Some(o) = args.as_object_mut() {
            o.insert("accountId".into(), Value::String(self.account_id.clone()));
        }
        self.call_many(vec![(name, args)]).await?.pop().unwrap_or_else(|| Err(Error::Imap("NO jmap".into())))
    }

    fn fill(&self, template: &str, blob: &str, name: &str, mime: &str) -> String {
        let enc = crate::oauth::form_encode;
        template
            .replace("{accountId}", &enc(&self.account_id))
            .replace("{blobId}", &enc(blob))
            .replace("{name}", &enc(name))
            .replace("{type}", &enc(mime))
    }

    pub async fn download(&self, blob: &str, name: &str, mime: &str) -> Result<Vec<u8>> {
        let url = self.fill(&self.download_url, blob, name, mime);
        network::check_https_url(&url)?;
        let r = self.http.get(&url).header("Authorization", self.auth.header()).send().await.map_err(http_error)?;
        let status = r.status().as_u16();
        let bytes = r.bytes().await.map_err(http_error)?;
        if !(200..300).contains(&status) {
            return Err(status_problem(status, &String::from_utf8_lossy(&bytes)));
        }
        Ok(bytes.to_vec())
    }

    pub async fn upload(&self, data: &[u8], mime: &str) -> Result<String> {
        let url = self.fill(&self.upload_url, "", "", mime);
        let r = self
            .http
            .post(&url)
            .header("Authorization", self.auth.header())
            .header("Content-Type", mime)
            .body(data.to_vec())
            .send()
            .await
            .map_err(http_error)?;
        let status = r.status().as_u16();
        let text = r.text().await.map_err(http_error)?;
        if !(200..300).contains(&status) {
            return Err(status_problem(status, &text));
        }
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        v.get("blobId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::from(Problem::Protocol { detail: "upload without blobId".into() }))
    }

    /// Opens the EventSource stream for Email and Mailbox changes.
    pub async fn events(&self) -> Result<reqwest::Response> {
        let url = self.event_source_url.clone().ok_or_else(|| Error::Imap("NO jmap no push".into()))?;
        let url = url.replace("{types}", "Email,Mailbox").replace("{closeafter}", "no").replace("{ping}", "60");
        network::check_https_url(&url)?;
        let http = network::client_for_events(Service::Jmap)?;
        let r = http
            .get(&url)
            .header("Authorization", self.auth.header())
            .header("Accept", "text/event-stream")
            .send()
            .await
            .map_err(http_error)?;
        let status = r.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(status_problem(status, ""));
        }
        Ok(r)
    }
}

// ---------------------------------------------------------------------------- discovery

/// The session resource of a known provider.
pub fn preset_for(email: &str) -> Option<(&'static str, &'static str)> {
    let domain = domain_of(email)?;
    PRESETS.iter().find(|(_, domains, _)| domains.contains(&domain.as_str())).map(|(name, _, url)| (*name, *url))
}

/// Looks for JMAP at an address's domain: a preset, else `https://<domain>/.well-known/jmap`
/// (RFC 8620, 2.2) answering with a session or asking to sign in. `candidates` replaces the
/// well-known address (tests: the test server).
pub async fn discover(email: &str, candidates: &[String]) -> Option<String> {
    if let Some((_, url)) = preset_for(email) {
        return Some(url.to_string());
    }
    let domain = domain_of(email)?;
    let http = network::client_for(Service::Autoconfig).ok()?;
    let defaults = [format!("https://{domain}/.well-known/jmap")];
    let list = if candidates.is_empty() { &defaults[..] } else { candidates };
    for c in list {
        let url = c.replace("{domain}", &domain);
        if network::check_https_url(&url).is_err() {
            continue;
        }
        let Ok(r) = http.get(&url).header("Accept", "application/json").send().await else { continue };
        let status = r.status().as_u16();
        let final_url = r.url().to_string();
        let ok =
            status == 401 || (status == 200 && r.text().await.is_ok_and(|t| t.contains("apiUrl") || t.contains(CORE)));
        if ok {
            return Some(final_url);
        }
    }
    None
}

/// The setup's check: signs in and counts the folders.
pub async fn check(session_url: &str, auth: &JmapAuth) -> Result<usize> {
    let c = JmapClient::connect(session_url, auth).await?;
    let v = c.call("Mailbox/get", json!({ "ids": null, "properties": ["id"] })).await?;
    Ok(v.get("list").and_then(Value::as_array).map_or(0, Vec::len))
}

// ---------------------------------------------------------------------------- the connector

/// The servers of a JMAP account, for the account worker.
pub struct JmapConnector {
    account_id: i64,
    session_url: String,
    username: String,
    auth_kind: Auth,
    secret: Mutex<String>,
    db: PathBuf,
    /// How often to ask for changes without push.
    pub poll: Duration,
}

impl JmapConnector {
    pub fn new(account: &Account, secret: String, db: PathBuf) -> Result<Self> {
        let Transport::Jmap { session_url, username, auth } = &account.transport else {
            return Err(Error::Invalid(tr!("Kein JMAP-Konto.", "Not a JMAP account.").into()));
        };
        Ok(JmapConnector {
            account_id: account.id,
            session_url: session_url.clone(),
            username: username.clone(),
            auth_kind: *auth,
            secret: Mutex::new(secret),
            db,
            poll: POLL_INTERVAL,
        })
    }

    fn auth(&self) -> JmapAuth {
        JmapAuth::of(&self.auth_kind, &self.username, &self.secret.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl crate::engine::Connector for JmapConnector {
    type Imap = JmapSession;

    async fn imap(&self) -> Result<JmapSession> {
        let client = JmapClient::connect(&self.session_url, &self.auth()).await?;
        let store = crate::store::Store::open(&self.db)?;
        Ok(JmapSession::new(client, store, self.account_id, self.poll))
    }

    async fn send(&self, from: &str, to: &[String], raw: &[u8]) -> Result<()> {
        let client = JmapClient::connect(&self.session_url, &self.auth()).await?;
        submit(&client, from, to, raw).await
    }

    fn files_sent(&self) -> bool {
        true
    }

    fn set_password(&self, password: String) {
        *self.secret.lock().unwrap_or_else(|e| e.into_inner()) = password;
    }
}

/// The ids of the folders with these roles.
async fn role_ids(client: &JmapClient) -> Result<std::collections::HashMap<String, String>> {
    let v = client.call("Mailbox/get", json!({ "ids": null, "properties": ["id", "role"] })).await?;
    Ok(v.get("list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| Some((m.get("role")?.as_str()?.to_string(), m.get("id")?.as_str()?.to_string())))
        .collect())
}

fn set_failure(v: &Value, key: &str) -> Option<String> {
    let map = v.get(key)?.as_object()?;
    let (_, e) = map.iter().next()?;
    Some(format!(
        "{} {}",
        e.get("type").and_then(Value::as_str).unwrap_or("error"),
        e.get("description").and_then(Value::as_str).unwrap_or_default()
    ))
}

/// Sends a finished message: into Drafts (`Email/import`), then `EmailSubmission/set`, which
/// moves it to Sent once it went out (RFC 8621, 7.5).
pub async fn submit(client: &JmapClient, from: &str, to: &[String], raw: &[u8]) -> Result<()> {
    if !client.can_submit {
        return Err(Error::Invalid(
            tr!("Dieses JMAP-Konto darf keine Nachrichten senden.", "This JMAP account may not send messages.").into(),
        ));
    }
    let roles = role_ids(client).await?;
    let drafts = roles.get("drafts").or_else(|| roles.get("sent")).or_else(|| roles.get("inbox")).cloned();
    let drafts = drafts.ok_or_else(|| Error::Imap("NO jmap no folder for the message".into()))?;
    let blob = client.upload(raw, "message/rfc822").await?;
    let imported = client
        .call(
            "Email/import",
            json!({ "emails": { "m": { "blobId": blob, "mailboxIds": { &drafts: true }, "keywords": { "$seen": true, "$draft": true } } } }),
        )
        .await?;
    if let Some(e) = set_failure(&imported, "notCreated") {
        return Err(Error::Invalid(trf!(
            "Der Server hat die Nachricht abgelehnt: {e}",
            "The server refused the message: {e}"
        )));
    }
    let email_id = imported
        .pointer("/created/m/id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Imap("NO jmap import without id".into()))?
        .to_string();
    let identities = client.call("Identity/get", json!({ "ids": null })).await?;
    let list = identities.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    let identity = list
        .iter()
        .find(|i| i.get("email").and_then(Value::as_str).is_some_and(|e| e.eq_ignore_ascii_case(from)))
        .or(list.first())
        .and_then(|i| i.get("id").and_then(Value::as_str))
        .ok_or_else(|| {
            Error::Invalid(
                tr!(
                    "Für diese Adresse gibt es keine Absender-Identität.",
                    "There is no sender identity for this address."
                )
                .into(),
            )
        })?
        .to_string();
    let mut on_success = serde_json::Map::new();
    on_success.insert("keywords/$draft".into(), Value::Null);
    if let Some(sent) = roles.get("sent").filter(|s| **s != drafts) {
        on_success.insert(format!("mailboxIds/{drafts}"), Value::Null);
        on_success.insert(format!("mailboxIds/{sent}"), Value::Bool(true));
    }
    let rcpt: Vec<Value> = to.iter().map(|t| json!({ "email": t.trim() })).collect();
    let r = client
        .call(
            "EmailSubmission/set",
            json!({
                "create": { "s": { "identityId": identity, "emailId": email_id,
                    "envelope": { "mailFrom": { "email": from.trim() }, "rcptTo": rcpt } } },
                "onSuccessUpdateEmail": { "#s": on_success },
            }),
        )
        .await?;
    if let Some(e) = set_failure(&r, "notCreated") {
        // The message stays in Drafts; nothing went out.
        let _ = client.call("Email/set", json!({ "destroy": [email_id] })).await;
        return Err(Error::Invalid(trf!(
            "Der Server hat die Nachricht abgelehnt: {e}",
            "The server refused the message: {e}"
        )));
    }
    Ok(())
}

/// The kinds of change an EventSource `state` event names, for the account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateChange {
    pub email: Option<String>,
    pub mailbox: Option<String>,
}

/// Reads the `state` events out of an EventSource chunk stream (RFC 8620, 7.3): `buf` keeps
/// what is not a whole event yet.
pub fn parse_events(buf: &mut String, account_id: &str) -> Vec<StateChange> {
    let mut out = vec![];
    while let Some(end) = buf.find("\n\n").or_else(|| buf.find("\r\n\r\n")) {
        let event: String = buf.drain(..end).collect();
        while buf.starts_with('\n') || buf.starts_with('\r') {
            buf.remove(0);
        }
        let mut kind = "message".to_string();
        let mut data = String::new();
        for line in event.lines() {
            if let Some(v) = line.strip_prefix("event:") {
                kind = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                data.push_str(v.trim());
            }
        }
        if kind != "state" || data.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
        let Some(changed) = v.get("changed").and_then(|c| c.get(account_id)) else { continue };
        out.push(StateChange {
            email: changed.get("Email").and_then(Value::as_str).map(str::to_string),
            mailbox: changed.get("Mailbox").and_then(Value::as_str).map(str::to_string),
        });
    }
    out
}

#[cfg(test)]
mod tests;
