//! CalDAV (RFC 4791 on WebDAV, RFC 6578 sync): iCloud, Fastmail, mailbox.org, Posteo, Nextcloud
//! and any other server. Discovery from the server's address (`/.well-known/caldav`, the
//! principal, its `calendar-home-set`, the calendars with name, color and the right to write),
//! then per calendar what changed since the last sync token (`sync-collection`), or by CTag and
//! ETags where a server has no tokens, the objects fetched together (`calendar-multiget`).
//! Writes carry the ETag they are based on (`If-Match`, `If-None-Match: *` for new ones); a
//! `412` is a conflict the user settles.

use serde::Serialize;

use crate::error::{Error, Result};
use crate::network::{self, HttpClient, Service};
use crate::{tr, trf};

/// Servers people use, with the address to start from (`{host}`: the user's own server).
pub const PRESETS: &[(&str, &str)] = &[
    ("iCloud", "https://caldav.icloud.com"),
    ("Fastmail", "https://caldav.fastmail.com"),
    ("mailbox.org", "https://dav.mailbox.org"),
    ("Posteo", "https://posteo.de:8443"),
    ("Nextcloud", "https://{host}/remote.php/dav"),
];

const DAV: &str = "DAV:";
const CAL: &str = "urn:ietf:params:xml:ns:caldav";
const APPLE: &str = "http://apple.com/ns/ical/";
const CS: &str = "http://calendarserver.org/ns/";

/// A server and the user's sign-in.
pub struct CalDav {
    http: HttpClient,
    user: String,
    password: String,
}

/// A calendar collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteCalendar {
    pub url: String,
    pub name: String,
    pub color: Option<String>,
    pub writable: bool,
    pub ctag: Option<String>,
    pub sync_token: Option<String>,
}

/// What discovery found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub calendars: Vec<RemoteCalendar>,
    /// The server sends invitations and updates itself (`calendar-auto-schedule`).
    pub auto_schedule: bool,
}

/// One `<response>` of a multistatus.
#[derive(Debug, Clone, Default)]
struct Response {
    href: String,
    /// The status of the response itself (sync-collection: `404` for a removed object).
    status: Option<u16>,
    /// Found properties: name → (text, names of the child elements, hrefs inside).
    props: Vec<(String, String, Vec<String>, Vec<String>)>,
}

impl Response {
    fn prop(&self, name: &str) -> Option<&(String, String, Vec<String>, Vec<String>)> {
        self.props.iter().find(|p| p.0 == name)
    }
    fn text(&self, name: &str) -> Option<String> {
        self.prop(name).map(|p| p.1.clone()).filter(|t| !t.is_empty())
    }
}

fn status_code(s: &str) -> Option<u16> {
    s.split_whitespace().nth(1).and_then(|c| c.parse().ok())
}

fn multistatus(xml: &str) -> Result<(Vec<Response>, Option<String>)> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Network(format!("CalDAV: {e}")))?;
    let mut out = vec![];
    for r in doc.descendants().filter(|n| n.has_tag_name((DAV, "response"))) {
        let mut resp = Response {
            href: r
                .children()
                .find(|n| n.has_tag_name((DAV, "href")))
                .and_then(|n| n.text())
                .unwrap_or_default()
                .trim()
                .to_string(),
            status: r.children().find(|n| n.has_tag_name((DAV, "status"))).and_then(|n| n.text()).and_then(status_code),
            props: vec![],
        };
        for ps in r.children().filter(|n| n.has_tag_name((DAV, "propstat"))) {
            let ok = ps
                .children()
                .find(|n| n.has_tag_name((DAV, "status")))
                .and_then(|n| n.text())
                .and_then(status_code)
                .is_none_or(|c| c == 200);
            if !ok {
                continue;
            }
            for prop in ps.children().filter(|n| n.has_tag_name((DAV, "prop"))) {
                for p in prop.children().filter(|n| n.is_element()) {
                    let text: String = p
                        .children()
                        .filter(|c| c.is_text())
                        .filter_map(|c| c.text())
                        .collect::<String>()
                        .trim()
                        .to_string();
                    let mut names = vec![];
                    for d in p.descendants().skip(1).filter(|d| d.is_element()) {
                        names.push(d.tag_name().name().to_string());
                        if let Some(n) = d.attribute("name") {
                            names.push(format!("comp:{}", n.to_ascii_uppercase()));
                        }
                    }
                    let hrefs = p
                        .descendants()
                        .filter(|d| d.has_tag_name((DAV, "href")))
                        .filter_map(|d| d.text())
                        .map(|t| t.trim().to_string())
                        .collect();
                    resp.props.push((p.tag_name().name().to_string(), text, names, hrefs));
                }
            }
        }
        out.push(resp);
    }
    let token = doc
        .descendants()
        .find(|n| {
            n.has_tag_name((DAV, "sync-token")) && n.parent().is_some_and(|p| p.has_tag_name((DAV, "multistatus")))
        })
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string());
    Ok((out, token))
}

