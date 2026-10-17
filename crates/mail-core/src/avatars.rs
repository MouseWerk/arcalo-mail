//! Profile pictures, in this order: the contact's own picture (an address book, Microsoft's
//! contacts), the sender's brand logo from BIMI, Gravatar when the user switched it on, else
//! the UI draws initials. Nothing is ever loaded from inside a mail.
//!
//! BIMI (Brand Indicators for Message Identification): the domain publishes `default._bimi`
//! (`v=BIMI1; l=https://…svg`) and enforces DMARC (`p=quarantine` or `reject` at 100 %); the
//! logo must be SVG Tiny Portable/Secure (`baseProfile="tiny-ps"`) and is rebuilt here from an
//! allow-list of elements and attributes before it is kept. It is fetched once per domain and
//! kept (`pictures`), as is the answer that there is none; both are asked again after a while.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::network::{self, Service};
use crate::store::{Store, now};

/// A picture is kept this long, the answer „none“ this long.
pub const KEEP_SECS: i64 = 30 * 86400;
pub const RETRY_SECS: i64 = 7 * 86400;
/// The largest logo taken (BIMI asks for 32 KB).
pub const MAX_SVG: usize = 32 * 1024;
/// The largest Gravatar taken.
pub const MAX_IMAGE: usize = 256 * 1024;

/// The type of an image by its first bytes.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        let head = String::from_utf8_lossy(&b[..b.len().min(512)]).to_ascii_lowercase();
        head.contains("<svg").then_some("image/svg+xml")
    }
}

/// A picture to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Picture {
    pub mime: String,
    pub data: Vec<u8>,
    /// `contact`, `bimi`, `gravatar`.
    pub source: &'static str,
}

/// What the store knows about an address's picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(Picture),
    /// Nothing kept yet: fetch from these (in order), then [`keep`].
    Fetch(Vec<Source>),
    /// None anywhere: initials.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Bimi(String),
    Gravatar(String),
}

impl Source {
    pub fn key(&self) -> String {
        match self {
            Source::Bimi(d) => format!("bimi:{d}"),
            Source::Gravatar(e) => format!("gravatar:{}", e.to_ascii_lowercase()),
        }
    }
}

/// A picture's type and bytes.
type Image = (String, Vec<u8>);

/// What is kept for a key: `Some(None)` is the kept answer „none“.
fn kept(store: &Store, key: &str, at: i64) -> Result<Option<Option<Image>>> {
    let row: Option<(Option<String>, Option<Vec<u8>>, i64)> = store
        .conn()
        .query_row("SELECT mime, data, fetched_at FROM pictures WHERE key = ?1", [key], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    Ok(match row {
        Some((Some(m), Some(d), t)) if at - t < KEEP_SECS => Some(Some((m, d))),
        Some((_, None, t)) if at - t < RETRY_SECS => Some(None),
        _ => None,
    })
}

/// The picture of an address as far as the store knows it. `bimi`, `gravatar`: the settings.
pub fn lookup(store: &Store, email: &str, bimi: bool, gravatar: bool) -> Result<Lookup> {
    let email = email.trim().to_ascii_lowercase();
    let card: Option<(Option<String>, Option<Vec<u8>>)> = store
        .conn()
        .query_row(
            "SELECT c.photo_type, c.photo FROM card_emails e JOIN cards c ON c.id = e.card_id
             WHERE e.email = ?1 AND c.photo IS NOT NULL LIMIT 1",
            [&email],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((Some(mime), Some(data))) = card {
        return Ok(Lookup::Found(Picture { mime, data, source: "contact" }));
    }
    let at = now();
    let mut fetch = vec![];
    let domain = crate::account::domain_of(&email);
    let sources: Vec<(Source, &'static str)> = [
        (bimi && domain.is_some()).then(|| (Source::Bimi(domain.clone().unwrap_or_default()), "bimi")),
        gravatar.then(|| (Source::Gravatar(email.clone()), "gravatar")),
    ]
    .into_iter()
    .flatten()
    .collect();
    for (s, name) in sources {
        match kept(store, &s.key(), at)? {
            Some(Some((mime, data))) => return Ok(Lookup::Found(Picture { mime, data, source: name })),
            Some(None) => {}
            None => fetch.push(s),
        }
    }
    Ok(if fetch.is_empty() { Lookup::None } else { Lookup::Fetch(fetch) })
}

/// Keeps what a fetch found (or that there is nothing).
pub fn keep(store: &Store, source: &Source, found: Option<&(String, Vec<u8>)>) -> Result<()> {
    store.conn().execute(
        "INSERT INTO pictures (key, mime, data, fetched_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (key) DO UPDATE SET mime = excluded.mime, data = excluded.data, fetched_at = excluded.fetched_at",
        params![source.key(), found.map(|f| f.0.as_str()), found.map(|f| f.1.as_slice()), now()],
    )?;
    Ok(())
}

/// Forgets every kept picture (Settings → Datenschutz, or Gravatar switched off).
pub fn forget(store: &Store, prefix: &str) -> Result<()> {
    store.conn().execute("DELETE FROM pictures WHERE key LIKE ?1", [format!("{prefix}%")])?;
    Ok(())
}

// ---------------------------------------------------------------------------- BIMI

/// The logo address of a BIMI record (`v=BIMI1; l=https://…; a=…`).
pub fn bimi_location(records: &[String]) -> Option<String> {
    let rec = records.iter().find(|r| r.trim_start().to_ascii_lowercase().starts_with("v=bimi1"))?;
    rec.split(';').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k.trim().eq_ignore_ascii_case("l") && v.trim().to_ascii_lowercase().starts_with("https://"))
            .then(|| v.trim().to_string())
    })
}

