//! HTML mail, cleaned before it is shown.
//!
//! Two layers keep a message from running code or calling home. This one removes everything
//! outside an allow-list (scripts, forms, frames, event handlers, `<style>` and `<link>`, CSS
//! with `url()`), and removes remote images unless the user allowed remote content for the
//! message or the sender; [`Sanitized::blocked_remote`] says how many were removed, for the
//! „Externe Inhalte laden“ bar. The reading pane then shows the result in an `<iframe sandbox>`
//! without scripts and with its own Content Security Policy (docs/ARCHITECTURE.md).

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

fn is_remote(url: &str) -> bool {
    let u = url.trim_start().to_ascii_lowercase();
    u.starts_with("http:") || u.starts_with("https:") || u.starts_with("//")
}

/// Cleans the HTML of a message. `allow_remote`: keep remote images (the user chose to load
/// them).
pub fn sanitize_html(html: &str, allow_remote: bool) -> Sanitized {
    let blocked = Arc::new(AtomicUsize::new(0));
    let counter = blocked.clone();
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
        .filter_style_properties(STYLE_PROPERTIES.iter().copied().collect())
        .attribute_filter(move |element, attribute, value| match (element, attribute) {
            // `data:` only for pictures; a link must not open a document made by the sender.
            (_, "href") if value.trim_start().to_ascii_lowercase().starts_with("data:") => None,
            ("img", "src") if is_remote(value) && !allow_remote => {
                counter.fetch_add(1, Ordering::Relaxed);
                None
            }
            _ => Some(Cow::Borrowed(value)),
        });
    let html = b.clean(html).to_string();
    Sanitized { html, blocked_remote: blocked.load(Ordering::Relaxed) }
}

/// The text of a message as HTML for the same frame (escaped, line breaks kept, links not
/// made clickable yet).
pub fn text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    out.push_str("<div class=\"plain\">");
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' => out.push_str("<br>"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out.push_str("</div>");
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
    }

    #[test]
    fn links_open_without_referrer_and_data_links_go() {
        let s = sanitize_html("<a href=\"https://example.org\">a</a><a href=\"data:text/html,x\">b</a>", false);
        assert!(s.html.contains("rel=\"noopener noreferrer\""), "{}", s.html);
        assert!(!s.html.contains("data:text/html"), "{}", s.html);
    }

    #[test]
    fn plain_text_is_escaped() {
        let h = text_to_html("a < b\n<script>");
        assert!(!h.contains("<script>"));
        assert!(h.contains("&lt;"));
    }
}
