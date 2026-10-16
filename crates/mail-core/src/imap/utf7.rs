//! Folder names on the wire: IMAP's modified UTF-7 (RFC 3501, 5.1.3). „Entwürfe“ travels as
//! `Entw&APw-rfe`; `&` itself as `&-`. The app keeps UTF-8 everywhere else.

use base64::Engine;
use base64::alphabet::Alphabet;
use base64::engine::{GeneralPurpose, GeneralPurposeConfig};

/// Base64 with `,` instead of `/` and without padding.
fn engine() -> GeneralPurpose {
    let alphabet = Alphabet::new("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,").expect("alphabet");
    GeneralPurpose::new(
        &alphabet,
        GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    )
}

/// UTF-8 → modified UTF-7.
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = vec![];
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
        out.push('&');
        out.push_str(&engine().encode(bytes));
        out.push('-');
        pending.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// Modified UTF-7 → UTF-8. A malformed name comes back as it is (some servers send UTF-8).
pub fn decode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else { return name.to_string() };
        let chunk = &after[..end];
        if chunk.is_empty() {
            out.push('&');
        } else {
            let Ok(bytes) = engine().decode(chunk) else { return name.to_string() };
            if bytes.len() % 2 != 0 {
                return name.to_string();
            }
            let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            match String::from_utf16(&units) {
                Ok(s) => out.push_str(&s),
                Err(_) => return name.to_string(),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for (utf8, wire) in [
            ("INBOX", "INBOX"),
            ("Entwürfe", "Entw&APw-rfe"),
            ("Gelöschte Elemente", "Gel&APY-schte Elemente"),
            ("A & B", "A &- B"),
            ("日本語", "&ZeVnLIqe-"),
            ("Projekte/Größe", "Projekte/Gr&APYA3w-e"),
        ] {
            assert_eq!(encode(utf8), wire, "{utf8}");
            assert_eq!(decode(wire), utf8, "{wire}");
        }
        assert_eq!(decode("Broken &xyz"), "Broken &xyz");
        assert_eq!(decode("Grüße"), "Grüße", "UTF-8 from a server stays");
    }
}
