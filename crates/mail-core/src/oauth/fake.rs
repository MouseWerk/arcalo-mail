//! An identity provider in memory for tests: the authorize endpoint signs the user in at once
//! (a redirect with a code, or the error a test asked for), the token endpoint checks PKCE and
//! hands out access and rotating refresh tokens that the test servers accept for XOAUTH2. The
//! core's tests serve it on loopback, the test server (`crates/mail-testserver`) on its control
//! port.

use std::collections::{HashMap, HashSet};

use base64::Engine;

use super::{challenge_of, form, form_decode, get};

/// One HTTP answer.
#[derive(Debug, Clone)]
pub struct FakeReply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub location: Option<String>,
}

impl FakeReply {
    fn json(status: u16, v: serde_json::Value) -> FakeReply {
        FakeReply { status, content_type: "application/json".into(), body: v.to_string().into_bytes(), location: None }
    }
}

#[derive(Debug, Default)]
pub struct FakeOAuth {
    /// The address the ID token names.
    pub email: String,
    codes: HashMap<String, (String, String)>,
    access: HashMap<String, i64>,
    refresh: HashSet<String>,
    counter: u64,
    /// Lifetime of new access tokens in seconds.
    pub expires_in: i64,
    /// The next authorize request fails: `admin_consent` or `access_denied`.
    pub next_error: Option<String>,
    /// Token requests fail with this answer: `admin_consent`.
    pub token_error: Option<String>,
    /// Token requests so far (the tests count refreshes).
    pub token_requests: u32,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl FakeOAuth {
    pub fn new(email: &str) -> Self {
        FakeOAuth { email: email.into(), expires_in: 3600, ..Default::default() }
    }

    /// Whether an access token is one of ours and still valid.
    pub fn valid(&self, token: &str) -> bool {
        self.access.get(token).is_some_and(|exp| *exp > now())
    }

    /// Every token stops working (the password changed, the admin revoked the app).
    pub fn revoke_all(&mut self) {
        self.access.clear();
        self.refresh.clear();
    }

    /// Access tokens stop working, refresh tokens keep working (they expired).
    pub fn expire_access(&mut self) {
        self.access.clear();
    }

    fn next(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}-{}-{}", self.counter, now())
    }

    fn tokens(&mut self) -> FakeReply {
        let access = self.next("at");
        let refresh = self.next("rt");
        self.access.insert(access.clone(), now() + self.expires_in);
        self.refresh.insert(refresh.clone());
        let b64 = |v: serde_json::Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        let id_token = format!(
            "{}.{}.sig",
            b64(serde_json::json!({"alg": "none"})),
            b64(serde_json::json!({"preferred_username": self.email, "email": self.email}))
        );
        FakeReply::json(
            200,
            serde_json::json!({
                "token_type": "Bearer",
                "access_token": access,
                "refresh_token": refresh,
                "expires_in": self.expires_in,
                "id_token": id_token,
            }),
        )
    }

    fn admin_consent() -> serde_json::Value {
        serde_json::json!({
            "error": "invalid_grant",
            "error_description": "AADSTS65001: The user or administrator has not consented to use the application.",
            "error_codes": [65001],
        })
    }

    /// Answers a request to `…/authorize` or `…/token`.
    pub fn handle(&mut self, method: &str, target: &str, body: &[u8]) -> FakeReply {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if method == "GET" && path.ends_with("/authorize") {
            let q = form_decode(query);
            let redirect = get(&q, "redirect_uri").unwrap_or_default().to_string();
            let state = get(&q, "state").unwrap_or_default().to_string();
            let answer = match self.next_error.take().as_deref() {
                Some("admin_consent") => form(&[
                    ("error", "consent_required"),
                    ("error_description", "AADSTS65001: The user or administrator has not consented."),
                    ("state", &state),
                ]),
                Some(other) => {
                    form(&[("error", other), ("error_description", "The user cancelled."), ("state", &state)])
                }
                None => {
                    let code = self.next("code");
                    let challenge = get(&q, "code_challenge").unwrap_or_default().to_string();
                    self.codes.insert(code.clone(), (challenge, redirect.clone()));
                    form(&[("code", &code), ("state", &state)])
                }
            };
            let location = format!("{redirect}/?{answer}");
            return FakeReply {
                status: 302,
                content_type: "text/plain".into(),
                body: b"redirect".to_vec(),
                location: Some(location),
            };
        }
        if method == "POST" && path.ends_with("/token") {
            self.token_requests += 1;
            if self.token_error.as_deref() == Some("admin_consent") {
                return FakeReply::json(400, Self::admin_consent());
            }
            let f = form_decode(&String::from_utf8_lossy(body));
            let bad = |what: &str| {
                FakeReply::json(400, serde_json::json!({"error": "invalid_grant", "error_description": what}))
            };
            return match get(&f, "grant_type") {
                Some("authorization_code") => {
                    let code = get(&f, "code").unwrap_or_default();
                    let Some((challenge, redirect)) = self.codes.remove(code) else { return bad("unknown code") };
                    if challenge != challenge_of(get(&f, "code_verifier").unwrap_or_default()) {
                        return bad("PKCE verification failed");
                    }
                    if redirect != get(&f, "redirect_uri").unwrap_or_default() {
                        return bad("redirect_uri does not match");
                    }
                    self.tokens()
                }
                Some("refresh_token") => {
                    let rt = get(&f, "refresh_token").unwrap_or_default().to_string();
                    if !self.refresh.remove(&rt) {
                        return bad("AADSTS70008: The refresh token has expired or was revoked.");
                    }
                    self.tokens()
                }
                _ => bad("unsupported grant_type"),
            };
        }
        FakeReply::json(404, serde_json::json!({"error": "not_found"}))
    }
}
