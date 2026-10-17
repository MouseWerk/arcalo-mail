//! Tracker protection: what senders put into mail to learn that and when it was read, and who
//! clicked which link. [`crate::sanitize`] calls this for every message it shows.
//!
//! - **Tracking pixels** (an image of one pixel, or any image from a known tracking host) are
//!   taken out of the HTML before anything else, also when the user loads remote images: they
//!   are never fetched.
//! - **Link trackers**: a link that only passes through a redirect with the real address in its
//!   query (`?url=https://…`) is replaced by that address, and campaign parameters (`utm_*`,
//!   `fbclid`, `mc_eid` …) are removed. Redirects without the address inside stay as they are;
//!   opening them is the user's click.
//!
//! Nothing here makes a request: the decisions come from the HTML and the addresses alone.

use url::Url;

/// Hosts that only serve open-tracking pixels and click redirects (a host or any of its
/// subdomains).
const TRACKER_HOSTS: &[&str] = &[
    "list-manage.com",
    "mailchimp.com",
    "mandrillapp.com",
    "sendgrid.net",
    "hubspotemail.net",
    "hubspotlinks.com",
    "hs-analytics.net",
    "google-analytics.com",
    "doubleclick.net",
    "mixpanel.com",
    "mailtrack.io",
    "bananatag.com",
    "yesware.com",
    "getnotify.com",
    "mailstat.us",
    "emltrk.com",
    "sparkpostmail.com",
    "exacttarget.com",
    "rs6.net",
    "klclick.com",
    "klclick1.com",
    "mktoresp.com",
    "pardot.com",
    "returnpath.net",
    "litmus.com",
    "streak.com",
    "mixmax.com",
    "superhuman.com",
    "cmail19.com",
    "cmail20.com",
    "createsend1.com",
    "intercom-mail.com",
    "customeriomail.com",
    "mailjet.com",
    "sendinblue.com",
    "brevo.com",
    "newsletter2go.com",
    "cleverreach.com",
    "rapidmail.de",
];

/// Paths of open-tracking images on hosts that also serve real pictures.
const PIXEL_PATHS: &[&str] = &[
    "/track/open",
    "/wf/open",
    "/open.php",
    "/open.aspx",
    "/tracking/open",
    "/trk/open",
    "/e/o/",
    "/beacon",
    "/pixel.gif",
    "/pixel.png",
    "/1x1.gif",
    "/1x1.png",
    "/spacer-open",
];

/// Query parameters that only say where a click came from.
const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "igshid",
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "__hstc",
    "__hssc",
    "__hsfp",
    "hsctatracking",
    "mkt_tok",
    "oly_anon_id",
    "oly_enc_id",
    "vero_conv",
    "vero_id",
    "wickedid",
    "_gl",
    "trk",
    "trkcampaign",
    "sc_campaign",
    "s_cid",
    "ref_src",
    "ml_subscriber",
    "ml_subscriber_hash",
];

/// Hosts that only forward to the address in their query.
const REDIRECT_HOSTS: &[&str] = &[
    "safelinks.protection.outlook.com",
    "l.facebook.com",
    "lm.facebook.com",
    "l.instagram.com",
    "out.reddit.com",
    "t.umblr.com",
    "href.li",
    "l.messenger.com",
];

/// The query keys a redirect keeps the real address in.
const TARGET_PARAMS: &[&str] =
    &["url", "u", "q", "target", "redirect", "redirect_url", "redirect_uri", "dest", "destination", "link", "to", "r"];

