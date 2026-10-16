//! OAuth 2.0 sign-in as a public client (no client secret in the app): the authorization code
//! flow with PKCE (RFC 7636) in the system browser and a loopback redirect on the desktop
//! (RFC 8252), the token exchange, and the refresh of access tokens ([`TokenSource`]).
//!
//! Microsoft 365 and Outlook.com are the one provider of 1.0.0 ([`microsoft`]); IMAP and SMTP
//! then sign in with XOAUTH2. The client id is compiled in from `ARCALO_MAIL_MS_CLIENT_ID`
//! (docs/microsoft.md); a build without it has no Microsoft sign-in, and the setup offers IMAP
//! with an app password instead.
//!
//! The refresh token goes to the OS credential store (the shell, under
//! `account::secret_key(id, RefreshToken)`), the access token stays in memory. On the phones
//! (milestone 3) the redirect is the platform's custom scheme [`MOBILE_REDIRECT`] through its
//! browser session; the flow here is the same apart from where the code arrives.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::account::OAuthProvider;
use crate::error::{Error, Result};
use crate::network::{self, Problem, Service};
use crate::{tr, trf};

/// Microsoft's endpoints for work, school and personal accounts („common“ tenant).
pub const MS_AUTHORITY: &str = "https://login.microsoftonline.com/common/oauth2/v2.0";

/// What Arcalo Mail asks Microsoft for: IMAP and SMTP for the signed-in user, a refresh token,
/// and the address of the account.
pub const MS_SCOPES: &[&str] = &[
    "offline_access",
    "https://outlook.office.com/IMAP.AccessAsUser.All",
    "https://outlook.office.com/SMTP.Send",
    "openid",
    "email",
    "profile",
];

/// The redirect of the Android and iOS apps (registered in the Entra app as a mobile redirect
/// URI; used from milestone 3 on, docs/microsoft.md).
pub const MOBILE_REDIRECT: &str = "de.mousewerk.arcalo.mail://oauth";

/// How long the browser sign-in may take.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Refresh the access token this long before it runs out.
pub const REFRESH_MARGIN: i64 = 5 * 60;

/// A provider's endpoints and the app's registration there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    pub provider: OAuthProvider,
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
}

/// The client id compiled into this build (`ARCALO_MAIL_MS_CLIENT_ID` at build time).
pub fn microsoft_client_id() -> Option<String> {
    let built = option_env!("ARCALO_MAIL_MS_CLIENT_ID").map(str::trim).filter(|s| !s.is_empty());
    if let Some(id) = built {
        return Some(id.to_string());
    }
    // The e2e tests run the app against the fake identity provider of the test server; only
    // then a client id from the environment counts.
    test_authority().and_then(|_| std::env::var("ARCALO_MAIL_MS_CLIENT_ID").ok()).filter(|s| !s.trim().is_empty())
}

/// `ARCALO_MAIL_MS_AUTHORITY`: the e2e tests' identity provider, accepted on this computer only.
fn test_authority() -> Option<String> {
    let a = std::env::var("ARCALO_MAIL_MS_AUTHORITY").ok()?;
    let a = a.trim().trim_end_matches('/').to_string();
    let host = a.strip_prefix("http://")?.split(['/', ':']).next()?.to_string();
    network::is_loopback(&host).then_some(a)
}

/// The e2e tests' IMAP and SMTP servers for a Microsoft account (`ARCALO_MAIL_MS_IMAP`,
/// `ARCALO_MAIL_MS_SMTP` as `127.0.0.1:port`), only together with the test identity provider
/// and only on this computer. `None` in normal use: Microsoft's own servers.
pub fn test_servers() -> Option<(crate::account::Server, crate::account::Server)> {
    test_authority()?;
    let server = |var: &str| {
        let v = std::env::var(var).ok()?;
        let (host, port) = v.rsplit_once(':')?;
        let port: u16 = port.parse().ok()?;
        network::is_loopback(host).then(|| crate::account::Server::new(host, port, crate::account::Security::Plain))
    };
    Some((server("ARCALO_MAIL_MS_IMAP")?, server("ARCALO_MAIL_MS_SMTP")?))
}

