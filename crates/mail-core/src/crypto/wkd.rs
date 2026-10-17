//! Web Key Directory (draft-koch-openpgp-webkey-service): a recipient's OpenPGP key from their
//! domain, only when the user asks for it. The advanced address
//! (`openpgpkey.<domain>/.well-known/openpgpkey/<domain>/hu/<hash>`) first, then the direct one
//! (`<domain>/.well-known/openpgpkey/hu/<hash>`); `<hash>` is the z-base-32 of the SHA-1 of the
//! lower-case local part. It tells that domain's server whose key was looked up, nothing else.

use sha1::Digest;

use crate::error::{Error, Result};
use crate::network::{self, Service};
use crate::{tr, trf};

const ZBASE32: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";

/// z-base-32 (RFC 6189), as WKD uses it.
pub fn zbase32(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for b in data {
        buffer = (buffer << 8) | *b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ZBASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ZBASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The two addresses of an e-mail address's key.
pub fn urls(email: &str) -> Option<[String; 2]> {
    let (local, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.to_ascii_lowercase();
    if local.is_empty() || domain.is_empty() {
        return None;
    }
    let hash = zbase32(&sha1::Sha1::digest(local.to_lowercase().as_bytes()));
    let l = crate::oauth::form_encode(local);
    Some([
        format!("https://openpgpkey.{domain}/.well-known/openpgpkey/{domain}/hu/{hash}?l={l}"),
        format!("https://{domain}/.well-known/openpgpkey/hu/{hash}?l={l}"),
    ])
}

/// Fetches the key bytes from the first of `urls` that has it (`None`: no key published).
pub async fn fetch(urls: &[String]) -> Result<Option<Vec<u8>>> {
    let http = network::client_for(Service::Keys)?;
    let mut last = None;
    for u in urls {
        if network::check_https_url(u).is_err() {
            continue;
        }
        match http.get(u).send().await {
            Ok(r) if r.status().is_success() => {
                let bytes = r.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
                if !bytes.is_empty() {
                    return Ok(Some(bytes.to_vec()));
                }
            }
            Ok(_) => {}
            Err(e) => last = Some(e.to_string()),
        }
    }
    match last {
        // Neither address answered at all: say so instead of „no key“.
        Some(e) if urls.len() > 1 && e.contains("dns") => {
            Err(Error::Network(trf!("Die Schlüssel-Suche hat nicht geklappt: {}", "The key lookup failed: {}", e)))
        }
        _ => Ok(None),
    }
}

/// The key of `email` from its domain's directory.
pub async fn lookup(email: &str) -> Result<Option<Vec<u8>>> {
    let urls = urls(email)
        .ok_or_else(|| Error::Invalid(tr!("Das ist keine E-Mail-Adresse.", "This is no e-mail address.").into()))?;
    fetch(&urls).await
}
