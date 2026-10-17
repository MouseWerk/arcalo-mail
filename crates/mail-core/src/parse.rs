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

impl AttachmentInfo {
    /// A part that carries the message rather than a file of it: the encrypted body and the
    /// signature of OpenPGP or S/MIME, a meeting's calendar data. The list's paperclip leaves
    /// them out (the reading pane shows them as a lock, a check, an invitation).
    pub fn is_protocol(&self) -> bool {
        let mime = self.mime_type.to_ascii_lowercase();
        matches!(
            mime.as_str(),
            "application/pgp-encrypted"
                | "application/pgp-signature"
                | "application/pkcs7-signature"
                | "application/x-pkcs7-signature"
                | "application/pkcs7-mime"
                | "application/x-pkcs7-mime"
                | "text/calendar"
                | "application/ics"
        ) || (mime == "application/octet-stream" && self.filename.as_deref() == Some("encrypted.asc"))
    }
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
    /// The list and automation headers the inbox is sorted by ([`crate::triage`]).
    #[serde(default)]
    pub list: ListHeaders,
}

/// What the header says about where a message comes from: a mailing list or newsletter, a
/// machine, a person.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListHeaders {
    /// `List-Id` without the angle brackets.
    pub list_id: Option<String>,
    /// `List-Unsubscribe` as sent: `<https://…>, <mailto:…>`.
    pub unsubscribe: Option<String>,
    /// `List-Unsubscribe-Post: List-Unsubscribe=One-Click` (RFC 8058).
    pub one_click: bool,
    /// `List-Post`: a discussion list people write to.
    pub list_post: bool,
    /// `Precedence` in lower case (`bulk`, `list`, `junk`).
    pub precedence: Option<String>,
    /// `Auto-Submitted` other than `no` (RFC 3834): written by a machine.
    pub auto_submitted: bool,
    /// The newest (topmost) `Authentication-Results` (RFC 8601): what the receiving server found
    /// ([`crate::avatars::dmarc_passed`] reads it).
    #[serde(default)]
    pub authentication: Option<String>,
}

/// One line of a header as sent: folded lines joined, surrounding blanks removed.
fn raw_header(m: &mail_parser::Message<'_>, name: &str) -> Option<String> {
    let v = m.header_raw(name)?;
    let joined = v.split(['\r', '\n']).map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    (!joined.is_empty()).then_some(joined)
}

fn list_headers(m: &mail_parser::Message<'_>) -> ListHeaders {
    ListHeaders {
        list_id: raw_header(m, "List-Id").map(|v| match (v.rfind('<'), v.rfind('>')) {
            (Some(a), Some(b)) if a < b => v[a + 1..b].trim().to_ascii_lowercase(),
            _ => v.trim().to_ascii_lowercase(),
        }),
        unsubscribe: raw_header(m, "List-Unsubscribe"),
        one_click: raw_header(m, "List-Unsubscribe-Post")
            .is_some_and(|v| v.replace(' ', "").eq_ignore_ascii_case("List-Unsubscribe=One-Click")),
        list_post: raw_header(m, "List-Post").is_some_and(|v| !v.eq_ignore_ascii_case("NO")),
        precedence: raw_header(m, "Precedence").map(|v| v.to_ascii_lowercase()),
        auto_submitted: raw_header(m, "Auto-Submitted").is_some_and(|v| !v.trim().eq_ignore_ascii_case("no")),
        authentication: raw_header(m, "Authentication-Results"),
    }
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
        list: list_headers(&m),
    })
}

fn mime_of(part: &mail_parser::MessagePart<'_>) -> String {
    part.content_type().map_or("text/plain".to_string(), |c| match c.subtype() {
        Some(sub) => format!("{}/{}", c.ctype(), sub).to_ascii_lowercase(),
        None => c.ctype().to_ascii_lowercase(),
    })
}

/// The content of attachment `index` (the order of [`ParsedMessage::attachments`]), decoded.
pub fn attachment_data(raw: &[u8], index: u32) -> Option<Vec<u8>> {
    let m = MessageParser::default().parse(raw)?;
    m.attachments().nth(index as usize).map(|p| p.contents().to_vec())
}

/// Pictures the HTML refers to by `cid:`: Content-ID, type and the decoded bytes.
pub fn inline_images(raw: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    let Some(m) = MessageParser::default().parse(raw) else { return vec![] };
    m.attachments()
        .filter_map(|p| {
            let cid = p.content_id()?.trim_matches(['<', '>']).to_string();
            let mime = mime_of(p);
            mime.starts_with("image/").then(|| (cid, mime, p.contents().to_vec()))
        })
        .collect()
}

/// The leaves of the MIME tree with their IMAP section numbers (what `BODYSTRUCTURE` says, for
/// the fake server and for messages read from the store).
pub fn structure(raw: &[u8]) -> Vec<crate::imap::BodyPart> {
    let Some(m) = MessageParser::default().parse(raw) else { return vec![] };
    let mut out = vec![];
    walk_parts(&m, 0, "", &mut out);
    out
}

