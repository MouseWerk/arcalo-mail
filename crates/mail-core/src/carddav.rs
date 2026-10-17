//! CardDAV address books (RFC 6352 on WebDAV): iCloud, Fastmail, mailbox.org, Nextcloud and any
//! other server. Discovery from the server's address (`/.well-known/carddav`, the principal,
//! its address book home, the address books), then the cards by their ETags: only new and
//! changed ones are fetched (`addressbook-multiget`), removed ones go. vCards 3 and 4 are read
//! for the name, addresses, phone numbers, organization and an embedded picture; a picture by
//! address is not fetched.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::network::{self, HttpClient, Service};
use crate::{tr, trf};

/// Servers people use, with the address to start from (`{host}`: the user's own server).
pub const PRESETS: &[(&str, &str)] = &[
    ("iCloud", "https://contacts.icloud.com"),
    ("Fastmail", "https://carddav.fastmail.com"),
    ("mailbox.org", "https://dav.mailbox.org"),
    ("Posteo", "https://posteo.de:8843"),
    ("Nextcloud", "https://{host}/remote.php/dav"),
];

/// One contact as a server keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub href: String,
    pub etag: Option<String>,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub org: Option<String>,
    pub photo: Option<(String, Vec<u8>)>,
}

/// Unfolds a vCard's lines (a line starting with a space or tab continues the one before).
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for line in text.replace("\r\n", "\n").split('\n') {
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = out.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

fn unescape(v: &str) -> String {
    v.replace("\\n", " ").replace("\\N", " ").replace("\\,", ",").replace("\\;", ";").replace("\\\\", "\\")
}

/// Reads a vCard (3.0 or 4.0).
pub fn parse_vcard(text: &str, href: &str, etag: Option<String>) -> Option<Card> {
    let mut card = Card { href: href.to_string(), etag, ..Default::default() };
    let mut n_name = String::new();
    let mut seen_begin = false;
    for line in unfold(text) {
        let Some((head, value)) = line.split_once(':') else { continue };
        let mut parts = head.split(';');
        let prop = parts.next().unwrap_or_default();
        // A group prefix: `item1.EMAIL`.
        let prop = prop.rsplit('.').next().unwrap_or(prop).to_ascii_uppercase();
        let params: Vec<String> = parts.map(|p| p.to_ascii_uppercase()).collect();
        match prop.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VCARD") => seen_begin = true,
            "FN" => card.name = unescape(value).trim().to_string(),
            "N" => {
                let f: Vec<&str> = value.split(';').collect();
                n_name = [f.get(1).copied().unwrap_or_default(), f.first().copied().unwrap_or_default()]
                    .iter()
                    .map(|s| unescape(s).trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            "EMAIL" => {
                let v = value.trim().trim_start_matches("mailto:").to_string();
                if v.contains('@') && !card.emails.iter().any(|e| e.eq_ignore_ascii_case(&v)) {
                    card.emails.push(v);
                }
            }
            "TEL" => {
                let v = value.trim().trim_start_matches("tel:").to_string();
                if !v.is_empty() {
                    card.phones.push(v);
                }
            }
            "ORG" => {
                let v = unescape(value.split(';').next().unwrap_or_default()).trim().to_string();
                if !v.is_empty() {
                    card.org = Some(v);
                }
            }
            "PHOTO" | "LOGO" if card.photo.is_none() || prop == "PHOTO" => {
                let photo = if let Some(rest) = value.strip_prefix("data:") {
                    // vCard 4: `data:image/jpeg;base64,…`
                    let (meta, data) = rest.split_once(',').unwrap_or_default();
                    let mime = meta.split(';').next().unwrap_or("image/jpeg").to_string();
                    base64::engine::general_purpose::STANDARD.decode(data.trim()).ok().map(|d| (mime, d))
                } else if params.iter().any(|p| p == "ENCODING=B" || p == "ENCODING=BASE64") {
                    let kind =
                        params.iter().find_map(|p| p.strip_prefix("TYPE=")).unwrap_or("JPEG").to_ascii_lowercase();
                    let mime = if kind.contains('/') { kind } else { format!("image/{kind}") };
                    base64::engine::general_purpose::STANDARD.decode(value.trim()).ok().map(|d| (mime, d))
                } else {
                    // A picture by address: not fetched.
                    None
                };
                if let Some((mime, data)) = photo
                    && let Some(real) = crate::avatars::sniff(&data)
                    && real != "image/svg+xml"
                {
                    let _ = mime;
                    card.photo = Some((real.to_string(), data));
                }
            }
            _ => {}
        }
    }
    if !seen_begin {
        return None;
    }
    if card.name.is_empty() {
        card.name = if n_name.is_empty() { card.emails.first().cloned().unwrap_or_default() } else { n_name };
    }
    Some(card)
}

// ---------------------------------------------------------------------------- WebDAV

const DAV: &str = "DAV:";
const CARD: &str = "urn:ietf:params:xml:ns:carddav";

/// A server and the user's sign-in.
pub struct Dav {
    http: HttpClient,
    user: String,
    password: String,
}

/// An address book collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Book {
    pub url: String,
    pub name: String,
}

fn method(name: &str) -> reqwest::Method {
    reqwest::Method::from_bytes(name.as_bytes()).unwrap_or(reqwest::Method::GET)
}

/// `href` against `base` (servers answer with paths).
fn join(base: &str, href: &str) -> String {
    match url::Url::parse(base).and_then(|b| b.join(href.trim())) {
        Ok(u) => u.to_string(),
        Err(_) => href.to_string(),
    }
}

/// The `<response>` elements of a multistatus: href and the found properties' elements.
/// A found property: its name, its text, and the names or hrefs inside it.
type Prop = (String, String, String);

fn responses(xml: &str) -> Result<Vec<(String, Vec<Prop>)>> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Network(format!("CardDAV: {e}")))?;
    let mut out = vec![];
    for r in doc.descendants().filter(|n| n.has_tag_name((DAV, "response"))) {
        let href = r
            .children()
            .find(|n| n.has_tag_name((DAV, "href")))
            .and_then(|n| n.text())
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut props = vec![];
        for ps in r.children().filter(|n| n.has_tag_name((DAV, "propstat"))) {
            let ok = ps
                .children()
                .find(|n| n.has_tag_name((DAV, "status")))
                .and_then(|n| n.text())
                .is_none_or(|s| s.contains(" 200"));
            if !ok {
                continue;
            }
            for prop in ps.children().filter(|n| n.has_tag_name((DAV, "prop"))) {
                for p in prop.children().filter(|n| n.is_element()) {
                    let name = p.tag_name().name().to_string();
                    let text = p.text().unwrap_or_default().trim().to_string();
                    // Nested: <current-user-principal><href>…; <resourcetype><addressbook/>.
                    let inner = p
                        .children()
                        .filter(|c| c.is_element())
                        .map(|c| {
                            if c.has_tag_name((DAV, "href")) {
                                c.text().unwrap_or_default().trim().to_string()
                            } else {
                                c.tag_name().name().to_string()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    props.push((name, text, inner));
                }
            }
        }
        out.push((href, props));
    }
    Ok(out)
}

fn prop<'a>(props: &'a [Prop], name: &str) -> Option<&'a Prop> {
    props.iter().find(|(n, _, _)| n == name)
}

