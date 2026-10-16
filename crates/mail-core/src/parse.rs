//! Messages as the app stores and shows them, read from RFC 5322 / MIME with mail-parser.

use mail_parser::{MessageParser, MimeHeaders};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::tr;

/// Characters of the preview line in the message list.
pub const SNIPPET_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    /// Position among the message's attachments (mail-parser's order), to fetch it again.
    pub index: u32,
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: u64,
    /// `Content-ID` without the angle brackets, for `cid:` images in HTML.
    pub content_id: Option<String>,
    pub inline: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedMessage {
    pub subject: String,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub reply_to: Vec<Address>,
    /// Unix seconds of the `Date` header.
    pub date: Option<i64>,
    /// Without angle brackets.
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub text: Option<String>,
    /// The HTML as sent; [`crate::sanitize`] cleans it before it is shown.
    pub html: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
    pub snippet: String,
}

fn addresses(a: Option<&mail_parser::Address<'_>>) -> Vec<Address> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| {
                let email = x.address()?.trim().to_string();
                let name = x.name().map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && *n != email);
                (!email.is_empty()).then_some(Address { name, email })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn ids(v: &mail_parser::HeaderValue<'_>) -> Vec<String> {
    let list: Vec<String> = match v.as_text_list() {
        Some(l) => l.iter().map(|s| s.to_string()).collect(),
        None => v.as_text().map(|s| vec![s.to_string()]).unwrap_or_default(),
    };
    list.into_iter().map(|s| s.trim().trim_matches(['<', '>']).to_string()).filter(|s| !s.is_empty()).collect()
}

/// One line of text: whitespace collapsed, cut at [`SNIPPET_CHARS`] characters.
pub fn snippet_of(text: &str) -> String {
    let joined = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('>'))
        .flat_map(|l| l.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    match joined.char_indices().nth(SNIPPET_CHARS) {
        Some((i, _)) => format!("{}…", joined[..i].trim_end()),
        None => joined,
    }
}

/// Reads a whole message, or only its header (the list is filled from `BODY.PEEK[HEADER]`).
pub fn parse(raw: &[u8]) -> Result<ParsedMessage> {
    let m = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| Error::Invalid(tr!("Die Nachricht ist nicht lesbar.", "The message cannot be read.").into()))?;
    let text = m.body_text(0).map(|t| t.into_owned());
    // mail-parser offers text parts as HTML too; only a real text/html part counts.
    let html = m.body_html(0).map(|t| t.into_owned()).filter(|_| m.html_bodies().any(|p| p.is_text_html()));
    // A message with HTML only: mail-parser derives the text from it.
    let snippet = snippet_of(text.as_deref().unwrap_or_default());
    let attachments = m
        .attachments()
        .enumerate()
        .map(|(i, part)| {
            let ct = part.content_type();
            let mime_type = ct.map_or("application/octet-stream".to_string(), |c| match c.subtype() {
                Some(sub) => format!("{}/{}", c.ctype(), sub).to_ascii_lowercase(),
                None => c.ctype().to_ascii_lowercase(),
            });
            let inline = part.content_disposition().is_some_and(|d| d.ctype().eq_ignore_ascii_case("inline"));
            AttachmentInfo {
                index: i as u32,
                filename: part.attachment_name().map(str::to_string),
                mime_type,
                size: part.len() as u64,
                content_id: part.content_id().map(|c| c.trim_matches(['<', '>']).to_string()),
                inline,
            }
        })
        .collect();
    Ok(ParsedMessage {
        subject: m.subject().unwrap_or_default().trim().to_string(),
        from: addresses(m.from()).into_iter().next(),
        to: addresses(m.to()),
        cc: addresses(m.cc()),
        reply_to: addresses(m.reply_to()),
        date: m.date().map(|d| d.to_timestamp()),
        message_id: m.message_id().map(|s| s.trim_matches(['<', '>']).to_string()).filter(|s| !s.is_empty()),
        in_reply_to: ids(m.in_reply_to()).into_iter().next(),
        references: ids(m.references()),
        text,
        html,
        attachments,
        snippet,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A message for tests: `id` is its Message-ID, `reply_to` the one it answers.
    pub fn sample(id: &str, subject: &str, reply_to: Option<&str>, body: &str) -> Vec<u8> {
        let mut s = String::new();
        s.push_str("From: Maria Muster <maria@example.org>\r\n");
        s.push_str("To: Jonas <jonas@example.org>, team@example.org\r\n");
        s.push_str(&format!("Subject: {subject}\r\n"));
        s.push_str("Date: Fri, 16 Oct 2026 09:30:00 +0200\r\n");
        s.push_str(&format!("Message-ID: <{id}>\r\n"));
        if let Some(r) = reply_to {
            s.push_str(&format!("In-Reply-To: <{r}>\r\nReferences: <{r}>\r\n"));
        }
        s.push_str("MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n");
        s.push_str(body);
        s.push_str("\r\n");
        s.into_bytes()
    }

    #[test]
    fn plain_message() {
        let p = parse(&sample("a1@example.org", "Angebot für Oktober", None, "Hallo Jonas,\r\nanbei das Angebot."))
            .unwrap();
        assert_eq!(p.subject, "Angebot für Oktober");
        assert_eq!(p.from, Some(Address { name: Some("Maria Muster".into()), email: "maria@example.org".into() }));
        assert_eq!(p.to.len(), 2);
        assert_eq!(p.to[1].name, None);
        assert_eq!(p.message_id.as_deref(), Some("a1@example.org"));
        assert_eq!(p.date, Some(1_792_135_800));
        assert_eq!(p.snippet, "Hallo Jonas, anbei das Angebot.");
        assert!(p.html.is_none());
    }

    #[test]
    fn reply_references() {
        let p = parse(&sample("b2@example.org", "Re: Angebot", Some("a1@example.org"), "> alt\r\nPasst.")).unwrap();
        assert_eq!(p.in_reply_to.as_deref(), Some("a1@example.org"));
        assert_eq!(p.references, vec!["a1@example.org".to_string()]);
        assert_eq!(p.snippet, "Passt.", "quoted lines are left out of the preview");
    }

    #[test]
    fn html_and_attachment() {
        let raw = "From: a@example.org\r\nSubject: Bericht\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"X\"\r\n\r\n\
--X\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hallo <b>Welt</b></p>\r\n\
--X\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"bericht.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--X--\r\n";
        let p = parse(raw.as_bytes()).unwrap();
        assert!(p.html.as_deref().unwrap().contains("<b>Welt</b>"));
        assert_eq!(p.attachments.len(), 1);
        assert_eq!(p.attachments[0].filename.as_deref(), Some("bericht.pdf"));
        assert_eq!(p.attachments[0].mime_type, "application/pdf");
        assert!(!p.attachments[0].inline);
        assert!(p.snippet.contains("Hallo Welt"), "{}", p.snippet);
    }

    #[test]
    fn long_snippets_are_cut() {
        let s = snippet_of(&"wort ".repeat(100));
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= SNIPPET_CHARS + 1);
    }
}