/// `href` against `base` (servers answer with paths).
pub fn join(base: &str, href: &str) -> String {
    match url::Url::parse(base).and_then(|b| b.join(href.trim())) {
        Ok(u) => u.to_string(),
        Err(_) => href.to_string(),
    }
}

/// The path of an absolute address (what `<href>` carries).
fn path_of(url: &str) -> String {
    url::Url::parse(url).map(|u| u.path().to_string()).unwrap_or_else(|_| url.to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// An answer of the server.
struct Answer {
    status: u16,
    text: String,
    etag: Option<String>,
    dav: String,
}

/// What a write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// Taken; the new ETag when the server says it (else the object is read again).
    Done(Option<String>),
    /// The object changed (or went) on the server since it was read here.
    Conflict,
}

/// What changed in a calendar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub token: Option<String>,
}

impl CalDav {
    pub fn new(user: &str, password: &str) -> Result<CalDav> {
        Ok(CalDav { http: network::client_for(Service::Calendar)?, user: user.into(), password: password.into() })
    }

    async fn request(
        &self,
        m: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<(&str, String)>,
    ) -> Result<Answer> {
        network::check_https_url(url)?;
        let method = reqwest::Method::from_bytes(m.as_bytes()).unwrap_or(reqwest::Method::GET);
        let mut req = self.http.request(method, url).basic_auth(&self.user, Some(&self.password));
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        if let Some((ctype, b)) = body {
            req = req.header(reqwest::header::CONTENT_TYPE, ctype).body(b);
        }
        let resp = req.send().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let header =
            |n: reqwest::header::HeaderName| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let etag = header(reqwest::header::ETAG);
        let dav = resp.headers().get_all("dav").iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>().join(", ");
        let text = resp.text().await.map_err(|e| Error::Network(e.to_string()))?;
        if status == 401 {
            return Err(Error::Connection(network::Problem::Auth {
                detail: tr!("Benutzername oder App-Passwort stimmt nicht", "user name or app password is wrong").into(),
            }));
        }
        Ok(Answer { status, text, etag, dav })
    }

