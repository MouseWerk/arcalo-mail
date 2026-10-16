//! New messages: the compose sheet's draft becomes an RFC 5322 message (mail-builder), with
//! text and HTML, attachments, the reply headers and a Message-ID of the sender's domain.
//! The same bytes are sent, filed in Sent and saved as a draft.

use serde::{Deserialize, Serialize};

use crate::account::{Account, domain_of};
use crate::error::{Error, Result};
use crate::parse::Address;
use crate::trf;

/// What the compose sheet hands over.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    pub account_id: i64,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    /// The text; with `html`, its plain alternative (made from the HTML when empty).
    pub text: String,
    /// The rich text of the editor, `None` in plain-text mode.
    pub html: Option<String>,
    /// Files to attach, by path (the shell reads them).
    pub attachments: Vec<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Kept across saves of the same draft, so the old copy in Drafts can be replaced.
    pub message_id: Option<String>,
    /// The message replied to (marked answered once sent).
    pub reply_to_id: Option<i64>,
    /// A forwarded message whose attachments go along (the shell fetches them).
    pub forward_from: Option<i64>,
}

/// A file read for sending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAttachment {
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// Reads `Name <a@b.org>`, `"Muster, Maria" <a@b.org>` or `a@b.org`.
pub fn parse_address(input: &str) -> Result<Address> {
    let s = input.trim();
    let invalid =
        || Error::Invalid(trf!("„{s}“ ist keine gültige E-Mail-Adresse.", "“{s}” is not a valid e-mail address."));
    let (name, email) = match (s.rfind('<'), s.ends_with('>')) {
        (Some(i), true) => {
            let name = s[..i].trim().trim_matches('"').trim();
            (Some(name.to_string()).filter(|n| !n.is_empty()), s[i + 1..s.len() - 1].trim())
        }
        _ => (None, s),
    };
    let ok = email.split_once('@').is_some_and(|(local, _)| {
        !local.is_empty() && !local.contains(char::is_whitespace) && domain_of(email).is_some()
    });
    if !ok {
        return Err(invalid());
    }
    Ok(Address { name, email: email.to_string() })
}

/// A new Message-ID at the sender's domain (`<random@example.org>`).
pub fn new_message_id(from_email: &str) -> String {
    let domain = domain_of(from_email).unwrap_or_else(|| "localhost".into());
    let mut bytes = [0u8; 12];
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    // Unique enough for an id: time, process and a counter, mixed.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut x = (t as u64) ^ ((std::process::id() as u64) << 32) ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for b in bytes.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{id}@{domain}")
}

/// The text of an HTML body (the plain alternative), as mail-parser reads it.
pub fn html_to_text(html: &str) -> String {
    let raw = format!("Content-Type: text/html; charset=utf-8\r\n\r\n{html}");
    mail_parser::MessageParser::default()
        .parse(raw.as_bytes())
        .and_then(|m| m.body_text(0).map(|t| t.into_owned()))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The envelope recipients of a draft (To, Cc and Bcc), checked.
pub fn recipients(d: &Draft) -> Result<Vec<Address>> {
    let all: Vec<Address> =
        d.to.iter()
            .chain(&d.cc)
            .chain(&d.bcc)
            .filter(|s| !s.trim().is_empty())
            .map(|s| parse_address(s))
            .collect::<Result<_>>()?;
    if all.is_empty() {
        return Err(Error::Invalid(
            crate::tr!("Gib mindestens einen Empfänger an.", "Add at least one recipient.").into(),
        ));
    }
    Ok(all)
}

fn list(field: &[String]) -> Result<Vec<(String, String)>> {
    field
        .iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| parse_address(s).map(|a| (a.name.unwrap_or_default(), a.email)))
        .collect()
}