fn walk_parts(m: &mail_parser::Message<'_>, id: u32, prefix: &str, out: &mut Vec<crate::imap::BodyPart>) {
    let Some(part) = m.part(id) else { return };
    if let mail_parser::PartType::Multipart(children) = &part.body {
        for (i, child) in children.iter().enumerate() {
            let section = if prefix.is_empty() { (i + 1).to_string() } else { format!("{prefix}.{}", i + 1) };
            walk_parts(m, *child, &section, out);
        }
        return;
    }
    let mime_type = mime_of(part);
    out.push(crate::imap::BodyPart {
        section: if prefix.is_empty() { "1".into() } else { prefix.into() },
        filename: part.attachment_name().map(str::to_string),
        size: (part.offset_end.saturating_sub(part.offset_body)) as u64,
        content_id: part.content_id().map(|c| c.trim_matches(['<', '>']).to_string()),
        inline: !part.content_disposition().is_some_and(|d| d.ctype().eq_ignore_ascii_case("attachment")),
        mime_type,
    });
}

/// The raw bytes of one part with its MIME header (`BODY[1.2.MIME]` and `BODY[1.2]`), which
/// parse as a message of their own.
pub fn section(raw: &[u8], section: &str) -> Option<Vec<u8>> {
    let m = MessageParser::default().parse(raw)?;
    let mut id = 0u32;
    let path: Vec<usize> = section.split('.').map(|s| s.parse::<usize>().ok()).collect::<Option<_>>()?;
    for (depth, n) in path.iter().enumerate() {
        match &m.part(id)?.body {
            mail_parser::PartType::Multipart(children) => id = *children.get(n.checked_sub(1)?)?,
            // A single-part message: its only part is 1.
            _ if depth == 0 && *n == 1 => {}
            _ => return None,
        }
    }
    let p = m.part(id)?;
    raw.get(p.offset_header as usize..p.offset_end as usize).map(<[u8]>::to_vec)
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
    fn calendar_data_and_signatures_are_no_files() {
        let raw = b"From: a@example.org\r\nSubject: Einladung\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=B\r\n\r\n--B\r\nContent-Type: text/plain\r\n\r\nHallo\r\n--B\r\nContent-Type: text/calendar; method=REQUEST\r\nContent-Disposition: attachment; filename=invite.ics\r\n\r\nBEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n--B\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=plan.pdf\r\n\r\n%PDF\r\n--B--\r\n";
        let p = parse(raw).unwrap();
        let kinds: Vec<(String, bool)> = p.attachments.iter().map(|a| (a.mime_type.clone(), a.is_protocol())).collect();
        assert!(kinds.contains(&("text/calendar".into(), true)), "{kinds:?}");
        assert!(kinds.contains(&("application/pdf".into(), false)), "{kinds:?}");
        let encrypted = AttachmentInfo {
            index: 1,
            filename: Some("encrypted.asc".into()),
            mime_type: "application/octet-stream".into(),
            size: 10,
            content_id: None,
            inline: false,
        };
        assert!(encrypted.is_protocol());
        assert!(!AttachmentInfo { filename: Some("daten.bin".into()), ..encrypted }.is_protocol());
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
        assert_eq!(attachment_data(raw.as_bytes(), 0).as_deref(), Some(&b"%PDF-1.4\n"[..]));

        let parts = structure(raw.as_bytes());
        assert_eq!(parts.iter().map(|p| p.section.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(parts[1].filename.as_deref(), Some("bericht.pdf"));
        assert!(parts[1].is_attachment() && !parts[0].is_attachment());
        let pdf = section(raw.as_bytes(), "2").unwrap();
        assert!(String::from_utf8_lossy(&pdf).starts_with("Content-Type: application/pdf"));
        let again = parse(&pdf).unwrap();
        assert_eq!(again.attachments.len(), 1, "a part with its header reads as a message");
        assert!(section(raw.as_bytes(), "3").is_none());
        let single = sample("s@x", "Kurz", None, "Text");
        assert_eq!(structure(&single).len(), 1);
        assert!(section(&single, "1").is_some());
    }

    #[test]
    fn list_and_automation_headers() {
        let raw = "From: Field Notes <letters@fieldnotes.example>\r\nSubject: Ausgabe 48\r\n\
List-Id: Field Notes <letters.fieldnotes.example>\r\n\
List-Unsubscribe: <https://fieldnotes.example/u?t=1>,\r\n <mailto:leave@fieldnotes.example?subject=leave>\r\n\
List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nPrecedence: Bulk\r\n\r\nHallo\r\n";
        let l = parse(raw.as_bytes()).unwrap().list;
        assert_eq!(l.list_id.as_deref(), Some("letters.fieldnotes.example"));
        assert_eq!(
            l.unsubscribe.as_deref(),
            Some("<https://fieldnotes.example/u?t=1>, <mailto:leave@fieldnotes.example?subject=leave>")
        );
        assert!(l.one_click && !l.list_post && !l.auto_submitted);
        assert_eq!(l.precedence.as_deref(), Some("bulk"));
        let auto = parse(b"From: x@example.org\r\nAuto-Submitted: auto-generated\r\nList-Post: <mailto:l@x>\r\n\r\nx")
            .unwrap()
            .list;
        assert!(auto.auto_submitted && auto.list_post);
        let person = parse(b"From: x@example.org\r\nAuto-Submitted: no\r\n\r\nx").unwrap().list;
        assert_eq!(person, ListHeaders::default());
    }

    #[test]
    fn long_snippets_are_cut() {
        let s = snippet_of(&"wort ".repeat(100));
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= SNIPPET_CHARS + 1);
    }
}