    async fn propfind(&self, url: &str, depth: &str, props: &str) -> Result<Vec<Response>> {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"{CAL}\" xmlns:a=\"{APPLE}\" xmlns:cs=\"{CS}\"><d:prop>{props}</d:prop></d:propfind>"
        );
        let a =
            self.request("PROPFIND", url, &[("Depth", depth)], Some(("application/xml; charset=utf-8", body))).await?;
        if a.status != 207 {
            return Err(Error::Network(trf!(
                "Der Kalender-Server antwortet nicht wie erwartet ({}).",
                "The calendar server does not answer as expected ({}).",
                a.status
            )));
        }
        Ok(multistatus(&a.text)?.0)
    }

    /// The calendars of the user, from the server's address.
    pub async fn discover(&self, start: &str) -> Result<Discovered> {
        let start = start.trim().trim_end_matches('/').to_string();
        let origin =
            url::Url::parse(&start).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| start.clone());
        let mut principal = None;
        for url in [format!("{start}/"), format!("{origin}/.well-known/caldav")] {
            if let Ok(list) = self.propfind(&url, "0", "<d:current-user-principal/>").await
                && let Some(p) =
                    list.iter().find_map(|r| r.prop("current-user-principal").and_then(|p| p.3.first().cloned()))
                && !p.is_empty()
            {
                principal = Some(join(&url, &p));
                break;
            }
        }
        let principal = principal.ok_or_else(|| {
            Error::Network(
                tr!("Unter dieser Adresse ist kein Kalender zu finden.", "No calendar is to be found at this address.")
                    .into(),
            )
        })?;
        let home = self
            .propfind(&principal, "0", "<c:calendar-home-set/>")
            .await?
            .iter()
            .find_map(|r| r.prop("calendar-home-set").map(|p| p.3.first().cloned().unwrap_or_else(|| p.1.clone())))
            .filter(|h| !h.is_empty())
            .map(|h| join(&principal, &h))
            .ok_or_else(|| {
                Error::Network(tr!("Der Server nennt keinen Kalender.", "The server names no calendar.").into())
            })?;
        let list = self
            .propfind(
                &home,
                "1",
                "<d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/><cs:getctag/><d:sync-token/><d:current-user-privilege-set/>",
            )
            .await?;
        let mut calendars = vec![];
        for r in list {
            let is_cal = r.prop("resourcetype").is_some_and(|p| p.2.iter().any(|n| n == "calendar"));
            if !is_cal {
                continue;
            }
            // Calendars for tasks only (VTODO) are not appointments.
            if let Some(set) = r.prop("supported-calendar-component-set")
                && set.2.iter().any(|n| n.starts_with("comp:"))
                && !set.2.iter().any(|n| n == "comp:VEVENT")
            {
                continue;
            }
            let writable = match r.prop("current-user-privilege-set") {
                Some(p) => p.2.iter().any(|n| matches!(n.as_str(), "write" | "write-content" | "all")),
                None => true,
            };
            calendars.push(RemoteCalendar {
                url: join(&home, &r.href),
                name: r.text("displayname").unwrap_or_else(|| tr!("Kalender", "Calendar").into()),
                color: r.text("calendar-color"),
                writable,
                ctag: r.text("getctag"),
                sync_token: r.text("sync-token"),
            });
        }
        if calendars.is_empty() {
            return Err(Error::Network(tr!("Der Server hat keinen Kalender.", "The server has no calendar.").into()));
        }
        let options = self.request("OPTIONS", &home, &[], None).await.map(|a| a.dav).unwrap_or_default();
        Ok(Discovered { calendars, auto_schedule: options.contains("calendar-auto-schedule") })
    }

    /// What changed since `token` (RFC 6578). `Ok(None)`: the server has no tokens or forgot
    /// this one; the caller compares ETags instead.
    pub async fn changes(&self, calendar: &str, token: &str) -> Result<Option<Changes>> {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:sync-collection xmlns:d=\"DAV:\"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>",
            xml_escape(token)
        );
        let a =
            self.request("REPORT", calendar, &[("Depth", "1")], Some(("application/xml; charset=utf-8", body))).await?;
        if a.status != 207 {
            return Ok(None);
        }
        let (list, token) = multistatus(&a.text)?;
        let mut c = Changes { token, ..Changes::default() };
        for r in list {
            let href = join(calendar, &r.href);
            if href.trim_end_matches('/') == calendar.trim_end_matches('/') {
                continue;
            }
            if r.status == Some(404) {
                c.removed.push(href);
            } else {
                c.changed.push(href);
            }
        }
        Ok(Some(c))
    }

    /// The calendar's CTag and sync token now.
    pub async fn state(&self, calendar: &str) -> Result<(Option<String>, Option<String>)> {
        let list = self.propfind(calendar, "0", "<cs:getctag/><d:sync-token/>").await?;
        let r = list.into_iter().next().unwrap_or_default();
        Ok((r.text("getctag"), r.text("sync-token")))
    }

    /// The objects' ETags of a calendar.
    pub async fn etags(&self, calendar: &str) -> Result<Vec<(String, String)>> {
        let list = self.propfind(calendar, "1", "<d:getetag/><d:resourcetype/>").await?;
        Ok(list
            .into_iter()
            .filter(|r| !r.prop("resourcetype").is_some_and(|p| p.2.iter().any(|n| n == "collection")))
            .filter_map(|r| r.text("getetag").map(|e| (join(calendar, &r.href), e)))
            .collect())
    }

    /// The objects at `hrefs` (absolute), fetched together: href, ETag, data.
    pub async fn multiget(&self, calendar: &str, hrefs: &[String]) -> Result<Vec<(String, Option<String>, String)>> {
        let mut out = vec![];
        for batch in hrefs.chunks(50) {
            let refs: String = batch.iter().map(|h| format!("<d:href>{}</d:href>", xml_escape(&path_of(h)))).collect();
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?><c:calendar-multiget xmlns:d=\"DAV:\" xmlns:c=\"{CAL}\"><d:prop><d:getetag/><c:calendar-data/></d:prop>{refs}</c:calendar-multiget>"
            );
            let a = self
                .request("REPORT", calendar, &[("Depth", "1")], Some(("application/xml; charset=utf-8", body)))
                .await?;
            if a.status != 207 {
                return Err(Error::Network(trf!(
                    "Der Kalender-Server meldet {}.",
                    "The calendar server reports {}.",
                    a.status
                )));
            }
            for r in multistatus(&a.text)?.0 {
                if let Some(data) = r.text("calendar-data") {
                    out.push((join(calendar, &r.href), r.text("getetag"), data));
                }
            }
        }
        Ok(out)
    }

    /// One object: its ETag and data (`None`: gone).
    pub async fn get(&self, href: &str) -> Result<Option<(Option<String>, String)>> {
        let a = self.request("GET", href, &[("Accept", "text/calendar")], None).await?;
        match a.status {
            200 => Ok(Some((a.etag, a.text))),
            404 | 410 => Ok(None),
            s => Err(Error::Network(trf!("Der Kalender-Server meldet {}.", "The calendar server reports {}.", s))),
        }
    }

    /// Writes an object: a new one (`etag` `None`, refused when one exists at `href`) or over the
    /// version with `etag`.
    pub async fn put(&self, href: &str, data: &str, etag: Option<&str>) -> Result<Written> {
        let cond = match etag {
            Some(e) => ("If-Match", e),
            None => ("If-None-Match", "*"),
        };
        let a = self.request("PUT", href, &[cond], Some(("text/calendar; charset=utf-8", data.to_string()))).await?;
        match a.status {
            200..=299 => Ok(Written::Done(a.etag)),
            412 => Ok(Written::Conflict),
            403 => Err(refused(a.status)),
            s => Err(Error::Network(trf!("Der Kalender-Server meldet {}.", "The calendar server reports {}.", s))),
        }
    }

    /// Deletes an object (the version with `etag`).
    pub async fn delete(&self, href: &str, etag: Option<&str>) -> Result<Written> {
        let headers: Vec<(&str, &str)> = etag.map(|e| vec![("If-Match", e)]).unwrap_or_default();
        let a = self.request("DELETE", href, &headers, None).await?;
        match a.status {
            200..=299 | 404 | 410 => Ok(Written::Done(None)),
            412 => Ok(Written::Conflict),
            403 => Err(refused(a.status)),
            s => Err(Error::Network(trf!("Der Kalender-Server meldet {}.", "The calendar server reports {}.", s))),
        }
    }
}

