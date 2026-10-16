//! Messages for servers without 8BITMIME (RFC 6152). The messages the app builds
//! (`compose::build`) are 7-bit already: every text part carries quoted-printable or base64,
//! every header with other characters is RFC 2047 encoded. A message from elsewhere (a raw draft,
//! an import, a test) may carry 8-bit text; [`downgrade`] rewrites only what is not 7-bit:
//!
//! - a leaf part whose body has 8-bit bytes gets quoted-printable (`text/*`) or base64 (anything
//!   else), with its `Content-Transfer-Encoding` replaced or added;
//! - an unstructured header (Subject and the like) becomes one or more encoded words;
//! - an address header keeps its addresses and encodes the display names; an address with
//!   other characters in it cannot travel without SMTPUTF8 and is refused with a clear error;
//! - a quoted parameter (`filename="Grüße.pdf"`) becomes an encoded word inside the quotes, as
//!   mail clients read it.

use base64::Engine;
use mail_parser::{HeaderName, MessageParser, PartType};

use crate::error::{Error, Result};
use crate::trf;

/// The error for an address that only SMTPUTF8 can carry.
pub fn needs_smtputf8(address: &str) -> Error {
    Error::Invalid(trf!(
        "Der Server kann die Adresse „{address}“ nicht zustellen: er unterstützt keine Umlaute in Adressen (SMTPUTF8).",
        "The server cannot deliver to “{address}”: it does not support non-ASCII characters in addresses (SMTPUTF8)."
    ))
}