fn host_in(host: &str, list: &[&str]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    list.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// Whether an image is there to report the opening of the message: one pixel by its size
/// attributes or style, hidden, or from a tracking host or path.
pub fn is_tracking_image(src: &str, width: Option<&str>, height: Option<&str>, style: Option<&str>) -> bool {
    let src = &browser_url(src);
    let lower = src.to_ascii_lowercase();
    if !(lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("//")) {
        return false;
    }
    let tiny = |v: &str| {
        let v = v.trim().trim_end_matches("px").trim();
        v.parse::<f32>().is_ok_and(|n| n <= 1.0)
    };
    if width.is_some_and(tiny) && height.is_some_and(tiny) {
        return true;
    }
    if let Some(style) = style {
        let s = style.to_ascii_lowercase().replace(' ', "");
        let px =
            |prop: &str| s.split(';').any(|d| d.strip_prefix(prop).and_then(|v| v.strip_prefix(':')).is_some_and(tiny));
        if (px("width") && px("height")) || s.contains("display:none") || s.contains("visibility:hidden") {
            return true;
        }
    }
    let with_scheme = if lower.starts_with("//") { format!("https:{src}") } else { src.to_string() };
    let Ok(url) = Url::parse(&with_scheme) else { return false };
    let host = url.host_str().unwrap_or_default();
    if host_in(host, TRACKER_HOSTS) {
        return true;
    }
    let path = url.path().to_ascii_lowercase();
    PIXEL_PATHS.iter().any(|p| path.contains(p))
}

/// A link without its tracking: the address a redirect hides, without campaign parameters.
/// `None` when nothing changed.
pub fn clean_link(href: &str) -> Option<String> {
    let lower = href.trim_start().to_ascii_lowercase();
    if !(lower.starts_with("http:") || lower.starts_with("https:")) {
        return None;
    }
    let mut url = Url::parse(href.trim()).ok()?;
    let mut changed = false;
    // A redirect with the real address inside: that address (a few levels at most).
    for _ in 0..3 {
        let Some(inner) = redirect_target(&url) else { break };
        url = inner;
        changed = true;
    }
    let pairs: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let keep: Vec<&(String, String)> = pairs.iter().filter(|(k, _)| !is_tracking_param(k)).collect();
    if keep.len() != pairs.len() {
        changed = true;
        if keep.is_empty() {
            url.set_query(None);
        } else {
            let mut q = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in keep {
                q.append_pair(k, v);
            }
            url.set_query(Some(&q.finish()));
        }
    }
    changed.then(|| url.to_string())
}

fn is_tracking_param(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.starts_with("utm_") || TRACKING_PARAMS.contains(&k.as_str())
}

/// The address a click redirect forwards to, when it carries it in its query.
fn redirect_target(url: &Url) -> Option<Url> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = url.path().to_ascii_lowercase();
    let google =
        (host == "google.com" || host.ends_with(".google.com") || host.starts_with("google.")) && path == "/url";
    let redirector = google
        || host_in(&host, REDIRECT_HOSTS)
        || (host.ends_with("youtube.com") && path == "/redirect")
        || ["click", "track", "redirect", "redir", "/r/", "/out", "/ls/"].iter().any(|p| path.contains(p));
    if !redirector {
        return None;
    }
    url.query_pairs().find_map(|(k, v)| {
        if !TARGET_PARAMS.contains(&k.to_ascii_lowercase().as_str()) {
            return None;
        }
        let v = v.trim();
        let l = v.to_ascii_lowercase();
        if !(l.starts_with("https://") || l.starts_with("http://")) {
            return None;
        }
        Url::parse(v).ok().filter(|u| u.host_str().is_some())
    })
}

/// A URL as a browser reads it (WHATWG URL parsing): without the C0 controls and spaces around
/// it and without tabs and line breaks anywhere in it.
pub fn browser_url(value: &str) -> String {
    value.trim_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect()
}

/// The addresses of the tracking pixels in an HTML message, found on its `<img>` tags as the
/// HTML tokenizer reads them (attributes decoded, whatever the quoting): the sanitizer drops
/// every picture with one of these addresses.
pub fn pixel_sources(html: &str) -> std::collections::HashSet<String> {
    use html5ever::tendril::{SliceExt, StrTendril};
    use html5ever::tokenizer::{BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer};
    use std::cell::RefCell;
    use std::collections::HashSet;

    struct Pixels(RefCell<HashSet<String>>);
    impl TokenSink for Pixels {
        type Handle = ();
        fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
            if let Token::TagToken(tag) = token
                && tag.kind == TagKind::StartTag
                && &*tag.name == "img"
            {
                let attr = |name: &str| tag.attrs.iter().find(|a| &*a.name.local == name).map(|a| a.value.to_string());
                let src = browser_url(&attr("src").unwrap_or_default());
                let (w, h, style) = (attr("width"), attr("height"), attr("style"));
                if is_tracking_image(&src, w.as_deref(), h.as_deref(), style.as_deref()) {
                    self.0.borrow_mut().insert(src);
                }
            }
            TokenSinkResult::Continue
        }
    }
    let input = BufferQueue::default();
    let t: StrTendril = html.to_tendril();
    input.push_back(t);
    let tok = Tokenizer::new(Pixels(RefCell::new(HashSet::new())), Default::default());
    let _ = tok.feed(&input);
    tok.end();
    tok.sink.0.take()
}