/// Microsoft's configuration; `None` when this build has no client id.
pub fn microsoft() -> Option<OAuthConfig> {
    let client_id = microsoft_client_id()?;
    let authority = test_authority().unwrap_or_else(|| MS_AUTHORITY.to_string());
    Some(OAuthConfig {
        provider: OAuthProvider::Microsoft,
        client_id,
        authorize_url: format!("{authority}/authorize"),
        token_url: format!("{authority}/token"),
        scopes: MS_SCOPES.iter().map(|s| s.to_string()).collect(),
    })
}

/// The configuration of a provider, or why there is none.
pub fn config_for(provider: OAuthProvider) -> Result<OAuthConfig> {
    match provider {
        OAuthProvider::Microsoft => microsoft().ok_or_else(|| {
            Error::Invalid(
                tr!(
                    "Diese Ausgabe von Arcalo Mail hat keine Microsoft-Anmeldung.",
                    "This build of Arcalo Mail has no Microsoft sign-in."
                )
                .into(),
            )
        }),
        OAuthProvider::Google => Err(Error::Invalid(
            tr!(
                "Die Google-Anmeldung kommt in einer späteren Version. Nutze bis dahin IMAP mit einem App-Passwort.",
                "Google sign-in comes in a later version. Until then, use IMAP with an app password."
            )
            .into(),
        )),
    }
}

fn service(provider: OAuthProvider) -> Service {
    match provider {
        OAuthProvider::Microsoft => Service::OAuthMicrosoft,
        OAuthProvider::Google => Service::OAuthGoogle,
    }
}

// ---------------------------------------------------------------------------- PKCE and URLs

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Random bytes from the TLS library's generator.
fn random(n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    rustls::crypto::aws_lc_rs::default_provider()
        .secure_random
        .fill(&mut buf)
        .map_err(|e| Error::Network(format!("random: {e:?}")))?;
    Ok(buf)
}

/// A PKCE pair (RFC 7636, S256).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn challenge_of(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

pub fn pkce() -> Result<Pkce> {
    let verifier = b64url(&random(32)?);
    Ok(Pkce { challenge: challenge_of(&verifier), verifier })
}

/// `application/x-www-form-urlencoded` for one value.
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v))).collect::<Vec<_>>().join("&")
}

/// The pairs of a query string or form body.
pub fn form_decode(s: &str) -> Vec<(String, String)> {
    let dec = |x: &str| {
        let b = x.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < b.len() => {
                    match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                        Some(v) => {
                            out.push(v);
                            i += 2;
                        }
                        None => out.push(b'%'),
                    }
                }
                c => out.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    s.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (dec(k), dec(v))
        })
        .collect()
}

fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// The address the browser opens.
pub fn authorize_url(cfg: &OAuthConfig, redirect_uri: &str, pkce: &Pkce, state: &str, login_hint: &str) -> String {
    let scope = cfg.scopes.join(" ");
    let mut pairs = vec![
        ("client_id", cfg.client_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri),
        ("response_mode", "query"),
        ("scope", scope.as_str()),
        ("state", state),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("prompt", "select_account"),
    ];
    if !login_hint.trim().is_empty() {
        pairs.push(("login_hint", login_hint.trim()));
    }
    format!("{}?{}", cfg.authorize_url, form(&pairs))
}

// ---------------------------------------------------------------------------- tokens

/// What the token endpoint gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    /// A new refresh token (Microsoft rotates them); `None`: keep the old one.
    pub refresh_token: Option<String>,
    /// Unix seconds.
    pub expires_at: i64,
    /// The account's address from the ID token (`email` or `preferred_username`).
    pub email: Option<String>,
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    id_token: Option<String>,
}