/// One encoded word per at most 45 bytes of text (whole characters), folded onto lines of
/// their own (RFC 2047: at most 75 characters per word).
fn encoded_words(text: &str) -> String {
    let mut words = vec![];
    let mut chunk = String::new();
    for ch in text.chars() {
        if chunk.len() + ch.len_utf8() > 45 {
            words.push(std::mem::take(&mut chunk));
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        words.push(chunk);
    }
    words
        .iter()
        .map(|w| format!("=?utf-8?b?{}?=", base64::engine::general_purpose::STANDARD.encode(w.as_bytes())))
        .collect::<Vec<_>>()
        .join("\r\n ")
}

/// A display name or other phrase: as it is when 7-bit, else encoded.
fn phrase(text: &str) -> String {
    if text.is_ascii() {
        let needs_quotes = text.chars().any(|c| "()<>[]:;@\\,.\"".contains(c));
        if needs_quotes { format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")) } else { text.into() }
    } else {
        encoded_words(text)
    }
}

/// An address header (`From: Jörg <j@x>, …`) with its names encoded.
fn address_header(name: &str, raw_value: &str) -> Result<String> {
    let probe = format!("{name}:{raw_value}\r\n\r\n");
    let parsed = MessageParser::default().parse(probe.as_bytes()).ok_or_else(|| Error::Invalid("header".into()))?;
    let header = parsed.parts.first().and_then(|p| p.headers.first()).ok_or_else(|| Error::Invalid("header".into()))?;
    let mut out = vec![];
    if let Some(list) = header.value.as_address() {
        for a in list.iter() {
            let address = a.address().unwrap_or_default();
            if !address.is_ascii() {
                return Err(needs_smtputf8(address));
            }
            out.push(match a.name().filter(|n| !n.trim().is_empty()) {
                Some(n) => format!("{} <{address}>", phrase(n.trim())),
                None => address.to_string(),
            });
        }
    }
    Ok(format!("{name}: {}\r\n", out.join(",\r\n ")))
}

/// Encoded words inside the quoted strings of a structured header (`name="…"`).
fn quoted_params(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let Some(len) = rest[start + 1..].find('"') else { break };
        let inner = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        out.push('"');
        out.push_str(&if inner.is_ascii() { inner.to_string() } else { encoded_words(inner).replace("\r\n ", " ") });
        out.push('"');
        rest = &rest[start + 2 + len..];
    }
    out.push_str(rest);
    out
}

/// A header line (name, colon, value, line end) with 8-bit text made 7-bit.
fn header_line(name: &HeaderName, raw: &[u8]) -> Result<String> {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (field, value) = text.split_once(':').unwrap_or((text.as_str(), ""));
    let unfolded = value.replace("\r\n", "").replace('\n', "");
    match name {
        HeaderName::From
        | HeaderName::To
        | HeaderName::Cc
        | HeaderName::Bcc
        | HeaderName::ReplyTo
        | HeaderName::Sender => address_header(field, &unfolded),
        HeaderName::ContentType | HeaderName::ContentDisposition => {
            Ok(format!("{field}:{}\r\n", quoted_params(&unfolded)))
        }
        _ => Ok(format!("{field}: {}\r\n", encoded_words(unfolded.trim()))),
    }
}

/// The body of a leaf part in a 7-bit transfer encoding.
fn encode_body(body: &[u8], text: bool) -> (Vec<u8>, &'static str) {
    if text {
        let mut out = vec![];
        let _ = mail_builder::encoders::quoted_printable::quoted_printable_encode(body, &mut out, true);
        (out, "quoted-printable")
    } else {
        let b64 = base64::engine::general_purpose::STANDARD.encode(body);
        let mut out = Vec::with_capacity(b64.len() + b64.len() / 76 * 2);
        for line in b64.as_bytes().chunks(76) {
            out.extend_from_slice(line);
            out.extend_from_slice(b"\r\n");
        }
        (out, "base64")
    }
}

/// `raw` with every 8-bit header and body made 7-bit; unchanged when it is 7-bit already.
pub fn downgrade(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.is_ascii() {
        return Ok(raw.to_vec());
    }
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| Error::Invalid(crate::tr!("Die Nachricht ist beschädigt.", "The message is damaged.").into()))?;
    // Replacements (start, end, bytes), in order and without overlap.
    let mut edits: Vec<(usize, usize, Vec<u8>)> = vec![];
    for part in &msg.parts {
        let leaf = !matches!(part.body, PartType::Multipart(_) | PartType::Message(_));
        let (body_start, body_end) = (part.offset_body as usize, part.offset_end as usize);
        let body = raw.get(body_start..body_end).unwrap_or_default();
        let reencode = leaf && !body.is_ascii();
        for h in &part.headers {
            let (start, end) = (h.offset_field as usize, h.offset_end as usize);
            let line = raw.get(start..end).unwrap_or_default();
            if reencode && h.name == HeaderName::ContentTransferEncoding {
                edits.push((start, end, Vec::new()));
                continue;
            }
            if !line.is_ascii() {
                edits.push((start, end, header_line(&h.name, line)?.into_bytes()));
            }
        }
        if reencode {
            let text = part
                .headers
                .iter()
                .find(|h| h.name == HeaderName::ContentType)
                .and_then(|h| h.value.as_content_type())
                .is_none_or(|c| c.ctype().eq_ignore_ascii_case("text"));
            // The line end before the next boundary belongs to the boundary.
            let trimmed = body.strip_suffix(b"\r\n").or_else(|| body.strip_suffix(b"\n")).unwrap_or(body);
            let (encoded, name) = encode_body(trimmed, text);
            let header = format!("Content-Transfer-Encoding: {name}\r\n").into_bytes();
            // Added as the part's first header; the old one (if any) is removed above.
            let first = part.headers.first().map_or(body_start, |h| h.offset_field as usize);
            edits.push((first, first, header));
            let mut out = encoded;
            if !out.ends_with(b"\r\n") {
                out.extend_from_slice(b"\r\n");
            }
            edits.push((body_start, body_start + trimmed.len(), out));
            if trimmed.len() < body.len() {
                // Keep the line end that followed (already part of `out`): drop the original.
                edits.push((body_start + trimmed.len(), body_end, Vec::new()));
            }
        }
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 3);
    let mut at = 0;
    for (start, end, bytes) in edits {
        if start < at {
            continue;
        }
        out.extend_from_slice(&raw[at..start]);
        out.extend_from_slice(&bytes);
        at = end;
    }
    out.extend_from_slice(&raw[at..]);
    debug_assert!(out.is_ascii());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::MimeHeaders;

    fn parse(raw: &[u8]) -> mail_parser::Message<'_> {
        MessageParser::default().parse(raw).unwrap()
    }

    #[test]
    fn a_plain_8bit_message_becomes_quoted_printable() {
        let raw = "From: Jörg Müller <joerg@example.org>\r\nTo: Lea <lea@example.org>, \"Groß, Anna\" <anna@example.org>\r\n\
Subject: Grüße aus dem Süden – eine längere Betreffzeile, die über mehrere Wörter geht\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\nGrüße aus dem Konformitätstest\r\n";
        let out = downgrade(raw.as_bytes()).unwrap();
        assert!(out.is_ascii(), "{}", String::from_utf8_lossy(&out));
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("Content-Transfer-Encoding: quoted-printable"), "{text}");
        assert!(!text.contains("8bit"), "{text}");
        let m = parse(&out);
        assert_eq!(m.subject(), Some("Grüße aus dem Süden – eine längere Betreffzeile, die über mehrere Wörter geht"));
        assert_eq!(m.from().unwrap().first().unwrap().name(), Some("Jörg Müller"));
        assert_eq!(m.from().unwrap().first().unwrap().address(), Some("joerg@example.org"));
        let to: Vec<_> =
            m.to().unwrap().iter().map(|a| (a.name().unwrap().to_string(), a.address().unwrap().to_string())).collect();
        assert_eq!(to, [("Lea".into(), "lea@example.org".into()), ("Groß, Anna".into(), "anna@example.org".into())]);
        assert_eq!(m.body_text(0).unwrap().trim(), "Grüße aus dem Konformitätstest");
    }

    #[test]
    fn parts_of_a_multipart_message_and_a_file_name() {
        let raw = "From: a@example.org\r\nTo: b@example.org\r\nSubject: ok\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Schöne Grüße</p>\r\n\
--b1\r\nContent-Type: application/octet-stream; name=\"Übersicht.bin\"\r\nContent-Disposition: attachment; filename=\"Übersicht.bin\"\r\n\r\n\u{e4}\u{f6}\u{fc}\r\n--b1--\r\n";
        let out = downgrade(raw.as_bytes()).unwrap();
        assert!(out.is_ascii(), "{}", String::from_utf8_lossy(&out));
        let m = parse(&out);
        assert_eq!(m.body_html(0).unwrap().trim(), "<p>Schöne Grüße</p>");
        let att = m.attachments().next().unwrap();
        assert_eq!(att.attachment_name(), Some("Übersicht.bin"));
        assert_eq!(att.contents(), "äöü".as_bytes());
    }

    #[test]
    fn seven_bit_messages_stay_and_non_ascii_addresses_are_refused() {
        let raw = b"From: a@example.org\r\nSubject: ok\r\n\r\nhello\r\n";
        assert_eq!(downgrade(raw).unwrap(), raw);
        let bad = "From: jörg@example.org\r\nSubject: x\r\n\r\nGrüße\r\n";
        assert!(matches!(downgrade(bad.as_bytes()), Err(Error::Invalid(_))));
    }
}