impl Dav {
    pub fn new(user: &str, password: &str) -> Result<Dav> {
        Ok(Dav { http: network::client_for(Service::Contacts)?, user: user.into(), password: password.into() })
    }

    async fn request(&self, m: &str, url: &str, depth: Option<&str>, body: Option<String>) -> Result<(u16, String)> {
        network::check_https_url(url)?;
        let mut req = self.http.request(method(m), url).basic_auth(&self.user, Some(&self.password));
        if let Some(d) = depth {
            req = req.header("Depth", d);
        }
        if let Some(b) = body {
            req = req.header(reqwest::header::CONTENT_TYPE, "application/xml; charset=utf-8").body(b);
        }
        let resp = req.send().await.map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| Error::Network(e.to_string()))?;
        if status == 401 || status == 403 {
            return Err(Error::Connection(network::Problem::Auth {
                detail: tr!("Benutzername oder App-Passwort stimmt nicht", "user name or app password is wrong").into(),
            }));
        }
        Ok((status, text))
    }

    async fn propfind(
        &self,
        url: &str,
        depth: &str,
        props: &str,
    ) -> Result<Vec<(String, Vec<(String, String, String)>)>> {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"{CARD}\"><d:prop>{props}</d:prop></d:propfind>"
        );
        let (status, text) = self.request("PROPFIND", url, Some(depth), Some(body)).await?;
        if status != 207 {
            return Err(Error::Network(trf!(
                "Der Adressbuch-Server antwortet nicht wie erwartet ({status}).",
                "The address book server does not answer as expected ({status})."
            )));
        }
        responses(&text)
    }

    /// The address books of the user, from the server's address.
    pub async fn discover(&self, start: &str) -> Result<Vec<Book>> {
        let start = start.trim().trim_end_matches('/').to_string();
        let mut principal = None;
        let origin =
            url::Url::parse(&start).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| start.clone());
        for url in [format!("{start}/"), format!("{origin}/.well-known/carddav")] {
            if let Ok(list) = self.propfind(&url, "0", "<d:current-user-principal/>").await
                && let Some(p) =
                    list.iter().find_map(|(_, props)| prop(props, "current-user-principal").map(|(_, _, h)| h.clone()))
                && !p.is_empty()
            {
                principal = Some(join(&url, &p));
                break;
            }
        }
        let principal = principal.ok_or_else(|| {
            Error::Network(
                tr!(
                    "Unter dieser Adresse ist kein Adressbuch zu finden.",
                    "No address book is to be found at this address."
                )
                .into(),
            )
        })?;
        let home = self
            .propfind(&principal, "0", "<c:addressbook-home-set/>")
            .await?
            .iter()
            .find_map(|(_, props)| {
                prop(props, "addressbook-home-set").map(|(_, t, h)| if h.is_empty() { t.clone() } else { h.clone() })
            })
            .filter(|h| !h.is_empty())
            .map(|h| join(&principal, &h))
            .ok_or_else(|| {
                Error::Network(tr!("Der Server nennt kein Adressbuch.", "The server names no address book.").into())
            })?;
        let list = self.propfind(&home, "1", "<d:resourcetype/><d:displayname/>").await?;
        let mut books = vec![];
        for (href, props) in list {
            if prop(&props, "resourcetype").is_some_and(|(_, _, inner)| inner.split(' ').any(|t| t == "addressbook")) {
                let name = prop(&props, "displayname")
                    .map(|(_, t, _)| t.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| tr!("Kontakte", "Contacts").into());
                books.push(Book { url: join(&home, &href), name });
            }
        }
        if books.is_empty() {
            return Err(Error::Network(
                tr!("Der Server hat kein Adressbuch.", "The server has no address book.").into(),
            ));
        }
        Ok(books)
    }

    /// The cards' ETags of a book.
    pub async fn etags(&self, book: &str) -> Result<Vec<(String, String)>> {
        let list = self.propfind(book, "1", "<d:getetag/><d:resourcetype/>").await?;
        Ok(list
            .into_iter()
            .filter(|(_, props)| !prop(props, "resourcetype").is_some_and(|(_, _, i)| i.contains("collection")))
            .filter_map(|(href, props)| prop(&props, "getetag").map(|(_, e, _)| (join(book, &href), e.clone())))
            .collect())
    }

    /// The cards at `hrefs` (absolute), fetched together.
    pub async fn multiget(&self, book: &str, hrefs: &[String]) -> Result<Vec<Card>> {
        let mut out = vec![];
        for batch in hrefs.chunks(50) {
            let refs: String = batch
                .iter()
                .map(|h| {
                    let path = url::Url::parse(h).map(|u| u.path().to_string()).unwrap_or_else(|_| h.clone());
                    format!("<d:href>{}</d:href>", path.replace('&', "&amp;").replace('<', "&lt;"))
                })
                .collect();
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?><c:addressbook-multiget xmlns:d=\"DAV:\" xmlns:c=\"{CARD}\"><d:prop><d:getetag/><c:address-data/></d:prop>{refs}</c:addressbook-multiget>"
            );
            let (status, text) = self.request("REPORT", book, Some("1"), Some(body)).await?;
            if status != 207 {
                return Err(Error::Network(trf!(
                    "Der Adressbuch-Server meldet {status}.",
                    "The address book server reports {status}."
                )));
            }
            for (href, props) in responses(&text)? {
                let etag = prop(&props, "getetag").map(|(_, e, _)| e.clone());
                if let Some((_, data, _)) = prop(&props, "address-data")
                    && let Some(c) = parse_vcard(data, &join(book, &href), etag)
                {
                    out.push(c);
                }
            }
        }
        Ok(out)
    }
}