#[derive(Deserialize, Default)]
struct ErrorAnswer {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
    #[serde(default)]
    error_codes: Vec<i64>,
}

fn now() -> i64 {
    crate::store::now()
}

/// The address claims of an ID token (not verified: it only names the account, the IMAP server
/// decides about access).
fn email_of(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    ["email", "preferred_username", "upn"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()).filter(|s| s.contains('@')).map(str::to_string))
}

/// An error answer of the identity provider as a [`Problem`] the UI explains.
pub fn token_problem(status: u16, body: &str) -> Error {
    let e: ErrorAnswer = serde_json::from_str(body).unwrap_or_default();
    let detail =
        if e.error_description.is_empty() { format!("{status} {}", e.error) } else { e.error_description.clone() };
    let first_line = detail.lines().next().unwrap_or_default().trim().to_string();
    let admin = e.error_codes.iter().any(|c| matches!(c, 65001 | 90094 | 90095))
        || ["AADSTS65001", "AADSTS90094", "AADSTS90095"].iter().any(|c| detail.contains(c));
    if admin {
        return Problem::AdminConsent { detail: first_line }.into();
    }
    match e.error.as_str() {
        // The refresh token was revoked, expired or the password changed: sign in again.
        "invalid_grant" | "interaction_required" | "consent_required" | "login_required" => {
            Problem::Auth { detail: first_line }.into()
        }
        _ if status >= 500 => Error::Network(first_line),
        _ => Problem::Protocol { detail: first_line }.into(),
    }
}