/// A refusal that trying again does not change.
pub fn refused(status: u16) -> Error {
    Error::Invalid(trf!(
        "Der Kalender-Server lehnt die Änderung ab ({}).",
        "The calendar server refuses the change ({}).",
        status
    ))
}

#[cfg(any(test, feature = "fake"))]
pub mod fake {
    //! A CalDAV server's answers, without sockets (the core's tests and the test server): two
    //! calendars below `/caldav/calendars/user/`, sync tokens, ETags, `If-Match`.

    use std::collections::BTreeMap;

    #[derive(Debug, Clone, Default)]
    pub struct FakeCalendar {
        pub name: String,
        pub color: String,
        pub writable: bool,
        /// href (a path) → (ETag, data).
        pub objects: BTreeMap<String, (String, String)>,
        /// What changed at which token: (token, href, removed).
        pub log: Vec<(u64, String, bool)>,
    }

    #[derive(Debug, Clone)]
    pub struct FakeCalDav {
        pub user: String,
        pub password: String,
        /// Calendar path → calendar.
        pub calendars: BTreeMap<String, FakeCalendar>,
        pub token: u64,
        pub auto_schedule: bool,
        /// Every write that came in: (method, path).
        pub writes: Vec<(String, String)>,
        etags: u64,
    }

    pub const WORK: &str = "/caldav/calendars/user/work/";
    pub const HOME: &str = "/caldav/calendars/user/home/";

    impl FakeCalDav {
        pub fn new(user: &str, password: &str) -> Self {
            let mut calendars = BTreeMap::new();
            calendars.insert(
                WORK.to_string(),
                FakeCalendar { name: "Arbeit".into(), color: "#0D9488FF".into(), writable: true, ..Default::default() },
            );
            calendars.insert(
                HOME.to_string(),
                FakeCalendar { name: "Privat".into(), color: "#9333EA".into(), writable: true, ..Default::default() },
            );
            FakeCalDav {
                user: user.into(),
                password: password.into(),
                calendars,
                token: 1,
                auto_schedule: false,
                writes: vec![],
                etags: 0,
            }
        }

        fn next_etag(&mut self) -> String {
            self.etags += 1;
            format!("\"e{}\"", self.etags)
        }

