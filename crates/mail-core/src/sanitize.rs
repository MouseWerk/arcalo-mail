//! HTML mail, cleaned before it is shown.
//!
//! Two layers keep a message from running code or calling home. This one removes everything
//! outside an allow-list (scripts, forms, frames, event handlers, `<style>` and `<link>`, CSS
//! with `url()`), and removes remote images unless the user allowed remote content for the
//! message or the sender; [`Sanitized::blocked_remote`] says how many were removed, for the
//! „Externe Inhalte laden“ bar. The reading pane then shows the result in an `<iframe sandbox>`
//! without scripts and with its own Content Security Policy (docs/ARCHITECTURE.md).
//!
//! Tracking pixels and link trackers go before that, whatever the user allowed
//! ([`crate::tracking`]); [`Sanitized::trackers`] counts them for the „Tracker entfernt“ note.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sanitized {
    pub html: String,
    /// Remote images and other resources that were removed.
    pub blocked_remote: usize,
    /// Tracking pixels and link trackers that were removed.
    pub trackers: usize,
}

/// CSS properties kept in `style` attributes: layout and type, nothing that loads a resource.
const STYLE_PROPERTIES: &[&str] = &[
    "color",
    "background-color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "text-align",
    "text-decoration",
    "text-transform",
    "line-height",
    "letter-spacing",
    "white-space",
    "vertical-align",
    "display",
    "width",
    "max-width",
    "min-width",
    "height",
    "max-height",
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "border",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-color",
    "border-style",
    "border-width",
    "border-collapse",
    "border-spacing",
    "border-radius",
    "direction",
];

/// A `style` attribute with only the allowed properties, nothing that loads a resource. The
/// `background` shorthand is kept as `background-color` when it is only a color (buttons in
/// newsletters are often drawn with it; without it their white text would vanish).
fn clean_style(style: &str) -> String {
    let mut out = String::new();
    for decl in style.split(';') {
        let Some((prop, value)) = decl.split_once(':') else { continue };
        let prop = prop.trim().to_ascii_lowercase();
        let value = value.trim();
        let lower = value.to_ascii_lowercase();
        if value.is_empty()
            || ["url(", "image(", "image-set(", "expression(", "gradient(", "\\", "@import"]
                .iter()
                .any(|b| lower.contains(b))
        {
            continue;
        }
        let prop = match prop.as_str() {
            "background" if !value.contains(char::is_whitespace) || lower.ends_with("!important") => "background-color",
            p if STYLE_PROPERTIES.contains(&p) => p,
            _ => continue,
        };
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(prop);
        out.push_str(": ");
        out.push_str(value);
    }
    out
}

/// A picture's address is the message's own (`cid:`) or inside it (`data:`); anything else
/// would be fetched from somewhere (or is nothing a picture can show).
fn is_own_picture(url: &str) -> bool {
    let u = crate::tracking::browser_url(url).to_ascii_lowercase();
    u.starts_with("cid:") || u.starts_with("data:image/")
}

/// The type of an inline picture as it may go into a `data:` address: `image/` and a plain
/// subtype, nothing else.
fn picture_type(mime: &str) -> Option<String> {
    let m = mime.trim().to_ascii_lowercase();
    let sub = m.strip_prefix("image/")?;
    (!sub.is_empty() && sub.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))).then_some(m)
}

/// The `data:` address of an inline picture, `None` for a part that is no picture.
fn data_uri(mime: &str, data: &[u8]) -> Option<String> {
    use base64::Engine;
    Some(format!("data:{};base64,{}", picture_type(mime)?, base64::engine::general_purpose::STANDARD.encode(data)))
}

/// Cleans the HTML of a message and removes its trackers. `allow_remote`: keep remote images
/// (the user chose to load them).
pub fn sanitize_html(html: &str, allow_remote: bool) -> Sanitized {
    sanitize_html_with(html, allow_remote, true)
}

/// [`sanitize_html`]; `trackers: false` leaves links and pictures as they are (the user's own
/// message in the compose sheet).
pub fn sanitize_html_with(html: &str, allow_remote: bool, trackers: bool) -> Sanitized {
    sanitize(html, allow_remote, trackers, &[])
}

/// [`sanitize_html`] with the message's inline pictures (Content-ID, type, bytes): a `cid:`
/// picture becomes a `data:` address while it is cleaned, so its type goes through the same
/// escaping as any attribute, and only a picture type is used.
pub fn sanitize_html_inline(html: &str, allow_remote: bool, images: &[(String, String, Vec<u8>)]) -> Sanitized {
    sanitize(html, allow_remote, true, images)
}