/// What a sync found: cards new or changed, and the hrefs that are gone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub changed: Vec<Card>,
    pub removed: Vec<String>,
}

/// Compares the server's ETags with the stored ones (`known`: href → ETag) and fetches what is
/// new or changed in every book.
pub async fn sync(dav: &Dav, books: &[Book], known: &[(String, Option<String>)]) -> Result<Changes> {
    let mut changes = Changes::default();
    let mut on_server: Vec<String> = vec![];
    for b in books {
        let tags = dav.etags(&b.url).await?;
        let wanted: Vec<String> = tags
            .iter()
            .filter(|(h, e)| !known.iter().any(|(kh, ke)| kh == h && ke.as_deref() == Some(e.as_str())))
            .map(|(h, _)| h.clone())
            .collect();
        on_server.extend(tags.into_iter().map(|(h, _)| h));
        if !wanted.is_empty() {
            changes.changed.extend(dav.multiget(&b.url, &wanted).await?);
        }
    }
    changes.removed = known.iter().map(|(h, _)| h.clone()).filter(|h| !on_server.contains(h)).collect();
    Ok(changes)
}

#[cfg(any(test, feature = "fake"))]
pub mod fake {
    //! A CardDAV server's answers, without sockets (the core's tests and the test server).

    use std::collections::BTreeMap;