/// Builds the message. `with_bcc`: keep the Bcc header (the copy in Sent and drafts; the
/// message that goes out never shows it).
pub fn build(
    account: &Account,
    d: &Draft,
    files: &[FileAttachment],
    message_id: &str,
    date: i64,
    with_bcc: bool,
) -> Result<Vec<u8>> {
    use mail_builder::MessageBuilder;
    use mail_builder::headers::address::Address as MbAddress;
    let to_mb = |v: Vec<(String, String)>| -> Vec<MbAddress<'static>> {
        v.into_iter().map(|(n, e)| if n.is_empty() { MbAddress::from(e) } else { MbAddress::from((n, e)) }).collect()
    };
    let mut b = MessageBuilder::new()
        .from(if account.sender_name.trim().is_empty() {
            MbAddress::from(account.email.clone())
        } else {
            MbAddress::from((account.sender_name.trim().to_string(), account.email.clone()))
        })
        .subject(d.subject.trim().to_string())
        .message_id(message_id.trim_matches(['<', '>']).to_string())
        .date(mail_builder::headers::date::Date::new(date));
    let to = list(&d.to)?;
    if !to.is_empty() {
        b = b.to(to_mb(to));
    }
    let cc = list(&d.cc)?;
    if !cc.is_empty() {
        b = b.cc(to_mb(cc));
    }
    let bcc = list(&d.bcc)?;
    if with_bcc && !bcc.is_empty() {
        b = b.bcc(to_mb(bcc));
    }
    if let Some(r) = &d.in_reply_to {
        b = b.in_reply_to(r.trim_matches(['<', '>']).to_string());
    }
    if !d.references.is_empty() {
        let refs: Vec<String> = d.references.iter().map(|r| r.trim_matches(['<', '>']).to_string()).collect();
        b = b.references(mail_builder::headers::message_id::MessageId::new_list(refs.into_iter()));
    }
    let text = match (&d.html, d.text.trim().is_empty()) {
        (Some(html), true) => html_to_text(html),
        _ => d.text.clone(),
    };
    b = b.text_body(text);
    if let Some(html) = &d.html {
        // A small frame of styles, so the message looks the same in other clients.
        b = b.html_body(format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"></head>\
             <body style=\"font-family: -apple-system, 'Segoe UI', Roboto, Arial, sans-serif; font-size: 14px; line-height: 1.5\">{html}</body></html>"
        ));
    }
    for f in files {
        b = b.attachment(f.mime_type.clone(), f.name.clone(), f.data.clone());
    }
    b.write_to_vec().map_err(|e| Error::Invalid(e.to_string()))
}

/// The type of a file by its extension (enough for the receiving client to open it).
pub fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "ics" => "text/calendar",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "eml" => "message/rfc822",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{Provider, preset};
    use crate::parse::parse;

    fn account() -> Account {
        Account {
            id: 1,
            provider: Provider::Imap,
            name: String::new(),
            email: "maria@example.org".into(),
            sender_name: "Maria Muster".into(),
            transport: preset(Provider::Imap, "maria@example.org").unwrap(),
            signature: String::new(),
        }
    }

    #[test]
    fn addresses() {
        let a = parse_address("\"Muster, Maria\" <maria@example.org>").unwrap();
        assert_eq!((a.name.as_deref(), a.email.as_str()), (Some("Muster, Maria"), "maria@example.org"));
        assert_eq!(parse_address(" jonas@example.org ").unwrap().name, None);
        for bad in ["", "jonas", "jonas@", "a b@example.org", "<>", "x <y>"] {
            assert!(parse_address(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn builds_a_reply_with_html_and_an_attachment() {
        let d = Draft {
            account_id: 1,
            to: vec!["Jonas <jonas@example.org>".into()],
            cc: vec!["team@example.org".into()],
            bcc: vec!["chef@example.org".into()],
            subject: "Re: Angebot für Oktober".into(),
            text: String::new(),
            html: Some("<p>Passt, <b>danke</b>!</p>".into()),
            in_reply_to: Some("<a1@example.org>".into()),
            references: vec!["a0@example.org".into(), "a1@example.org".into()],
            ..Draft::default()
        };
        let file =
            FileAttachment { name: "Grüße.pdf".into(), mime_type: "application/pdf".into(), data: b"%PDF".to_vec() };
        let id = new_message_id("maria@example.org");
        assert!(id.ends_with("@example.org"));
        let raw = build(&account(), &d, &[file], &id, 1_792_135_800, false).unwrap();
        let p = parse(&raw).unwrap();
        assert_eq!(p.subject, "Re: Angebot für Oktober");
        assert_eq!(p.from.unwrap().name.as_deref(), Some("Maria Muster"));
        assert_eq!(p.to[0].email, "jonas@example.org");
        assert_eq!(p.in_reply_to.as_deref(), Some("a1@example.org"));
        assert_eq!(p.references, ["a0@example.org", "a1@example.org"]);
        assert_eq!(p.message_id.as_deref(), Some(id.as_str()));
        assert!(p.html.unwrap().contains("<b>danke</b>"));
        assert!(p.text.unwrap().contains("Passt, danke!"), "the plain alternative");
        assert_eq!(p.attachments.iter().filter(|a| !a.inline).count(), 1);
        assert_eq!(p.attachments[0].filename.as_deref(), Some("Grüße.pdf"));
        assert!(!String::from_utf8_lossy(&raw).contains("chef@example.org"), "no Bcc in what goes out");
        let kept = build(&account(), &d, &[], &id, 1_792_135_800, true).unwrap();
        assert!(String::from_utf8_lossy(&kept).contains("chef@example.org"), "Bcc in the copy in Sent");
        assert_eq!(recipients(&d).unwrap().len(), 3);
        assert!(recipients(&Draft::default()).is_err());
        assert_eq!(mime_for("Bericht.PDF"), "application/pdf");
    }
}