        /// Puts an object as another client would (a new ETag, logged for sync).
        pub fn put(&mut self, calendar: &str, name: &str, data: &str) -> String {
            let href = format!("{calendar}{name}");
            let etag = self.next_etag();
            self.token += 1;
            let token = self.token;
            let c = self.calendars.get_mut(calendar).expect("calendar");
            c.objects.insert(href.clone(), (etag.clone(), data.to_string()));
            c.log.push((token, href.clone(), false));
            href
        }

        /// Removes an object as another client would.
        pub fn remove(&mut self, href: &str) {
            self.token += 1;
            let token = self.token;
            for c in self.calendars.values_mut() {
                if c.objects.remove(href).is_some() {
                    c.log.push((token, href.to_string(), true));
                }
            }
        }

        fn calendar_of(&self, path: &str) -> Option<String> {
            self.calendars.keys().find(|k| path.starts_with(k.as_str()) && path.len() > k.len()).cloned()
        }

        fn multistatus(inner: &str, token: Option<u64>) -> (u16, Vec<(String, String)>, String) {
            let token = token.map(|t| format!("<d:sync-token>http://fake/sync/{t}</d:sync-token>")).unwrap_or_default();
            (
                207,
                vec![],
                format!(
                    "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:a=\"http://apple.com/ns/ical/\" xmlns:cs=\"http://calendarserver.org/ns/\">{inner}{token}</d:multistatus>"
                ),
            )
        }

