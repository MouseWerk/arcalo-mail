//! `mailto:` addresses (RFC 6068): a click on an e-mail link in another app opens the compose
//! sheet with the recipients, the subject and the text the link names, once Arcalo Mail is the
//! system's mail app (Settings → Über, „Als Standard-Mail-App festlegen“). The shell hands the
//! address over from the command line (Windows, Linux), the system's open event (macOS) or the
//! activity's intent (Android).
//!
//! Only these fields are taken: `to`, `cc`, `bcc`, `subject` and `body`. Other headers a link may
//! carry (`in-reply-to`, `from`, `attach` and the like) are ignored on purpose: a web page must
//! not choose the sender, attach a file of this computer, or pose as a reply.

use serde::Serialize;

/// What a `mailto:` address asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Mailto {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// The longest address taken (links longer than this are cut off by most systems anyway).
const MAX_LEN: usize = 64 * 1024;

/// Reads a `mailto:` address; `None` for anything else (another scheme, a file path).
pub fn parse(raw: &str) -> Option<Mailto> {
    let raw = raw.trim();
    if raw.len() > MAX_LEN {
        return None;
    }
    let rest = raw.get(..7).filter(|s| s.eq_ignore_ascii_case("mailto:")).map(|_| &raw[7..])?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut m = Mailto::default();
    m.to.extend(addresses(&decode(path)));
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = decode(value);
        match decode(key).to_ascii_lowercase().as_str() {
            "to" => m.to.extend(addresses(&value)),
            "cc" => m.cc.extend(addresses(&value)),
            "bcc" => m.bcc.extend(addresses(&value)),
            "subject" if m.subject.is_empty() => m.subject = one_line(&value),
            "body" if m.body.is_empty() => m.body = value.replace("\r\n", "\n"),
            _ => {}
        }
    }
    Some(m)
}

/// The first `mailto:` address among command line arguments (the program path and options are
/// skipped).
pub fn from_args<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> Option<Mailto> {
    args.into_iter().find_map(|a| parse(a.as_ref()))
}

/// Percent-decoding as RFC 3986 has it: `+` stays a plus (an address may hold one).
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = bytes.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(h).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The comma-separated addresses of a field, without empty ones.
fn addresses(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|a| !a.is_empty()).map(one_line).collect()
}

/// No line breaks where a header goes.
fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_address() {
        let m = parse("mailto:jonas@example.org").unwrap();
        assert_eq!(m.to, ["jonas@example.org"]);
        assert_eq!(m.subject, "");
        assert_eq!(parse("MAILTO:a@example.org").unwrap().to, ["a@example.org"]);
    }

    #[test]
    fn fields_are_decoded() {
        let m = parse(
            "mailto:a@example.org,b@example.org?cc=c%40example.org&bcc=d@example.org&subject=Gr%C3%BC%C3%9Fe%20aus%20Berlin&body=Hallo%0D%0Azweite%20Zeile%2B1",
        )
        .unwrap();
        assert_eq!(m.to, ["a@example.org", "b@example.org"]);
        assert_eq!(m.cc, ["c@example.org"]);
        assert_eq!(m.bcc, ["d@example.org"]);
        assert_eq!(m.subject, "Grüße aus Berlin");
        assert_eq!(m.body, "Hallo\nzweite Zeile+1");
    }

    #[test]
    fn plus_addresses_stay() {
        assert_eq!(parse("mailto:maria+news@example.org").unwrap().to, ["maria+news@example.org"]);
    }

    #[test]
    fn only_the_known_fields_are_taken() {
        let m = parse(
            "mailto:?to=a@example.org&from=evil@example.org&attach=/etc/passwd&in-reply-to=%3Cx%3E&subject=a%0Ab",
        )
        .unwrap();
        assert_eq!(m.to, ["a@example.org"]);
        assert_eq!(m.subject, "a b", "no line break in a header");
        assert_eq!(m.body, "");
    }

    #[test]
    fn other_arguments_are_not_mailto() {
        assert_eq!(parse("https://example.org"), None);
        assert_eq!(parse("/usr/bin/arcalo-mail"), None);
        assert_eq!(parse("mail"), None);
        let args = ["/opt/arcalo-mail", "--flag", "mailto:x@example.org?subject=Hi"];
        assert_eq!(from_args(args).unwrap().subject, "Hi");
        assert_eq!(from_args(["arcalo-mail"]), None);
        assert_eq!(parse(&format!("mailto:{}", "a".repeat(MAX_LEN))), None);
    }

    #[test]
    fn broken_escapes_stay_as_they_are() {
        assert_eq!(parse("mailto:a@example.org?subject=100%25%20sure%zz%4").unwrap().subject, "100% sure%zz%4");
    }
}