    #[derive(Debug, Clone)]
    pub struct FakeCardDav {
        pub user: String,
        pub password: String,
        /// href (a path below `/addressbooks/user/contacts/`) → (ETag, vCard).
        pub cards: BTreeMap<String, (String, String)>,
    }

    impl FakeCardDav {
        pub fn new(user: &str, password: &str) -> Self {
            FakeCardDav { user: user.into(), password: password.into(), cards: BTreeMap::new() }
        }

        pub fn put(&mut self, name: &str, vcard: &str) {
            let n = self.cards.len() + 1;
            self.cards.insert(
                format!("/dav/addressbooks/user/contacts/{name}.vcf"),
                (format!("\"e{n}-{}\"", vcard.len()), vcard.into()),
            );
        }

        fn multistatus(inner: &str) -> (u16, String) {
            (
                207,
                format!(
                    "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\">{inner}</d:multistatus>"
                ),
            )
        }

        /// Answers a request; `auth`: the `Authorization` header.
        pub fn handle(&self, method: &str, path: &str, auth: Option<&str>, body: &str) -> (u16, String) {
            use base64::Engine;
            let expected = format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", self.user, self.password))
            );
            if auth != Some(expected.as_str()) {
                return (401, String::new());
            }
            let ok = |props: &str| {
                format!("<d:propstat><d:prop>{props}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>")
            };
            let path = path.split('?').next().unwrap_or_default();
            match (method, path.trim_end_matches('/')) {
                ("PROPFIND", "/dav/.well-known/carddav") | ("PROPFIND", "/dav") => Self::multistatus(&format!(
                    "<d:response><d:href>{path}</d:href>{}</d:response>",
                    ok("<d:current-user-principal><d:href>/dav/principals/user/</d:href></d:current-user-principal>")
                )),
                ("PROPFIND", "/dav/principals/user") => Self::multistatus(&format!(
                    "<d:response><d:href>/dav/principals/user/</d:href>{}</d:response>",
                    ok("<c:addressbook-home-set><d:href>/dav/addressbooks/user/</d:href></c:addressbook-home-set>")
                )),
                ("PROPFIND", "/dav/addressbooks/user") => Self::multistatus(&format!(
                    "<d:response><d:href>/dav/addressbooks/user/</d:href>{}</d:response><d:response><d:href>/dav/addressbooks/user/contacts/</d:href>{}</d:response>",
                    ok("<d:resourcetype><d:collection/></d:resourcetype>"),
                    ok(
                        "<d:resourcetype><d:collection/><c:addressbook/></d:resourcetype><d:displayname>Kontakte</d:displayname>"
                    )
                )),
                ("PROPFIND", "/dav/addressbooks/user/contacts") => {
                    let mut inner = format!(
                        "<d:response><d:href>/dav/addressbooks/user/contacts/</d:href>{}</d:response>",
                        ok("<d:resourcetype><d:collection/><c:addressbook/></d:resourcetype>")
                    );
                    for (href, (etag, _)) in &self.cards {
                        inner.push_str(&format!(
                            "<d:response><d:href>{href}</d:href>{}</d:response>",
                            ok(&format!("<d:getetag>{etag}</d:getetag><d:resourcetype/>"))
                        ));
                    }
                    Self::multistatus(&inner)
                }
                ("REPORT", "/dav/addressbooks/user/contacts") => {
                    let mut inner = String::new();
                    for (href, (etag, card)) in &self.cards {
                        if body.contains(&format!("<d:href>{href}</d:href>")) {
                            let data = card.replace('&', "&amp;").replace('<', "&lt;");
                            inner.push_str(&format!(
                                "<d:response><d:href>{href}</d:href>{}</d:response>",
                                ok(&format!("<d:getetag>{etag}</d:getetag><c:address-data>{data}</c:address-data>"))
                            ));
                        }
                    }
                    Self::multistatus(&inner)
                }
                _ => (404, String::new()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::fake::FakeCardDav;
    use super::*;
    use crate::network::tests::testhttp::{Reply, serve};

    const LEA: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Lea Weber\r\nN:Weber;Lea;;;\r\nitem1.EMAIL;TYPE=INTERNET:lea@example.org\r\nEMAIL;TYPE=WORK:lea.weber@\r\n firma.example\r\nTEL;TYPE=CELL:+49 170 1234567\r\nORG:Weber GmbH;Vertrieb\r\nPHOTO;ENCODING=b;TYPE=PNG:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGBgAAAABQABpfZFQAAAAABJRU5ErkJggg==\r\nEND:VCARD\r\n";

    #[test]
    fn reads_vcards_3_and_4() {
        let c = parse_vcard(LEA, "/a.vcf", Some("e1".into())).unwrap();
        assert_eq!(c.name, "Lea Weber");
        assert_eq!(c.emails, ["lea@example.org", "lea.weber@firma.example"], "folded lines and groups");
        assert_eq!(c.phones, ["+49 170 1234567"]);
        assert_eq!(c.org.as_deref(), Some("Weber GmbH"));
        assert_eq!(c.photo.as_ref().map(|p| p.0.as_str()), Some("image/png"));
        let v4 = "BEGIN:VCARD\nVERSION:4.0\nN:Berger;Jonas;;;\nEMAIL:mailto:jonas@example.org\nPHOTO:https://example.org/j.jpg\nEND:VCARD\n";
        let j = parse_vcard(v4, "/j.vcf", None).unwrap();
        assert_eq!((j.name.as_str(), j.emails[0].as_str()), ("Jonas Berger", "jonas@example.org"));
        assert!(j.photo.is_none(), "pictures by address are not fetched");
        assert!(parse_vcard("not a card", "/x", None).is_none());
    }

    #[tokio::test]
    async fn discovers_and_syncs_only_what_changed() {
        let fake = Arc::new(Mutex::new(FakeCardDav::new("maria", "app-pw")));
        fake.lock().unwrap().put("lea", LEA);
        fake.lock().unwrap().put("jonas", "BEGIN:VCARD\nVERSION:4.0\nFN:Jonas\nEMAIL:jonas@example.org\nEND:VCARD\n");
        let f = fake.clone();
        let port = serve(Arc::new(move |method: &str, path: &str, headers: &[(String, String)], body: &[u8]| {
            let auth = headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.as_str());
            let (status, text) = f.lock().unwrap().handle(method, path, auth, &String::from_utf8_lossy(body));
            Reply { status, content_type: "application/xml".into(), body: text.into_bytes(), location: None }
        }))
        .await;
        let base = format!("http://127.0.0.1:{port}/dav");
        let wrong = Dav::new("maria", "falsch").unwrap();
        assert!(matches!(wrong.discover(&base).await, Err(Error::Network(_)) | Err(Error::Connection(_))));
        let dav = Dav::new("maria", "app-pw").unwrap();
        let books = dav.discover(&base).await.unwrap();
        assert_eq!(
            books,
            [Book { url: format!("http://127.0.0.1:{port}/dav/addressbooks/user/contacts/"), name: "Kontakte".into() }]
        );
        let first = sync(&dav, &books, &[]).await.unwrap();
        assert_eq!(first.changed.len(), 2);
        let known: Vec<(String, Option<String>)> =
            first.changed.iter().map(|c| (c.href.clone(), c.etag.clone())).collect();
        assert!(sync(&dav, &books, &known).await.unwrap().changed.is_empty(), "nothing changed");
        {
            let mut f = fake.lock().unwrap();
            let jonas = f.cards.keys().find(|k| k.contains("jonas")).cloned().unwrap();
            f.cards.remove(&jonas);
            f.put("tom", "BEGIN:VCARD\nVERSION:3.0\nFN:Tom\nEMAIL:tom@example.org\nEND:VCARD\n");
        }
        let next = sync(&dav, &books, &known).await.unwrap();
        assert_eq!(next.changed.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Tom"]);
        assert_eq!(next.removed.len(), 1);
        assert!(next.removed[0].ends_with("jonas.vcf"));
    }
}