/// The value of an attribute in the text of one tag (`<img src="…" width=1>`), as sent.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before = lower[..at].chars().last();
        if !before.is_some_and(|c| c.is_whitespace() || c == '/') {
            continue;
        }
        let rest = tag[at + name.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        return Some(match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or_default(),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or_default(),
        });
    }
    None
}

/// Takes the tracking pixels out of an HTML message by its text; returns the rest and how many
/// were removed. A quick look only: the sanitizer decides on the tags as parsed
/// ([`pixel_sources`]), which markup cannot fool.
pub fn strip_pixels(html: &str) -> (String, usize) {
    let lower = html.to_ascii_lowercase();
    if lower.len() != html.len() {
        // Lower case changed the byte offsets (rare letters); the sanitizer still blocks the
        // remote images unless the user allows them.
        return (html.to_string(), 0);
    }
    let mut out = String::with_capacity(html.len());
    let mut removed = 0;
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<img") {
        let start = pos + i;
        let Some(len) = html[start..].find('>') else { break };
        let end = start + len + 1;
        let tag = &html[start..end];
        let src = attr(tag, "src").unwrap_or_default();
        out.push_str(&html[pos..start]);
        if is_tracking_image(src, attr(tag, "width"), attr(tag, "height"), attr(tag, "style")) {
            removed += 1;
        } else {
            out.push_str(tag);
        }
        pos = end;
    }
    out.push_str(&html[pos..]);
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_are_recognized() {
        assert!(is_tracking_image("https://news.example.org/o.gif", Some("1"), Some("1"), None));
        assert!(is_tracking_image("https://news.example.org/o.gif", Some("0"), Some("0px"), None));
        assert!(is_tracking_image("https://x.example.org/a.png", None, None, Some("width: 1px; height:1px")));
        assert!(is_tracking_image("https://x.example.org/a.png", None, None, Some("display:none")));
        assert!(is_tracking_image("https://us1.list-manage.com/track/open.php?u=1", None, None, None));
        assert!(is_tracking_image("//u1.ct.sendgrid.net/wf/open?upn=x", None, None, None));
        assert!(is_tracking_image("https://shop.example.org/e/o/abc", None, None, None));
        assert!(!is_tracking_image("https://cdn.example.org/hero.jpg", Some("600"), Some("300"), None));
        assert!(!is_tracking_image("https://track.example.org/p.gif", Some("1"), None, None), "one size only");
        assert!(!is_tracking_image("cid:logo@x", Some("1"), Some("1"), None), "not remote");
    }

    #[test]
    fn pixels_leave_the_html() {
        let html = "<p>Hi</p><img src=\"https://cdn.example.org/a.jpg\" alt=\"A\">\
                    <IMG SRC='https://news.example.org/open.php?id=7' width=1 height=1><img src=x>";
        let (out, n) = strip_pixels(html);
        assert_eq!(n, 1);
        assert!(out.contains("cdn.example.org/a.jpg") && !out.contains("open.php"), "{out}");
        assert!(out.contains("<img src=x>"));
        assert_eq!(strip_pixels("<p>nothing</p>"), ("<p>nothing</p>".to_string(), 0));
    }

    #[test]
    fn links_lose_their_trackers() {
        assert_eq!(
            clean_link("https://example.org/article?id=4&utm_source=news&utm_medium=email&fbclid=abc").as_deref(),
            Some("https://example.org/article?id=4")
        );
        assert_eq!(clean_link("https://example.org/a?utm_campaign=x").as_deref(), Some("https://example.org/a"));
        assert_eq!(
            clean_link("https://www.google.com/url?q=https%3A%2F%2Fexample.org%2Fdoc%3Futm_source%3Dx&sa=D").as_deref(),
            Some("https://example.org/doc")
        );
        assert_eq!(
            clean_link(
                "https://eur01.safelinks.protection.outlook.com/?url=https%3A%2F%2Fexample.org%2Finvoice&data=1"
            )
            .as_deref(),
            Some("https://example.org/invoice")
        );
        assert_eq!(
            clean_link("https://links.shop.example/click?upn=1&url=https%3A%2F%2Fshop.example%2Fsale").as_deref(),
            Some("https://shop.example/sale")
        );
        assert_eq!(clean_link("https://example.org/a?page=2"), None, "nothing to remove");
        assert_eq!(
            clean_link("https://twitter.example/intent/tweet?url=https%3A%2F%2Fexample.org"),
            None,
            "a share link"
        );
        assert_eq!(clean_link("mailto:a@example.org?subject=utm_x"), None);
    }
}