fn sanitize(html: &str, allow_remote: bool, trackers: bool, images: &[(String, String, Vec<u8>)]) -> Sanitized {
    // Pictures are decided on as the browser reads them: the tracking pixels by their tags as
    // parsed, any other picture from outside the message as remote.
    let pixels = if trackers { crate::tracking::pixel_sources(html) } else { HashSet::new() };
    let pixel_count = Arc::new(AtomicUsize::new(0));
    let pixel_counter = pixel_count.clone();
    let inline: Vec<(String, String)> =
        images.iter().filter_map(|(cid, mime, data)| Some((format!("cid:{cid}"), data_uri(mime, data)?))).collect();
    let blocked = Arc::new(AtomicUsize::new(0));
    let counter = blocked.clone();
    let links = Arc::new(AtomicUsize::new(0));
    let link_counter = links.clone();
    let mut b = ammonia::Builder::default();
    b.url_schemes(HashSet::from(["http", "https", "mailto", "tel", "cid", "data"]))
        .link_rel(Some("noopener noreferrer"))
        .add_generic_attributes(["style", "align", "valign", "bgcolor", "dir", "lang"])
        .add_tag_attributes("table", ["width", "border", "cellpadding", "cellspacing"])
        .add_tag_attributes("td", ["width", "height", "colspan", "rowspan"])
        .add_tag_attributes("th", ["width", "height", "colspan", "rowspan"])
        .add_tag_attributes("img", ["width", "height", "alt", "title"])
        .add_tag_attributes("font", ["color", "face", "size"])
        .add_tags(["font", "center"])
        .attribute_filter(move |element, attribute, value| match (element, attribute) {
            (_, "style") => Some(Cow::Owned(clean_style(value))),
            // `data:` only for pictures; a link must not open a document made by the sender.
            (_, "href") if value.trim_start().to_ascii_lowercase().starts_with("data:") => None,
            ("a", "href") if trackers => match crate::tracking::clean_link(value) {
                Some(clean) => {
                    link_counter.fetch_add(1, Ordering::Relaxed);
                    Some(Cow::Owned(clean))
                }
                None => Some(Cow::Borrowed(value)),
            },
            ("img", "src") => {
                let url = crate::tracking::browser_url(value);
                if let Some((_, uri)) = inline.iter().find(|(cid, _)| *cid == url) {
                    return Some(Cow::Owned(uri.clone()));
                }
                if is_own_picture(&url) {
                    return Some(Cow::Owned(url));
                }
                if pixels.contains(&url) || (trackers && crate::tracking::is_tracking_image(&url, None, None, None)) {
                    pixel_counter.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
                let lower = url.to_ascii_lowercase();
                let fetched = lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("//");
                if !fetched {
                    // Nothing a picture can show (a relative or unknown address).
                    None
                } else if allow_remote {
                    Some(Cow::Owned(url))
                } else {
                    counter.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
            _ => Some(Cow::Borrowed(value)),
        });
    let html = b.clean(html).to_string();
    Sanitized {
        html,
        blocked_remote: blocked.load(Ordering::Relaxed),
        trackers: pixel_count.load(Ordering::Relaxed) + links.load(Ordering::Relaxed),
    }
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\r' => {}
            c => out.push(c),
        }
    }
}

/// One line of text with its web addresses as links, their trackers removed (counted in
/// `trackers`).
fn linkify(line: &str, out: &mut String, trackers: &mut usize) {
    let mut rest = line;
    while let Some(i) = ["https://", "http://"].iter().filter_map(|p| rest.find(p)).min() {
        escape(&rest[..i], out);
        let url_len =
            rest[i..].find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"')).unwrap_or(rest.len() - i);
        let mut url = &rest[i..i + url_len];
        // A sentence's full stop or closing bracket is not part of the address.
        while url.ends_with(['.', ',', ';', ':', ')', '!', '?']) {
            url = &url[..url.len() - 1];
        }
        let clean = crate::tracking::clean_link(url);
        *trackers += usize::from(clean.is_some());
        let shown = clean.as_deref().unwrap_or(url);
        out.push_str("<a href=\"");
        escape(shown, out);
        out.push_str("\" rel=\"noopener noreferrer\">");
        escape(shown, out);
        out.push_str("</a>");
        rest = &rest[i + url.len()..];
    }
    escape(rest, out);
}

/// The text of a message as HTML for the same frame: escaped, line breaks kept, web addresses
/// as links, quoted lines (`> `) as quotes.
pub fn text_to_html(text: &str) -> String {
    text_to_html_counted(text).0
}

/// [`text_to_html`] and the number of links whose trackers were removed.
pub fn text_to_html_counted(text: &str) -> (String, usize) {
    let mut trackers = 0;
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str("<div class=\"plain\">");
    let mut depth = 0usize;
    for line in text.replace("\r\n", "\n").split('\n') {
        let level = line.chars().take_while(|c| *c == '>' || *c == ' ').filter(|c| *c == '>').count();
        while depth < level {
            out.push_str("<blockquote>");
            depth += 1;
        }
        while depth > level {
            out.push_str("</blockquote>");
            depth -= 1;
        }
        let mut content = line;
        for _ in 0..level {
            content = content.trim_start().strip_prefix('>').unwrap_or(content);
        }
        let content = if level > 0 { content.strip_prefix(' ').unwrap_or(content) } else { content };
        linkify(content, &mut out, &mut trackers);
        out.push_str("<br>");
    }
    while depth > 0 {
        out.push_str("</blockquote>");
        depth -= 1;
    }
    out.push_str("</div>");
    (out, trackers)
}

/// Where the quoted part of an HTML reply starts (the earlier message below the answer), so
/// the reading pane can fold it away. `None` when there is none, or nothing above it.
pub fn quote_start_html(html: &str) -> Option<usize> {
    let lower = html.to_lowercase();
    if lower.len() != html.len() {
        // Lower case changed the byte offsets (rare letters); no folding then.
        return None;
    }
    let markers = [
        "<blockquote type=\"cite\"",
        "class=\"gmail_quote",
        "id=\"divrplyfwdmsg\"",
        "class=\"moz-cite-prefix\"",
        "id=\"appendonsend\"",
        "class=\"yahoo_quoted\"",
        "-----original message-----",
        "-------- ursprüngliche nachricht --------",
        "-------- original message --------",
    ];
    let at = markers.iter().filter_map(|m| lower.find(m)).min()?;
    // From the start of the tag that holds the marker.
    let start = lower[..at].rfind('<').filter(|i| !lower[*i..at].contains('>')).unwrap_or(at);
    let before = &html[..start];
    let text = ammonia::Builder::empty().clean(before).to_string();
    text.trim().chars().any(char::is_alphanumeric).then_some(start)
}

/// Where the quoted end of a text reply starts: the attribution line („Am … schrieb …:“,
/// „On … wrote:“) or the first of the `>` lines that run to the end.
pub fn quote_start_text(text: &str) -> Option<usize> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = lines.len();
    let mut quoted = 0;
    while i > 0 {
        let l = lines[i - 1].trim();
        if l.starts_with('>') {
            quoted += 1;
        } else if !l.is_empty() {
            break;
        }
        i -= 1;
    }
    if quoted < 2 {
        return None;
    }
    if i > 0 && lines[i - 1].trim_end().ends_with(':') {
        i -= 1;
    }
    let before: usize = lines[..i].iter().map(|l| l.len() + 1).sum();
    lines[..i].iter().any(|l| l.chars().any(char::is_alphanumeric)).then_some(before.min(text.len()))
}

