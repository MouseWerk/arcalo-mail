//! Microsoft Graph for what IMAP does not offer a Microsoft account: its inbox rules
//! (`/me/mailFolders/inbox/messageRules`, delegated permission MailboxSettings.ReadWrite) and its
//! contacts with their pictures (`/me/contacts`, Contacts.Read). The access token comes from the
//! account's refresh token with Graph's scopes ([`crate::oauth::graph_config`]); requests go
//! through [`network::client_for`] (`Service::Graph`, `Service::Contacts`).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::network::{self, HttpClient, Service};
use crate::rules::{Action, Field, Op, Rule};
use crate::{tr, trf};

pub const GRAPH_URL: &str = "https://graph.microsoft.com/v1.0";

/// Graph's address: `ARCALO_MAIL_GRAPH_URL` stands in for it in the e2e tests, on this computer
/// only and only together with the test identity provider.
pub fn base_url() -> String {
    if crate::oauth::test_servers().is_some()
        && let Ok(u) = std::env::var("ARCALO_MAIL_GRAPH_URL")
        && u.starts_with("http://127.0.0.1")
    {
        return u.trim_end_matches('/').to_string();
    }
    GRAPH_URL.into()
}

/// Why a rule cannot be a Microsoft inbox rule (`None`: it can).
pub fn unsupported(rule: &Rule) -> Option<String> {
    if !rule.body.match_all && rule.body.conditions.len() > 1 {
        return Some(
            tr!("Microsoft verknüpft Bedingungen immer mit „und“.", "Microsoft always combines conditions with “and”.")
                .into(),
        );
    }
    for c in &rule.body.conditions {
        let bad = match c.field {
            Field::Age => Some(tr!("Microsoft kennt kein Alter von Nachrichten.", "Microsoft has no age of messages.")),
            Field::Category => Some(tr!(
                "Die Bereiche des Posteingangs gibt es nur in der App.",
                "The inbox tabs exist in the app only."
            )),
            _ if c.op == Op::StartsWith => {
                Some(tr!("Microsoft kennt „beginnt mit“ nicht.", "Microsoft has no “starts with”."))
            }
            Field::HasAttachment if c.op == Op::NotContains => None,
            _ => None,
        };
        if let Some(b) = bad {
            return Some(b.into());
        }
    }
    for a in &rule.body.actions {
        match a {
            Action::Flag => {
                return Some(tr!("Microsoft-Regeln können nicht markieren.", "Microsoft rules cannot flag.").into());
            }
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

fn addresses(list: &[&str]) -> Value {
    Value::Array(list.iter().map(|a| json!({"emailAddress": {"address": a.trim()}})).collect())
}

/// A rule as Graph's `messageRule`. `folder`: the id of a folder by name (`archive` and
/// `deleteditems` are Graph's well-known names).
pub fn rule_json(rule: &Rule, sequence: i64, folder: &dyn Fn(&str) -> Option<String>) -> Result<Value> {
    if let Some(why) = unsupported(rule) {
        return Err(Error::Invalid(why));
    }
    let mut cond = serde_json::Map::new();
    let mut except = serde_json::Map::new();
    let push = |map: &mut serde_json::Map<String, Value>, key: &str, v: Value| match (map.get_mut(key), v) {
        (Some(Value::Array(a)), Value::Array(b)) => a.extend(b),
        (_, v) => {
            map.insert(key.into(), v);
        }
    };
    for c in &rule.body.conditions {
        let v = c.value.trim();
        let target = if c.op == Op::NotContains { &mut except } else { &mut cond };
        match c.field {
            Field::From if c.op == Op::Is => push(target, "fromAddresses", addresses(&[v])),
            Field::From => push(target, "senderContains", json!([v])),
            Field::To if c.op == Op::Is => push(target, "sentToAddresses", addresses(&[v])),
            Field::To | Field::Cc => push(target, "recipientContains", json!([v])),
            Field::Subject => push(target, "subjectContains", json!([v])),
            Field::List => push(target, "headerContains", json!([format!("List-Id: {v}")])),
            Field::HasAttachment => {
                let want = !matches!(v, "false" | "no" | "0");
                push(target, "hasAttachments", json!(want));
            }
            Field::Size => {
                let kb = v.parse::<u64>().unwrap_or(0);
                let range = if c.op == Op::Under {
                    json!({"minimumSize": 0, "maximumSize": kb})
                } else {
                    json!({"minimumSize": kb, "maximumSize": 2_147_483_647u64})
                };
                push(target, "withinSizeRange", range);
            }
            Field::Age | Field::Category => {}
        }
    }
    let mut actions = serde_json::Map::new();
    for a in &rule.body.actions {
        match a {
            Action::Move { folder: name } => {
                let id = folder(name.trim()).ok_or_else(|| {
                    Error::Invalid(trf!(
                        "Den Ordner „{}“ gibt es bei Microsoft nicht.",
                        "There is no folder “{}” at Microsoft.",
                        name.trim()
                    ))
                })?;
                actions.insert("moveToFolder".into(), json!(id));
            }
            Action::SkipInbox => {
                actions.insert("moveToFolder".into(), json!(folder("archive").unwrap_or_else(|| "archive".into())));
            }
            Action::Delete => {
                actions.insert("delete".into(), json!(true));
            }
            Action::MarkRead => {
                actions.insert("markAsRead".into(), json!(true));
            }
            Action::Label { keyword } => {
                let list = actions.entry("assignCategories").or_insert(json!([]));
                if let Value::Array(l) = list {
                    l.push(json!(keyword.trim()));
                }
            }
            Action::Forward { to } => {
                actions.insert("forwardTo".into(), addresses(&[to]));
            }
            Action::Flag | Action::Pin | Action::Category { .. } => {}
        }
    }
    if rule.body.stop {
        actions.insert("stopProcessingRules".into(), json!(true));
    }
    let mut v = json!({
        "displayName": rule.name.trim(),
        "sequence": sequence.max(1),
        "isEnabled": rule.enabled,
        "conditions": Value::Object(cond),
        "actions": Value::Object(actions),
    });
    if !except.is_empty() {
        v["exceptions"] = Value::Object(except);
    }
    Ok(v)
}

/// A contact of a Microsoft account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphContact {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub org: Option<String>,
}

/// Graph with an access token.
pub struct Graph {
    http: HttpClient,
    base: String,
    token: String,
}

impl Graph {
    pub fn new(token: &str, service: Service) -> Result<Graph> {
        Self::at(&base_url(), token, service)
    }

    /// Graph at `base` (the tests' server on this computer).
    pub fn at(base: &str, token: &str, service: Service) -> Result<Graph> {
        network::check_https_url(base)?;
        Ok(Graph {
            http: network::client_for(service)?,
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
        })
    }

    /// A request with extra headers; the status and the body as they are, so a caller can read a
    /// `412` (the calendar's writes) or a `410` (an expired delta link).
    pub async fn exchange(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>)> {
        let url = if path.starts_with("http") { path.to_string() } else { format!("{}{path}", self.base) };
        network::check_https_url(&url)?;
        let mut req = self.http.request(method, &url).bearer_auth(&self.token);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        if let Some(b) = body {
            req = req.header(reqwest::header::CONTENT_TYPE, "application/json").body(serde_json::to_vec(b)?);
        }
        let resp = req.send().await.map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?.to_vec();
        if status == 401 || status == 403 {
            return Err(Error::Invalid(
                tr!(
                    "Microsoft erlaubt den Zugriff noch nicht. Melde das Konto in den Einstellungen neu an und erlaube den Zugriff auf Regeln, Kontakte und Kalender.",
                    "Microsoft does not allow the access yet. Sign the account in again in Settings and allow access to rules, contacts and calendars."
                )
                .into(),
            ));
        }
        Ok((status, bytes))
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<(u16, Vec<u8>)> {
        let (status, bytes) = self.exchange(method, path, body, &[]).await?;
        if status >= 400 && status != 404 {
            let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
            let msg = v["error"]["message"].as_str().unwrap_or_default().to_string();
            return Err(Error::Network(trf!(
                "Microsoft Graph meldet {status}: {msg}",
                "Microsoft Graph reports {status}: {msg}"
            )));
        }
        Ok((status, bytes))
    }

    async fn json(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Option<Value>> {
        let (status, bytes) = self.send(method, path, body).await?;
        if status == 404 {
            return Ok(None);
        }
        if bytes.is_empty() {
            return Ok(Some(Value::Null));
        }
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    /// The mail folders by name, with Graph's well-known `archive` and `deleteditems`.
    pub async fn folders(&self) -> Result<Vec<(String, String)>> {
        let mut out = vec![];
        let mut next = Some("/me/mailFolders?$top=200&$select=id,displayName".to_string());
        while let Some(path) = next.take() {
            let Some(v) = self.json(reqwest::Method::GET, &path, None).await? else { break };
            for f in v["value"].as_array().cloned().unwrap_or_default() {
                if let (Some(id), Some(name)) = (f["id"].as_str(), f["displayName"].as_str()) {
                    out.push((name.to_string(), id.to_string()));
                }
            }
            next = v["@odata.nextLink"].as_str().map(str::to_owned);
        }
        Ok(out)
    }

    /// Creates or replaces a rule; returns its id.
    pub async fn put_rule(&self, rule: &Rule, sequence: i64, existing: Option<&str>) -> Result<String> {
        let folders = if rule.body.actions.iter().any(|a| matches!(a, Action::Move { .. } | Action::SkipInbox)) {
            self.folders().await?
        } else {
            vec![]
        };
        let lookup = |name: &str| {
            folders
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, id)| id.clone())
                .or_else(|| (name == "archive" || name == "deleteditems").then(|| name.to_string()))
        };
        let body = rule_json(rule, sequence, &lookup)?;
        let path = "/me/mailFolders/inbox/messageRules";
        if let Some(id) = existing {
            let p = format!("{path}/{id}");
            if self.json(reqwest::Method::PATCH, &p, Some(&body)).await?.is_some() {
                return Ok(id.to_string());
            }
        }
        let v = self.json(reqwest::Method::POST, path, Some(&body)).await?.unwrap_or_default();
        v["id"].as_str().map(str::to_owned).ok_or_else(|| Error::Network("Microsoft Graph: no rule id".into()))
    }

    pub async fn delete_rule(&self, id: &str) -> Result<()> {
        self.send(reqwest::Method::DELETE, &format!("/me/mailFolders/inbox/messageRules/{id}"), None).await?;
        Ok(())
    }

    /// Every contact of the account.
    pub async fn contacts(&self) -> Result<Vec<GraphContact>> {
        let mut out = vec![];
        let mut next = Some(
            "/me/contacts?$top=500&$select=id,displayName,emailAddresses,mobilePhone,businessPhones,companyName"
                .to_string(),
        );
        while let Some(path) = next.take() {
            let Some(v) = self.json(reqwest::Method::GET, &path, None).await? else { break };
            for c in v["value"].as_array().cloned().unwrap_or_default() {
                let emails: Vec<String> = c["emailAddresses"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| e["address"].as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| s.contains('@'))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut phones: Vec<String> = c["businessPhones"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|p| p.as_str()).map(str::to_owned).collect())
                    .unwrap_or_default();
                if let Some(m) = c["mobilePhone"].as_str().filter(|m| !m.is_empty()) {
                    phones.push(m.to_string());
                }
                out.push(GraphContact {
                    id: c["id"].as_str().unwrap_or_default().to_string(),
                    name: c["displayName"].as_str().unwrap_or_default().to_string(),
                    emails,
                    phones,
                    org: c["companyName"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
                });
            }
            next = v["@odata.nextLink"].as_str().map(str::to_owned);
        }
        Ok(out)
    }

    /// A contact's picture (type, bytes), if it has one.
    pub async fn contact_photo(&self, id: &str) -> Result<Option<(String, Vec<u8>)>> {
        let (status, bytes) = self.send(reqwest::Method::GET, &format!("/me/contacts/{id}/photo/$value"), None).await?;
        if status == 404 || bytes.is_empty() {
            return Ok(None);
        }
        let mime = crate::avatars::sniff(&bytes).unwrap_or("image/jpeg").to_string();
        Ok(Some((mime, bytes)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::network::tests::testhttp::{Reply, serve};
    use crate::rules::tests::{cond, rule};

    #[test]
    fn rules_as_message_rules() {
        let mut r = rule(
            "Rechnungen",
            vec![
                cond(Field::From, Op::Is, "billing@shop.example"),
                cond(Field::Subject, Op::NotContains, "Mahnung"),
                cond(Field::Size, Op::Under, "200"),
            ],
            vec![
                Action::Move { folder: "Rechnungen".into() },
                Action::MarkRead,
                Action::Label { keyword: "Finanzen".into() },
            ],
        );
        r.body.stop = true;
        let v = rule_json(&r, 3, &|n| (n == "Rechnungen").then(|| "AAMk-1".to_string())).unwrap();
        assert_eq!(v["sequence"], 3);
        assert_eq!(v["conditions"]["fromAddresses"][0]["emailAddress"]["address"], "billing@shop.example");
        assert_eq!(v["exceptions"]["subjectContains"], json!(["Mahnung"]));
        assert_eq!(v["conditions"]["withinSizeRange"]["maximumSize"], 200);
        assert_eq!(v["actions"]["moveToFolder"], "AAMk-1");
        assert_eq!(v["actions"]["assignCategories"], json!(["Finanzen"]));
        assert_eq!(v["actions"]["stopProcessingRules"], true);
        let missing = rule_json(&r, 1, &|_| None).unwrap_err().to_string();
        assert!(missing.contains("“Rechnungen”"), "{missing}");
        assert!(unsupported(&rule("x", vec![cond(Field::Subject, Op::Contains, "a")], vec![Action::Flag])).is_some());
        let mut any = rule(
            "y",
            vec![cond(Field::Subject, Op::Contains, "a"), cond(Field::Subject, Op::Contains, "b")],
            vec![Action::MarkRead],
        );
        any.body.match_all = false;
        assert!(unsupported(&any).is_some());
    }

    #[tokio::test]
    async fn creates_replaces_and_deletes_rules_and_reads_contacts() {
        let seen: Arc<Mutex<Vec<(String, String, String)>>> = Arc::default();
        let log = seen.clone();
        let me: Arc<Mutex<u16>> = Arc::default();
        let own = me.clone();
        let port = serve(Arc::new(move |method: &str, path: &str, headers: &[(String, String)], body: &[u8]| {
            assert!(headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer tok"));
            log.lock().unwrap().push((method.into(), path.into(), String::from_utf8_lossy(body).into()));
            match (method, path.split('?').next().unwrap_or_default()) {
                ("GET", "/v1.0/me/mailFolders") => Reply::status(200, r#"{"value":[{"id":"F1","displayName":"Shop"}]}"#),
                ("POST", "/v1.0/me/mailFolders/inbox/messageRules") => Reply::status(201, r#"{"id":"R1"}"#),
                ("PATCH", "/v1.0/me/mailFolders/inbox/messageRules/R1") => Reply::status(200, r#"{"id":"R1"}"#),
                ("PATCH", _) => Reply::status(404, r#"{"error":{"message":"gone"}}"#),
                ("DELETE", _) => Reply::status(204, ""),
                ("GET", "/v1.0/me/contacts") if path.contains("skip") => Reply::status(
                    200,
                    r#"{"value":[{"id":"C2","displayName":"Tom","emailAddresses":[],"businessPhones":[]}]}"#,
                ),
                ("GET", "/v1.0/me/contacts") => Reply::status(
                    200,
                    &format!(
                        r#"{{"value":[{{"id":"C1","displayName":"Lea Weber","emailAddresses":[{{"address":"lea@example.org"}}],"businessPhones":["+49 30 1"],"mobilePhone":"","companyName":"Weber GmbH"}}],"@odata.nextLink":"http://127.0.0.1:{}/v1.0/me/contacts?skip=1"}}"#,
                        *own.lock().unwrap()
                    ),
                ),
                ("GET", "/v1.0/me/contacts/C1/photo/$value") => Reply { status: 200, content_type: "image/png".into(), body: b"\x89PNG\r\n\x1a\nxx".to_vec(), location: None, headers: vec![] },
                _ => Reply::status(404, "{}"),
            }
        }))
        .await;
        *me.lock().unwrap() = port;
        let g = Graph::at(&format!("http://127.0.0.1:{port}/v1.0"), "tok", Service::Graph).unwrap();
        let r =
            rule("Shop", vec![cond(Field::From, Op::Contains, "shop")], vec![Action::Move { folder: "Shop".into() }]);
        assert_eq!(g.put_rule(&r, 1, None).await.unwrap(), "R1");
        assert_eq!(g.put_rule(&r, 1, Some("R1")).await.unwrap(), "R1");
        assert_eq!(g.put_rule(&r, 1, Some("R9")).await.unwrap(), "R1", "a rule removed on the server is made again");
        g.delete_rule("R1").await.unwrap();
        let log = seen.lock().unwrap().clone();
        let post = log.iter().find(|(m, _, _)| m == "POST").unwrap();
        assert!(post.2.contains(r#""moveToFolder":"F1""#), "{}", post.2);
        assert!(log.iter().any(|(m, p, _)| m == "DELETE" && p.ends_with("/R1")));
        let contacts = g.contacts().await.unwrap();
        assert_eq!(contacts.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Lea Weber", "Tom"], "both pages");
        assert_eq!(contacts[0].emails, ["lea@example.org"]);
        assert_eq!(contacts[0].phones, ["+49 30 1"]);
        assert_eq!(contacts[0].org.as_deref(), Some("Weber GmbH"));
        let photo = g.contact_photo("C1").await.unwrap().unwrap();
        assert_eq!(photo.0, "image/png");
        assert!(g.contact_photo("C9").await.unwrap().is_none());
    }
}