        /// Answers a request: status, extra headers, body. `auth`: the `Authorization` header;
        /// `headers`: the request's headers (lower-case names).
        pub fn handle(
            &mut self,
            method: &str,
            path: &str,
            auth: Option<&str>,
            headers: &[(String, String)],
            body: &str,
        ) -> (u16, Vec<(String, String)>, String) {
            use base64::Engine;
            let expected = format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", self.user, self.password))
            );
            if auth != Some(expected.as_str()) {
                return (401, vec![], String::new());
            }
            let header = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
            let ok = |props: &str| {
                format!("<d:propstat><d:prop>{props}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>")
            };
            let path = path.split('?').next().unwrap_or_default().to_string();
            let trimmed = path.trim_end_matches('/');
            let token = self.token;
            match method {
                "OPTIONS" => {
                    let dav = if self.auto_schedule {
                        "1, 3, calendar-access, calendar-auto-schedule"
                    } else {
                        "1, 3, calendar-access"
                    };
                    (200, vec![("DAV".into(), dav.into())], String::new())
                }
                "PROPFIND" => match trimmed {
                    "/.well-known/caldav" | "/caldav" => Self::multistatus(
                        &format!(
                            "<d:response><d:href>{path}</d:href>{}</d:response>",
                            ok(
                                "<d:current-user-principal><d:href>/caldav/principals/user/</d:href></d:current-user-principal>"
                            )
                        ),
                        None,
                    ),
                    "/caldav/principals/user" => Self::multistatus(
                        &format!(
                            "<d:response><d:href>/caldav/principals/user/</d:href>{}</d:response>",
                            ok("<c:calendar-home-set><d:href>/caldav/calendars/user/</d:href></c:calendar-home-set>")
                        ),
                        None,
                    ),
                    "/caldav/calendars/user" => {
                        let mut inner = format!(
                            "<d:response><d:href>/caldav/calendars/user/</d:href>{}</d:response>",
                            ok("<d:resourcetype><d:collection/></d:resourcetype>")
                        );
                        for (p, c) in &self.calendars {
                            let privs = if c.writable {
                                "<d:privilege><d:read/></d:privilege><d:privilege><d:write/></d:privilege>"
                            } else {
                                "<d:privilege><d:read/></d:privilege>"
                            };
                            inner.push_str(&format!(
                                "<d:response><d:href>{p}</d:href>{}</d:response>",
                                ok(&format!(
                                    "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>{}</d:displayname><a:calendar-color>{}</a:calendar-color><c:supported-calendar-component-set><c:comp name=\"VEVENT\"/></c:supported-calendar-component-set><cs:getctag>c{token}</cs:getctag><d:sync-token>http://fake/sync/{token}</d:sync-token><d:current-user-privilege-set>{privs}</d:current-user-privilege-set>",
                                    c.name, c.color
                                ))
                            ));
                        }
                        Self::multistatus(&inner, None)
                    }
                    _ => {
                        let key = format!("{trimmed}/");
                        let Some(c) = self.calendars.get(&key) else { return (404, vec![], String::new()) };
                        let depth = header("depth").unwrap_or_default();
                        let mut inner = format!(
                            "<d:response><d:href>{key}</d:href>{}</d:response>",
                            ok(&format!(
                                "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype><cs:getctag>c{token}</cs:getctag><d:sync-token>http://fake/sync/{token}</d:sync-token>"
                            ))
                        );
                        if depth == "1" {
                            for (href, (etag, _)) in &c.objects {
                                inner.push_str(&format!(
                                    "<d:response><d:href>{href}</d:href>{}</d:response>",
                                    ok(&format!("<d:getetag>{etag}</d:getetag><d:resourcetype/>"))
                                ));
                            }
                        }
                        Self::multistatus(&inner, None)
                    }
                },
                "REPORT" => {
                    let key = format!("{trimmed}/");
                    let Some(c) = self.calendars.get(&key) else { return (404, vec![], String::new()) };
                    if body.contains("sync-collection") {
                        let since = body
                            .split("<d:sync-token>")
                            .nth(1)
                            .and_then(|r| r.split('<').next())
                            .and_then(|t| t.rsplit('/').next())
                            .and_then(|n| n.parse::<u64>().ok());
                        let Some(since) = since else {
                            return (403, vec![], "<d:error xmlns:d=\"DAV:\"><d:valid-sync-token/></d:error>".into());
                        };
                        let mut seen = std::collections::BTreeSet::new();
                        let mut inner = String::new();
                        for (t, href, _) in c.log.iter().rev() {
                            if *t <= since || !seen.insert(href.clone()) {
                                continue;
                            }
                            match c.objects.get(href) {
                                Some((etag, _)) => inner.push_str(&format!(
                                    "<d:response><d:href>{href}</d:href>{}</d:response>",
                                    ok(&format!("<d:getetag>{etag}</d:getetag>"))
                                )),
                                None => inner.push_str(&format!(
                                    "<d:response><d:href>{href}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"
                                )),
                            }
                        }
                        return Self::multistatus(&inner, Some(token));
                    }
                    let mut inner = String::new();
                    for (href, (etag, data)) in &c.objects {
                        if body.contains(&format!("<d:href>{href}</d:href>")) {
                            let data = data.replace('&', "&amp;").replace('<', "&lt;");
                            inner.push_str(&format!(
                                "<d:response><d:href>{href}</d:href>{}</d:response>",
                                ok(&format!("<d:getetag>{etag}</d:getetag><c:calendar-data>{data}</c:calendar-data>"))
                            ));
                        }
                    }
                    Self::multistatus(&inner, None)
                }
                "GET" => {
                    let Some(cal) = self.calendar_of(&path) else { return (404, vec![], String::new()) };
                    match self.calendars[&cal].objects.get(&path) {
                        Some((etag, data)) => (200, vec![("ETag".into(), etag.clone())], data.clone()),
                        None => (404, vec![], String::new()),
                    }
                }
                "PUT" => {
                    let Some(cal) = self.calendar_of(&path) else { return (404, vec![], String::new()) };
                    if !self.calendars[&cal].writable {
                        return (403, vec![], String::new());
                    }
                    let current = self.calendars[&cal].objects.get(&path).map(|(e, _)| e.clone());
                    let fits = match (header("if-match"), header("if-none-match")) {
                        (Some(m), _) => current.as_deref() == Some(m.as_str()),
                        (None, Some(n)) if n == "*" => current.is_none(),
                        _ => true,
                    };
                    if !fits {
                        return (412, vec![], String::new());
                    }
                    self.writes.push(("PUT".into(), path.clone()));
                    let name = path[cal.len()..].to_string();
                    let etag = self.put(&cal, &name, body);
                    let etag = self.calendars[&cal].objects[&etag].0.clone();
                    (if current.is_some() { 204 } else { 201 }, vec![("ETag".into(), etag)], String::new())
                }
                "DELETE" => {
                    let Some(cal) = self.calendar_of(&path) else { return (404, vec![], String::new()) };
                    let current = self.calendars[&cal].objects.get(&path).map(|(e, _)| e.clone());
                    let Some(current) = current else { return (404, vec![], String::new()) };
                    if header("if-match").is_some_and(|m| m != current) {
                        return (412, vec![], String::new());
                    }
                    self.writes.push(("DELETE".into(), path.clone()));
                    self.remove(&path);
                    (204, vec![], String::new())
                }
                _ => (405, vec![], String::new()),
            }
        }
    }
}