async fn token_request(cfg: &OAuthConfig, body: String) -> Result<Tokens> {
    network::check_https_url(&cfg.token_url)?;
    let client = network::client_for(service(cfg.provider))?;
    let r = client
        .post(&cfg.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = r.status().as_u16();
    let text = r.text().await.map_err(|e| Error::Network(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(token_problem(status, &text));
    }
    let t: TokenAnswer =
        serde_json::from_str(&text).map_err(|e| Error::from(Problem::Protocol { detail: format!("token: {e}") }))?;
    Ok(Tokens {
        access_token: t.access_token,
        refresh_token: t.refresh_token.filter(|r| !r.is_empty()),
        expires_at: now() + t.expires_in.unwrap_or(3600).max(60),
        email: t.id_token.as_deref().and_then(email_of),
    })
}

/// Exchanges the code from the redirect for tokens.
pub async fn exchange_code(cfg: &OAuthConfig, code: &str, verifier: &str, redirect_uri: &str) -> Result<Tokens> {
    let scope = cfg.scopes.join(" ");
    let body = form(&[
        ("client_id", &cfg.client_id),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
        ("scope", &scope),
    ]);
    token_request(cfg, body).await
}

/// A new access token for the refresh token.
pub async fn refresh(cfg: &OAuthConfig, refresh_token: &str) -> Result<Tokens> {
    let scope = cfg.scopes.join(" ");
    let body = form(&[
        ("client_id", &cfg.client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("scope", &scope),
    ]);
    token_request(cfg, body).await
}

/// The page the browser shows after the redirect.
fn done_page(ok: bool) -> String {
    let (title, text) = if ok {
        (
            tr!("Angemeldet", "Signed in"),
            tr!(
                "Du kannst dieses Fenster schließen und zu Arcalo Mail zurückkehren.",
                "You can close this window and return to Arcalo Mail."
            ),
        )
    } else {
        (
            tr!("Anmeldung nicht abgeschlossen", "Sign-in not finished"),
            tr!(
                "Kehre zu Arcalo Mail zurück, dort steht, was fehlt.",
                "Return to Arcalo Mail, it says what is missing."
            ),
        )
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Arcalo Mail</title></head>\
         <body style=\"font-family:system-ui,sans-serif;margin:15vh auto;max-width:28rem;text-align:center;color:#2f2f3a\">\
         <h1 style=\"font-size:1.4rem;font-weight:600\">{title}</h1><p>{text}</p></body></html>"
    )
}

/// The redirect's answer as tokens, or the provider's refusal as a [`Problem`].
async fn finish(cfg: &OAuthConfig, query: &str, state: &str, pkce: &Pkce, redirect_uri: &str) -> Result<Tokens> {
    let q = form_decode(query);
    if get(&q, "state") != Some(state) {
        return Err(Problem::Protocol {
            detail: tr!("Antwort passt nicht zur Anmeldung", "answer does not match the sign-in").into(),
        }
        .into());
    }
    if let Some(err) = get(&q, "error") {
        let desc = get(&q, "error_description").unwrap_or_default();
        if ["AADSTS65001", "AADSTS90094", "AADSTS90095"].iter().any(|c| desc.contains(c)) {
            return Err(Problem::AdminConsent { detail: desc.lines().next().unwrap_or_default().to_string() }.into());
        }
        if err == "access_denied" {
            return Err(cancelled());
        }
        return Err(Problem::Auth { detail: format!("{err}: {desc}") }.into());
    }
    let code = get(&q, "code").ok_or_else(|| Error::from(Problem::Protocol { detail: "no code".into() }))?;
    exchange_code(cfg, code, &pkce.verifier, redirect_uri).await
}

/// Signs in on the desktop: opens the loopback listener, hands the provider's address to
/// `open` (the system browser), waits for the redirect and exchanges the code.
pub async fn sign_in(cfg: &OAuthConfig, login_hint: &str, open: impl FnOnce(&str) -> Result<()>) -> Result<Tokens> {
    let listener = network::redirect_listener().await?;
    let redirect_uri = listener.redirect_uri();
    let pkce = pkce()?;
    let state = b64url(&random(16)?);
    open(&authorize_url(cfg, &redirect_uri, &pkce, &state, login_hint))?;
    let query =
        listener.wait(SIGN_IN_TIMEOUT, &|q: &str| done_page(q.split('&').any(|kv| kv.starts_with("code=")))).await?;
    finish(cfg, &query, &state, &pkce, &redirect_uri).await
}

/// The query of the redirect a phone's browser session hands back
/// (`de.mousewerk.arcalo.mail://oauth?code=…&state=…`, also with the answer after `#`).
pub fn redirect_query(url: &str) -> String {
    let rest = url.split_once('?').map_or("", |(_, q)| q);
    let rest = if rest.is_empty() { url.split_once('#').map_or("", |(_, f)| f) } else { rest };
    rest.split('#').next().unwrap_or_default().to_string()
}

/// Signs in on a phone: `session` opens the provider's address in the platform's browser
/// session (Custom Tabs on Android, `ASWebAuthenticationSession` on iOS) and resolves with the
/// address the provider redirected to ([`MOBILE_REDIRECT`]); then the code is exchanged as on
/// the desktop. A session that ends without a redirect (the user closed it) is an `Err` from
/// `session`.
pub async fn sign_in_with_session<F, Fut>(
    cfg: &OAuthConfig,
    login_hint: &str,
    redirect_uri: &str,
    session: F,
) -> Result<Tokens>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    let pkce = pkce()?;
    let state = b64url(&random(16)?);
    let url = authorize_url(cfg, redirect_uri, &pkce, &state, login_hint);
    let back = match tokio::time::timeout(SIGN_IN_TIMEOUT, session(url)).await {
        Ok(r) => r?,
        Err(_) => {
            return Err(Error::Network(
                tr!(
                    "Die Anmeldung im Browser wurde nicht rechtzeitig abgeschlossen.",
                    "The sign-in in the browser was not finished in time."
                )
                .into(),
            ));
        }
    };
    if !back.starts_with(redirect_uri.trim_end_matches('/')) {
        return Err(Problem::Protocol {
            detail: tr!("Antwort passt nicht zur Anmeldung", "answer does not match the sign-in").into(),
        }
        .into());
    }
    finish(cfg, &redirect_query(&back), &state, &pkce, redirect_uri).await
}

/// What a phone says when the browser session was closed without signing in.
pub fn cancelled() -> Error {
    Error::Invalid(tr!("Die Anmeldung wurde abgebrochen.", "The sign-in was cancelled.").into())
}

// ---------------------------------------------------------------------------- token source

/// Called with a new refresh token (rotation), to put it in the credential store.
pub type OnRefreshToken = Arc<dyn Fn(&str) + Send + Sync>;

/// Access tokens for an account: kept in memory, refreshed [`REFRESH_MARGIN`] before they run
/// out or when the server refused one.
pub struct TokenSource {
    cfg: OAuthConfig,
    refresh_token: Mutex<String>,
    access: Mutex<Option<(String, i64)>>,
    on_refresh_token: Option<OnRefreshToken>,
}

impl TokenSource {
    pub fn new(cfg: OAuthConfig, refresh_token: String, on_refresh_token: Option<OnRefreshToken>) -> Self {
        TokenSource { cfg, refresh_token: Mutex::new(refresh_token), access: Mutex::new(None), on_refresh_token }
    }

    /// After signing in again: the new refresh token, and the access token that came with it.
    pub fn signed_in(&self, refresh_token: String, access: Option<(String, i64)>) {
        *self.refresh_token.lock().unwrap_or_else(|e| e.into_inner()) = refresh_token;
        *self.access.lock().unwrap_or_else(|e| e.into_inner()) = access;
    }

    /// A valid access token; `force`: a new one even if the old one has not run out (the
    /// server refused it).
    pub async fn access_token(&self, force: bool) -> Result<String> {
        if !force
            && let Some((t, exp)) = self.access.lock().unwrap_or_else(|e| e.into_inner()).clone()
            && exp - REFRESH_MARGIN > now()
        {
            return Ok(t);
        }
        let rt = self.refresh_token.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if rt.is_empty() {
            return Err(Problem::Auth { detail: tr!("nicht angemeldet", "not signed in").into() }.into());
        }
        let t = refresh(&self.cfg, &rt).await?;
        if let Some(new) = &t.refresh_token
            && *new != rt
        {
            *self.refresh_token.lock().unwrap_or_else(|e| e.into_inner()) = new.clone();
            if let Some(f) = &self.on_refresh_token {
                f(new);
            }
        }
        *self.access.lock().unwrap_or_else(|e| e.into_inner()) = Some((t.access_token.clone(), t.expires_at));
        Ok(t.access_token)
    }
}

/// The SASL XOAUTH2 initial response (`user=…^Aauth=Bearer …^A^A`), Base64.
pub fn xoauth2(user: &str, access_token: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("user={user}\x01auth=Bearer {access_token}\x01\x01"))
}

/// What the setup and the account's sign-in get back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignedIn {
    /// The account's address as the provider knows it.
    pub email: Option<String>,
}