/// Whether a DMARC record enforces (`p=quarantine` or `p=reject`, at 100 %), as BIMI asks.
pub fn dmarc_enforced(records: &[String]) -> bool {
    let Some(rec) = records.iter().find(|r| r.trim_start().to_ascii_lowercase().starts_with("v=dmarc1")) else {
        return false;
    };
    let mut policy = false;
    let mut pct = 100;
    for part in rec.split(';') {
        let Some((k, v)) = part.split_once('=') else { continue };
        match k.trim().to_ascii_lowercase().as_str() {
            "p" => policy = matches!(v.trim().to_ascii_lowercase().as_str(), "quarantine" | "reject"),
            "pct" => pct = v.trim().parse().unwrap_or(100),
            _ => {}
        }
    }
    policy && pct == 100
}

/// SVG Tiny PS: the elements and attributes kept.
const SVG_ELEMENTS: &[&str] = &[
    "svg",
    "title",
    "desc",
    "g",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "defs",
    "linearGradient",
    "radialGradient",
    "stop",
    "use",
    "text",
    "tspan",
    "solidColor",
];
const SVG_ATTRIBUTES: &[&str] = &[
    "version",
    "baseProfile",
    "viewBox",
    "width",
    "height",
    "preserveAspectRatio",
    "id",
    "d",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "points",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "opacity",
    "transform",
    "offset",
    "stop-color",
    "stop-opacity",
    "gradientUnits",
    "gradientTransform",
    "font-family",
    "font-size",
    "font-weight",
    "text-anchor",
    "solid-color",
    "solid-opacity",
    "href",
];

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Checks a BIMI logo (SVG Tiny PS: `baseProfile="tiny-ps"`, a title, no scripts, no
/// animation, no references outside the file) and rebuilds it from the allow-list.
pub fn check_svg(bytes: &[u8]) -> Result<String> {
    let bad = |why: &str| Error::Invalid(format!("BIMI logo: {why}"));
    if bytes.len() > MAX_SVG {
        return Err(bad("too large"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| bad("not UTF-8"))?;
    if text.contains("<!DOCTYPE") || text.contains("<!ENTITY") {
        return Err(bad("DTD"));
    }
    let doc = roxmltree::Document::parse(text).map_err(|e| bad(&e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" || root.attribute("baseProfile") != Some("tiny-ps") {
        return Err(bad("not SVG Tiny PS"));
    }
    if !root.children().any(|c| c.has_tag_name("title") || c.tag_name().name() == "title") {
        return Err(bad("no title"));
    }
    fn walk(n: roxmltree::Node<'_, '_>, out: &mut String) -> std::result::Result<(), &'static str> {
        let name = n.tag_name().name();
        if !SVG_ELEMENTS.contains(&name) {
            return Err(if matches!(name, "script" | "foreignObject") {
                "script"
            } else if name.starts_with("animate") || name == "set" {
                "animation"
            } else {
                "element not allowed"
            });
        }
        out.push('<');
        out.push_str(name);
        if name == "svg" {
            out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
        }
        for a in n.attributes() {
            let an = a.name();
            if an.starts_with("on") {
                return Err("script");
            }
            if !SVG_ATTRIBUTES.contains(&an) {
                continue;
            }
            let v = a.value();
            if an == "href" && !v.starts_with('#') {
                return Err("external reference");
            }
            if v.to_ascii_lowercase().contains("url(") && !v.contains("url(#") {
                return Err("external reference");
            }
            out.push_str(&format!(" {an}=\"{}\"", esc(v)));
        }
        out.push('>');
        for c in n.children() {
            if c.is_element() {
                walk(c, out)?;
            } else if let Some(t) = c.text().filter(|_| c.is_text()) {
                out.push_str(&esc(t));
            }
        }
        out.push_str(&format!("</{name}>"));
        Ok(())
    }
    let mut out = String::new();
    walk(root, &mut out).map_err(bad)?;
    Ok(out)
}

/// The organizational domain for DMARC (the record of `mail.shop.example` may be on
/// `shop.example`), roughly: the last two labels.
fn org_domain(domain: &str) -> String {
    let labels: Vec<&str> = domain.split('.').collect();
    labels[labels.len().saturating_sub(2)..].join(".")
}

/// Fetches and checks a domain's BIMI logo; `txt` looks up TXT records (the network's, or a
/// test's). `None`: the domain has no (valid) logo.
pub async fn fetch_bimi<F, Fut>(domain: &str, txt: F) -> Result<Option<(String, Vec<u8>)>>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>>>,
{
    let Some(location) = bimi_location(&txt(format!("default._bimi.{domain}")).await?) else { return Ok(None) };
    let mut dmarc = txt(format!("_dmarc.{domain}")).await?;
    if dmarc.is_empty() && org_domain(domain) != domain {
        dmarc = txt(format!("_dmarc.{}", org_domain(domain))).await?;
    }
    if !dmarc_enforced(&dmarc) {
        return Ok(None);
    }
    let Some(bytes) = get_limited(&location, MAX_SVG).await? else { return Ok(None) };
    match check_svg(&bytes) {
        Ok(svg) => Ok(Some(("image/svg+xml".into(), svg.into_bytes()))),
        Err(e) => {
            crate::diag::log("pictures", &format!("{domain}: {e}"));
            Ok(None)
        }
    }
}

/// A BIMI logo through the system's resolver.
pub async fn fetch_bimi_dns(domain: &str) -> Result<Option<(String, Vec<u8>)>> {
    fetch_bimi(domain, |name| async move { network::txt(&name).await }).await
}

async fn get_limited(url: &str, max: usize) -> Result<Option<Vec<u8>>> {
    network::check_https_url(url)?;
    let client = network::client_for(Service::Pictures)?;
    let mut resp = client.get(url).send().await.map_err(|e| Error::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let mut out = vec![];
    while let Some(chunk) = resp.chunk().await.map_err(|e| Error::Network(e.to_string()))? {
        out.extend_from_slice(&chunk);
        if out.len() > max {
            return Ok(None);
        }
    }
    Ok(Some(out))
}

/// The Gravatar address of an e-mail address (SHA-256 of the trimmed, lower-cased address;
/// `d=404`: no picture is an answer, not a generated one).
pub fn gravatar_url(email: &str) -> String {
    let hash = Sha256::digest(email.trim().to_ascii_lowercase().as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("https://gravatar.com/avatar/{hex}?d=404&s=128")
}

/// Fetches a Gravatar (only when the user switched Gravatar on).
pub async fn fetch_gravatar(email: &str) -> Result<Option<(String, Vec<u8>)>> {
    let Some(bytes) = get_limited(&gravatar_url(email), MAX_IMAGE).await? else { return Ok(None) };
    Ok(sniff(&bytes).filter(|m| *m != "image/svg+xml").map(|m| (m.to_string(), bytes)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::network::tests::testhttp::{Reply, serve};

    const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" version="1.2" baseProfile="tiny-ps" viewBox="0 0 64 64"><title>Shop</title><rect width="64" height="64" fill="#1a73e8"/><path d="M10 10h44v44H10z" fill="#fff" onload="x()"/></svg>"##;

    #[test]
    fn records() {
        let bimi = ["v=BIMI1; l=https://shop.example/logo.svg; a=https://shop.example/vmc.pem".to_string()];
        assert_eq!(bimi_location(&bimi).as_deref(), Some("https://shop.example/logo.svg"));
        assert_eq!(bimi_location(&["v=BIMI1; l=http://x/logo.svg".into()]), None, "https only");
        assert!(dmarc_enforced(&["v=DMARC1; p=reject; rua=mailto:d@x".into()]));
        assert!(!dmarc_enforced(&["v=DMARC1; p=none".into()]));
        assert!(!dmarc_enforced(&["v=DMARC1; p=quarantine; pct=50".into()]));
        assert_eq!(org_domain("mail.shop.example"), "shop.example");
    }

    #[test]
    fn logos_are_svg_tiny_ps_and_rebuilt() {
        assert!(
            check_svg(LOGO.as_bytes()).unwrap_err().to_string().contains("script"),
            "event handlers count as script"
        );
        let clean = LOGO.replace(r#" onload="x()""#, "");
        let svg = check_svg(clean.as_bytes()).unwrap();
        assert!(
            svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.2\" baseProfile=\"tiny-ps\""),
            "{svg}"
        );
        assert!(svg.contains("<title>Shop</title>"));
        assert!(check_svg(clean.replace("tiny-ps", "full").as_bytes()).is_err());
        assert!(check_svg(clean.replace("<title>Shop</title>", "").as_bytes()).is_err());
        let ext = clean.replace("<rect", "<use href=\"https://evil.example/x.svg#a\"/><rect");
        assert!(check_svg(ext.as_bytes()).unwrap_err().to_string().contains("external"));
        let script = clean.replace("<rect", "<script>alert(1)</script><rect");
        assert!(check_svg(script.as_bytes()).is_err());
        assert!(check_svg(clean.replace("<rect", "<animate attributeName=\"x\"/><rect").as_bytes()).is_err());
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(clean.as_bytes()), Some("image/svg+xml"));
    }

    #[tokio::test]
    async fn fetches_a_logo_once_per_domain() {
        let clean = LOGO.replace(r#" onload="x()""#, "");
        let port = serve(Arc::new(move |_m: &str, path: &str, _h: &[(String, String)], _b: &[u8]| match path {
            "/logo.svg" => Reply {
                status: 200,
                content_type: "image/svg+xml".into(),
                body: clean.clone().into_bytes(),
                location: None,
                headers: vec![],
            },
            _ => Reply::status(404, ""),
        }))
        .await;
        let txt = move |name: String| async move {
            Ok(match name.as_str() {
                "default._bimi.shop.example" => vec![format!("v=BIMI1; l=https://127.0.0.1:{port}/logo.svg")],
                "_dmarc.shop.example" => vec!["v=DMARC1; p=reject".to_string()],
                "default._bimi.lax.example" => vec![format!("v=BIMI1; l=https://127.0.0.1:{port}/logo.svg")],
                "_dmarc.lax.example" => vec!["v=DMARC1; p=none".to_string()],
                _ => vec![],
            })
        };
        // The test server speaks plain HTTP; BIMI asks for https, which the record above has. The
        // fetch itself is checked against the test server through `get_limited` below.
        assert!(fetch_bimi("lax.example", txt).await.unwrap().is_none(), "DMARC not enforced");
        assert!(fetch_bimi("other.example", txt).await.unwrap().is_none(), "no record");
        let got = get_limited(&format!("http://127.0.0.1:{port}/logo.svg"), MAX_SVG).await.unwrap().unwrap();
        assert!(check_svg(&got).is_ok());
        assert!(get_limited(&format!("http://127.0.0.1:{port}/logo.svg"), 10).await.unwrap().is_none(), "too large");

        let (s, _, _) = crate::store::tests::store_with_inbox();
        let src = Source::Bimi("shop.example".into());
        assert_eq!(lookup_of(&s, "info@shop.example"), Lookup::Fetch(vec![src.clone()]));
        keep(&s, &src, Some(&("image/svg+xml".into(), got.clone()))).unwrap();
        let Lookup::Found(p) = lookup_of(&s, "news@shop.example") else { panic!("kept per domain") };
        assert_eq!(p.source, "bimi");
        keep(&s, &Source::Bimi("plain.example".into()), None).unwrap();
        assert_eq!(lookup_of(&s, "a@plain.example"), Lookup::None, "„none“ is kept too");
        assert_eq!(
            lookup(&s, "a@plain.example", true, true).unwrap(),
            Lookup::Fetch(vec![Source::Gravatar("a@plain.example".into())])
        );
        assert_eq!(lookup(&s, "a@plain.example", false, false).unwrap(), Lookup::None, "Gravatar is off by default");
    }

    fn lookup_of(s: &Store, email: &str) -> Lookup {
        lookup(s, email, true, false).unwrap()
    }

    #[test]
    fn contact_pictures_come_first() {
        let (s, _, _) = crate::store::tests::store_with_inbox();
        s.conn().execute("INSERT INTO address_books (kind, name, created_at) VALUES ('carddav', 'K', 0)", []).unwrap();
        s.conn()
            .execute("INSERT INTO cards (book_id, href, name, photo, photo_type) VALUES (1, '/a', 'Lea', x'89504e47', 'image/png')", [])
            .unwrap();
        s.conn().execute("INSERT INTO card_emails (email, card_id) VALUES ('lea@example.org', 1)", []).unwrap();
        let Lookup::Found(p) = lookup(&s, "Lea@Example.org", true, true).unwrap() else { panic!() };
        assert_eq!((p.source, p.mime.as_str()), ("contact", "image/png"));
        assert_eq!(gravatar_url(" Lea@Example.org "), gravatar_url("lea@example.org"));
        assert!(gravatar_url("x@y.z").contains("d=404"));
    }
}