/// Replaces `cid:` pictures by `data:` addresses of the stored parts, so the frame shows them
/// without asking anything outside.
pub fn inline_cids(html: &str, images: &[(String, String, Vec<u8>)]) -> String {
    if images.is_empty() || !html.contains("cid:") {
        return html.to_string();
    }
    let mut out = html.to_string();
    for (cid, mime, data) in images {
        // Only a picture type, and a Content-ID that cannot leave the attribute.
        let Some(uri) = data_uri(mime, data) else { continue };
        if cid.contains(['"', '\'', '<', '>', '&']) {
            continue;
        }
        out = out.replace(&format!("\"cid:{cid}\""), &format!("\"{uri}\""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_handlers_and_frames_are_removed() {
        let s = sanitize_html(
            "<p onclick=\"steal()\">Hi<script>alert(1)</script></p><iframe src=\"https://x.org\"></iframe>\
             <form action=\"https://x.org\"><input name=\"pw\"></form><a href=\"javascript:alert(1)\">x</a>",
            true,
        );
        for bad in ["script", "onclick", "iframe", "form", "input", "javascript"] {
            assert!(!s.html.contains(bad), "{bad} in {}", s.html);
        }
        assert!(s.html.contains("<p>Hi</p>"), "{}", s.html);
    }

    #[test]
    fn remote_images_are_blocked_by_default() {
        let html = "<img src=\"https://track.example.org/p.gif\" width=\"1\"><img src=\"cid:logo@x\" alt=\"Logo\">\
                    <img src=\"//cdn.example.org/a.png\">";
        let blocked = sanitize_html(html, false);
        assert_eq!(blocked.blocked_remote, 2);
        assert!(!blocked.html.contains("track.example.org"));
        assert!(!blocked.html.contains("cdn.example.org"));
        assert!(blocked.html.contains("cid:logo@x"), "inline images stay: {}", blocked.html);
        let allowed = sanitize_html(html, true);
        assert_eq!(allowed.blocked_remote, 0);
        assert!(allowed.html.contains("https://track.example.org/p.gif"));
    }

    #[test]
    fn styles_keep_layout_but_load_nothing() {
        let s = sanitize_html(
            "<style>body{background:url(https://x.org/a)}</style>\
             <p style=\"color: red; background-image: url(https://x.org/b.png); padding: 4px\">x</p>",
            false,
        );
        assert!(!s.html.contains("x.org"), "{}", s.html);
        assert!(s.html.contains("color"), "{}", s.html);
        assert!(s.html.contains("padding"), "{}", s.html);
        // A color in the shorthand stays (a button's background), anything else goes.
        assert_eq!(clean_style("background:#2b2a28;color:#fff"), "background-color: #2b2a28; color: #fff");
        assert_eq!(clean_style("background: #fff url(https://x.org/a.png)"), "");
        assert_eq!(clean_style("background: linear-gradient(red, blue); position: fixed"), "");
        assert_eq!(clean_style("width: expression(alert(1)); height: 4px"), "height: 4px");
        assert_eq!(clean_style("color: red; font-family: \\66 oo"), "color: red");
    }

    #[test]
    fn links_open_without_referrer_and_data_links_go() {
        let s = sanitize_html("<a href=\"https://example.org\">a</a><a href=\"data:text/html,x\">b</a>", false);
        assert!(s.html.contains("rel=\"noopener noreferrer\""), "{}", s.html);
        assert!(!s.html.contains("data:text/html"), "{}", s.html);
    }

    #[test]
    fn trackers_go_even_with_remote_images_allowed() {
        let html = "<p>Neu</p><img src=\"https://news.example.org/open.php?id=1\" width=\"1\" height=\"1\">\
                    <a href=\"https://example.org/a?utm_source=news&amp;id=2\">Lesen</a>\
                    <img src=\"https://cdn.example.org/bild.jpg\">";
        for allow in [false, true] {
            let s = sanitize_html(html, allow);
            assert_eq!(s.trackers, 2, "{}", s.html);
            assert!(!s.html.contains("open.php") && !s.html.contains("utm_source"), "{}", s.html);
            assert!(s.html.contains("https://example.org/a?id=2"), "{}", s.html);
            assert_eq!(s.blocked_remote, usize::from(!allow), "the picture is a remote image, not a tracker");
        }
        let own = sanitize_html_with(html, true, false);
        assert_eq!(own.trackers, 0);
        assert!(own.html.contains("utm_source"), "the compose sheet keeps the user's links");
        let (text, n) = text_to_html_counted("Siehe https://example.org/x?utm_medium=mail und https://example.org/y");
        assert_eq!(n, 1);
        assert!(text.contains("href=\"https://example.org/x\"") && !text.contains("utm_medium"), "{text}");
    }

    #[test]
    fn plain_text_is_escaped() {
        let h = text_to_html("a < b\n<script>");
        assert!(!h.contains("<script>"));
        assert!(h.contains("&lt;"));
    }

    #[test]
    fn plain_text_gets_links_and_quotes() {
        let h = text_to_html("Siehe https://example.org/a?b=1&c=2.\n> alt\n>> älter\nneu");
        assert!(h.contains("<a href=\"https://example.org/a?b=1&amp;c=2\""), "{h}");
        assert!(h.contains(">https://example.org/a?b=1&amp;c=2</a>."), "the full stop stays outside: {h}");
        assert!(h.contains("<blockquote>alt<br><blockquote>älter<br></blockquote></blockquote>neu"), "{h}");
    }

    #[test]
    fn quotes_are_found_below_the_answer() {
        let html = "<div>Passt, danke!</div><div class=\"gmail_quote\"><div>Am Fr. schrieb Jonas:</div><blockquote>alt</blockquote></div>";
        let at = quote_start_html(html).unwrap();
        assert_eq!(&html[..at], "<div>Passt, danke!</div>");
        assert!(quote_start_html("<blockquote type=\"cite\">nur Zitat</blockquote>").is_none(), "nothing above it");
        assert!(quote_start_html("<p>Kein Zitat</p>").is_none());
        let text = "Passt!\n\nAm 16.10. schrieb Jonas:\n> Angebot\n> anbei\n";
        assert_eq!(&text[..quote_start_text(text).unwrap()], "Passt!\n\n");
        assert!(quote_start_text("> nur\n> Zitat").is_none());
        assert!(quote_start_text("Hallo\n> eine Zeile").is_none());
    }

    #[test]
    fn a_pictures_type_cannot_add_markup() {
        let html = sanitize_html("<img src=\"cid:logo@x\">", false).html;
        let evil = "image/x\"><link/rel=dns-prefetch/href=//victim-42.evil.example><meta/x=\"";
        for out in [
            inline_cids(&html, &[("logo@x".into(), evil.into(), vec![1, 2, 3])]),
            sanitize_html_inline("<img src=\"cid:logo@x\">", false, &[("logo@x".into(), evil.into(), vec![1, 2, 3])])
                .html,
        ] {
            assert!(!out.contains("<link") && !out.contains("victim-42") && !out.contains("<meta"), "{out}");
        }
        let ok = sanitize_html_inline(
            "<img src=\"cid:logo@x\">",
            false,
            &[("logo@x".into(), "image/svg+xml".into(), vec![1])],
        );
        assert!(ok.html.contains("src=\"data:image/svg+xml;base64,AQ==\""), "{}", ok.html);
        let odd =
            sanitize_html_inline("<img src=\"cid:logo@x\">", false, &[("logo@x".into(), "text/html".into(), vec![1])]);
        assert!(!odd.html.contains("text/html"), "only pictures: {}", odd.html);
    }

    #[test]
    fn remote_pictures_are_found_as_the_browser_reads_them() {
        // Tabs and line breaks inside, control characters and spaces before: a browser still
        // loads these.
        for src in [
            "ht&#9;tps://track.example/a.png",
            "&#1;https://track.example/a.png",
            " https://track.example/a.png",
            "https&#58;//track.example/a.png",
            "h&#10;ttps://track.example/a.png",
            "//track.example/a.png",
        ] {
            let s = sanitize_html(&format!("<img src=\"{src}\">"), false);
            assert!(!s.html.contains("track.example"), "{src}: {}", s.html);
            assert_eq!(s.blocked_remote, 1, "{src}");
        }
        // Anything that is not a picture of the message itself counts as remote.
        let s = sanitize_html("<img src=\"x-unknown:abc\">", false);
        assert!(!s.html.contains("x-unknown"), "{}", s.html);
    }

    #[test]
    fn pixels_go_whatever_the_markup_around_them() {
        for img in [
            "<img alt=\">\" src=\"https://track.example/o.gif\" width=\"1\" height=\"1\">",
            "<img src=\"https&#58;//track.example/o.gif\" width=\"1\" height=\"1\">",
            "<img src=\"ht&#9;tps://track.example/o.gif\" width=1 height=1>",
            "<IMG\nSRC=\"https://track.example/o.gif\"\nstyle=\"width:1px;height:1px\">",
        ] {
            let s = sanitize_html(&format!("<p>Neu</p>{img}<img src=\"https://cdn.example/hero.jpg\">"), true);
            assert!(!s.html.contains("track.example"), "{img}: {}", s.html);
            assert_eq!(s.trackers, 1, "{img}");
            assert!(s.html.contains("cdn.example/hero.jpg"), "{}", s.html);
        }
    }

    #[test]
    fn inline_pictures_become_data_addresses() {
        let html = sanitize_html("<img src=\"cid:logo@x\">", false).html;
        let out = inline_cids(&html, &[("logo@x".into(), "image/png".into(), vec![1, 2, 3])]);
        assert!(out.contains("data:image/png;base64,AQID"), "{out}");
    }
}