/// Explains why a sign-in failed, for errors that are not a [`Problem`].
pub fn describe(e: &Error) -> String {
    match e {
        Error::Connection(p) => p.to_string(),
        other => trf!("Anmeldung fehlgeschlagen: {other}", "Sign-in failed: {other}"),
    }
}

#[cfg(any(test, feature = "fake"))]
pub mod fake;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::tests::testhttp::{self, Reply};

    fn cfg(port: u16) -> OAuthConfig {
        OAuthConfig {
            provider: OAuthProvider::Microsoft,
            client_id: "test-client".into(),
            authorize_url: format!("http://127.0.0.1:{port}/oauth/authorize"),
            token_url: format!("http://127.0.0.1:{port}/oauth/token"),
            scopes: MS_SCOPES.iter().map(|s| s.to_string()).collect(),
        }
    }

    async fn idp() -> (u16, Arc<Mutex<fake::FakeOAuth>>) {
        let f = Arc::new(Mutex::new(fake::FakeOAuth::new("maria@example.org")));
        let g = f.clone();
        let port = testhttp::serve(Arc::new(move |method, target, _h, body| {
            let r = g.lock().unwrap().handle(method, target, body);
            Reply { status: r.status, content_type: r.content_type, body: r.body, location: r.location }
        }))
        .await;
        (port, f)
    }

    #[test]
    fn pkce_and_the_authorize_address() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let p = pkce().unwrap();
        assert_eq!(p.verifier.len(), 43);
        let url = authorize_url(&cfg(1), "http://127.0.0.1:5000", &p, "st", "maria@example.org");
        for part in [
            "client_id=test-client",
            "code_challenge_method=S256",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A5000",
            "scope=offline_access+https%3A%2F%2Foutlook.office.com%2FIMAP.AccessAsUser.All",
            "login_hint=maria%40example.org",
        ] {
            assert!(url.contains(part), "{part} in {url}");
        }
        assert_eq!(form_decode("a=b+c&d=%C3%A4%2F"), vec![("a".into(), "b c".into()), ("d".into(), "ä/".into())]);
    }

    #[test]
    fn provider_errors_are_explained() {
        let admin = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented.\r\nTrace ID: x","error_codes":[65001]}"#;
        assert!(matches!(token_problem(400, admin), Error::Connection(Problem::AdminConsent { .. })));
        let gone = r#"{"error":"invalid_grant","error_description":"AADSTS70008: The refresh token has expired."}"#;
        let e = token_problem(400, gone);
        assert!(matches!(&e, Error::Connection(p) if p.needs_sign_in()), "{e:?}");
        assert!(matches!(token_problem(503, "{}"), Error::Network(_)));
        assert!(Problem::AdminConsent { detail: String::new() }.needs_sign_in());
    }

    /// The whole desktop flow against the fake identity provider: the browser is played by an
    /// HTTP request that follows the authorize address.
    #[tokio::test]
    async fn signs_in_through_the_loopback_redirect_and_refreshes() {
        let (port, idp) = idp().await;
        let c = cfg(port);
        let tokens = sign_in(&c, "maria@example.org", |url| {
            assert!(url.starts_with(&c.authorize_url));
            let url = url.to_string();
            tokio::spawn(async move {
                let client = network::client_for(Service::OAuthMicrosoft).unwrap();
                let _ = client.get(&url).send().await.unwrap().text().await;
            });
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(tokens.email.as_deref(), Some("maria@example.org"));
        assert!(idp.lock().unwrap().valid(&tokens.access_token));
        let rt = tokens.refresh_token.clone().unwrap();

        // Refresh before expiry and when forced; the rotated refresh token is handed on.
        let rotated = Arc::new(Mutex::new(vec![]));
        let r2 = rotated.clone();
        let src =
            TokenSource::new(c.clone(), rt, Some(Arc::new(move |t: &str| r2.lock().unwrap().push(t.to_string()))));
        let a1 = src.access_token(false).await.unwrap();
        assert_eq!(src.access_token(false).await.unwrap(), a1, "kept while valid");
        let a2 = src.access_token(true).await.unwrap();
        assert_ne!(a1, a2);
        assert_eq!(rotated.lock().unwrap().len(), 2, "each refresh rotated the refresh token");
        assert!(idp.lock().unwrap().valid(&a2));

        // A short-lived token is refreshed ahead of its expiry.
        idp.lock().unwrap().expires_in = 60;
        let a3 = src.access_token(true).await.unwrap();
        let a4 = src.access_token(false).await.unwrap();
        assert_ne!(a3, a4, "within the margin");

        // A revoked refresh token: sign in again.
        idp.lock().unwrap().revoke_all();
        let e = src.access_token(true).await.unwrap_err();
        assert!(matches!(&e, Error::Connection(p) if p.needs_sign_in()), "{e:?}");
    }

    #[tokio::test]
    async fn admin_consent_and_cancel_from_the_redirect() {
        let (port, idp) = idp().await;
        let c = cfg(port);
        for (mode, check) in [
            (
                "admin_consent",
                (|e: &Error| matches!(e, Error::Connection(Problem::AdminConsent { .. }))) as fn(&Error) -> bool,
            ),
            ("access_denied", |e: &Error| matches!(e, Error::Invalid(_))),
        ] {
            idp.lock().unwrap().next_error = Some(mode.into());
            let e = sign_in(&c, "", |url| {
                let url = url.to_string();
                tokio::spawn(async move {
                    let client = network::client_for(Service::OAuthMicrosoft).unwrap();
                    let _ = client.get(&url).send().await.unwrap().text().await;
                });
                Ok(())
            })
            .await
            .unwrap_err();
            assert!(check(&e), "{mode}: {e:?}");
        }
    }

    /// The phones' flow: the browser session is played by the fake's authorize endpoint, whose
    /// redirect to the app's scheme is handed back as the session's result.
    #[tokio::test]
    async fn signs_in_through_the_mobile_redirect() {
        let (port, idp) = idp().await;
        let c = cfg(port);
        let session = |idp: Arc<Mutex<fake::FakeOAuth>>| {
            move |url: String| async move {
                assert!(url.contains("redirect_uri=de.mousewerk.arcalo.mail%3A%2F%2Foauth"), "{url}");
                let target = url.split_once("/oauth/").map(|(_, t)| format!("/oauth/{t}")).unwrap();
                let reply = idp.lock().unwrap().handle("GET", &target, b"");
                Ok(reply.location.unwrap())
            }
        };
        let tokens =
            sign_in_with_session(&c, "maria@example.org", MOBILE_REDIRECT, session(idp.clone())).await.unwrap();
        assert_eq!(tokens.email.as_deref(), Some("maria@example.org"));
        assert!(idp.lock().unwrap().valid(&tokens.access_token));

        // Refused or cancelled in the browser.
        idp.lock().unwrap().next_error = Some("access_denied".into());
        let e = sign_in_with_session(&c, "", MOBILE_REDIRECT, session(idp.clone())).await.unwrap_err();
        assert!(matches!(e, Error::Invalid(_)), "{e:?}");
        idp.lock().unwrap().next_error = Some("admin_consent".into());
        let e = sign_in_with_session(&c, "", MOBILE_REDIRECT, session(idp.clone())).await.unwrap_err();
        assert!(matches!(e, Error::Connection(Problem::AdminConsent { .. })), "{e:?}");

        // The session closed without a redirect, or something else came back.
        let e = sign_in_with_session(&c, "", MOBILE_REDIRECT, |_| async { Err(cancelled()) }).await.unwrap_err();
        assert!(matches!(e, Error::Invalid(_)), "{e:?}");
        let e = sign_in_with_session(&c, "", MOBILE_REDIRECT, |_| async { Ok("https://evil.example/?code=x".into()) })
            .await
            .unwrap_err();
        assert!(matches!(e, Error::Connection(Problem::Protocol { .. })), "{e:?}");
    }

    #[test]
    fn the_query_of_a_redirect() {
        assert_eq!(redirect_query("de.mousewerk.arcalo.mail://oauth?code=a&state=b"), "code=a&state=b");
        assert_eq!(redirect_query("de.mousewerk.arcalo.mail://oauth/?code=a#frag"), "code=a");
        assert_eq!(redirect_query("de.mousewerk.arcalo.mail://oauth#code=a&state=b"), "code=a&state=b");
        assert_eq!(redirect_query("de.mousewerk.arcalo.mail://oauth"), "");
    }

    #[test]
    fn xoauth2_initial_response() {
        let s = xoauth2("maria@example.org", "tok");
        let raw = base64::engine::general_purpose::STANDARD.decode(s).unwrap();
        assert_eq!(raw, b"user=maria@example.org\x01auth=Bearer tok\x01\x01");
    }
}
