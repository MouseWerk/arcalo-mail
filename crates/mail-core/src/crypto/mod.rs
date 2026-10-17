//! Signed and encrypted mail: OpenPGP ([`pgp`], PGP/MIME, RFC 3156, and inline text) and S/MIME
//! ([`smime`], RFC 8551). Sending wraps the finished message's body ([`protect`]); reading
//! unwraps it ([`open`]) into the entity the reading pane shows and what it says about the
//! signature. Decrypted content is never stored: it is opened again each time, and remote
//! content in it never loads.
//!
//! Keys: other people's OpenPGP keys and S/MIME certificates are in the store (imported, from
//! Autocrypt headers, from a Web Key Directory, from signed mail); the user's secret keys and
//! PKCS#12 files are in the credential store ([`Keys`] reads them for the core).

pub mod pgp;
pub mod smime;
pub mod wkd;

use mail_parser::MessageParser;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::{Store, now};
use crate::{tr, trf};

/// The user's secrets, from the credential store.
pub trait Keys {
    /// An own OpenPGP secret key (armored) by fingerprint.
    fn pgp_secret(&self, fingerprint: &str) -> Option<String>;
    /// An own PKCS#12 file and its password by the certificate's fingerprint.
    fn smime_secret(&self, fingerprint: &str) -> Option<(Vec<u8>, String)>;
    /// The passphrase of an own protected OpenPGP key, when the user gave it (kept in memory for
    /// a while, or in the credential store when the user chose so).
    fn pgp_passphrase(&self, _fingerprint: &str) -> Option<String> {
        None
    }
}

/// The secrets the shell hands the core once at start (sending and reading use them).
static REGISTERED: std::sync::RwLock<Option<std::sync::Arc<dyn Keys + Send + Sync>>> = std::sync::RwLock::new(None);

pub fn register(keys: std::sync::Arc<dyn Keys + Send + Sync>) {
    *REGISTERED.write().unwrap_or_else(|e| e.into_inner()) = Some(keys);
}

pub fn registered() -> std::sync::Arc<dyn Keys + Send + Sync> {
    REGISTERED.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| std::sync::Arc::new(NoKeys))
}

/// No secrets (a device without keys; tests of plain mail).
pub struct NoKeys;

impl Keys for NoKeys {
    fn pgp_secret(&self, _: &str) -> Option<String> {
        None
    }
    fn smime_secret(&self, _: &str) -> Option<(Vec<u8>, String)> {
        None
    }
}

/// The credential store names.
pub fn pgp_secret_key(fingerprint: &str) -> String {
    format!("pgp-{fingerprint}-secret")
}

/// The credential store name of an own key's passphrase (only when the user chose to keep it).
pub fn pgp_passphrase_key(fingerprint: &str) -> String {
    format!("pgp-{fingerprint}-passphrase")
}

pub fn smime_secret_key(fingerprint: &str) -> String {
    format!("smime-{fingerprint}-p12")
}

// ---------------------------------------------------------------------------- stored keys

/// A key or certificate as the settings list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredKey {
    /// `pgp` or `smime`.
    pub kind: String,
    pub fingerprint: String,
    pub email: String,
    pub name: String,
    pub own: bool,
    /// `generated`, `imported`, `autocrypt`, `wkd`, `mail`.
    pub source: String,
    /// S/MIME: when the certificate ends (Unix seconds).
    pub expires: Option<i64>,
}

/// Keeps an OpenPGP public key (an own one marked as such) for every address of its user ids.
pub fn save_pgp(
    store: &Store,
    key: &pgp::SignedPublicKey,
    own: bool,
    source: &str,
    prefer_encrypt: bool,
) -> Result<pgp::KeyInfo> {
    let info = keep_pgp(store, key, own, source, prefer_encrypt)?;
    for (_, email) in pgp::users(key).into_iter().filter(|(_, e)| !e.is_empty()) {
        map_pgp(store, &info.fingerprint, &email, source, prefer_encrypt, now())?;
    }
    Ok(info)
}

/// The key's row (its first address as the one the list shows).
fn keep_pgp(
    store: &Store,
    key: &pgp::SignedPublicKey,
    own: bool,
    source: &str,
    prefer_encrypt: bool,
) -> Result<pgp::KeyInfo> {
    let info = pgp::info(key);
    if info.email.is_empty() {
        return Err(Error::Invalid(
            tr!("Der Schlüssel nennt keine E-Mail-Adresse.", "The key names no e-mail address.").into(),
        ));
    }
    let armored = pgp::armored_public(key)?;
    store.conn().execute(
        "INSERT INTO pgp_keys (fingerprint, email, name, public, own, source, prefer_encrypt, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT (fingerprint) DO UPDATE SET public = excluded.public, own = MAX(own, excluded.own),
           prefer_encrypt = excluded.prefer_encrypt, updated_at = excluded.updated_at,
           source = CASE WHEN own = 1 OR source <> 'autocrypt' THEN source ELSE excluded.source END",
        params![info.fingerprint, info.email, info.name, armored, own, source, prefer_encrypt, now()],
    )?;
    Ok(info)
}

/// Uses a key for an address. A key the user imported (or found in a directory) stays such
/// when Autocrypt brings it again.
fn map_pgp(store: &Store, fingerprint: &str, email: &str, source: &str, prefer_encrypt: bool, seen: i64) -> Result<()> {
    store.conn().execute(
        "INSERT INTO pgp_key_emails (fingerprint, email, source, prefer_encrypt, seen_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (fingerprint, email) DO UPDATE SET prefer_encrypt = excluded.prefer_encrypt,
           seen_at = MAX(seen_at, excluded.seen_at),
           source = CASE WHEN source <> 'autocrypt' THEN source ELSE excluded.source END",
        params![fingerprint, email.trim().to_ascii_lowercase(), source, prefer_encrypt, seen],
    )?;
    Ok(())
}

/// Keeps the key of an Autocrypt header for the address it was sent from (`addr`, the sender),
/// as of the message's date `seen`. A header older than the one kept changes nothing; a newer
/// one replaces the Autocrypt key of that address (never an imported one).
pub fn save_autocrypt(
    store: &Store,
    key: &pgp::SignedPublicKey,
    addr: &str,
    prefer_encrypt: bool,
    seen: i64,
) -> Result<bool> {
    let addr = addr.trim().to_ascii_lowercase();
    let fp = pgp::fingerprint_of(key);
    let newer: Option<i64> = store
        .conn()
        .query_row(
            "SELECT MAX(seen_at) FROM pgp_key_emails WHERE email = ?1 AND source = 'autocrypt' AND fingerprint <> ?2",
            params![addr, fp],
            |r| r.get(0),
        )
        .unwrap_or(None);
    if newer.is_some_and(|n| n > seen) {
        return Ok(false);
    }
    keep_pgp(store, key, false, "autocrypt", prefer_encrypt)?;
    map_pgp(store, &fp, &addr, "autocrypt", prefer_encrypt, seen)?;
    // The address's earlier Autocrypt key is no longer its key.
    store.conn().execute(
        "DELETE FROM pgp_key_emails WHERE email = ?1 AND source = 'autocrypt' AND fingerprint <> ?2",
        params![addr, fp],
    )?;
    store.conn().execute(
        "DELETE FROM pgp_keys WHERE own = 0 AND source = 'autocrypt'
           AND fingerprint NOT IN (SELECT fingerprint FROM pgp_key_emails)",
        [],
    )?;
    Ok(true)
}

/// The OpenPGP keys of an address (own ones too): own keys first, then keys the user imported
/// or found in a directory, then the address's Autocrypt key.
pub fn pgp_keys_for(store: &Store, email: &str) -> Result<Vec<pgp::SignedPublicKey>> {
    let mut st = store.conn().prepare(
        "SELECT k.public FROM pgp_key_emails e JOIN pgp_keys k ON k.fingerprint = e.fingerprint WHERE e.email = ?1
         ORDER BY k.own DESC, e.source = 'autocrypt', e.seen_at DESC, k.updated_at DESC",
    )?;
    let rows: Vec<String> =
        st.query_map([email.trim().to_ascii_lowercase()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows.iter().filter_map(|a| pgp::read_public(a.as_bytes()).ok()).collect())
}

/// The address's key prefers encrypted mail (Autocrypt `prefer-encrypt=mutual`).
fn prefers_encrypt(store: &Store, email: &str) -> Result<bool> {
    let mut st = store.conn().prepare(
        "SELECT e.prefer_encrypt FROM pgp_key_emails e JOIN pgp_keys k ON k.fingerprint = e.fingerprint
         WHERE e.email = ?1 ORDER BY k.own DESC, e.source = 'autocrypt', e.seen_at DESC LIMIT 1",
    )?;
    let v: Option<bool> = st.query_map([email.trim().to_ascii_lowercase()], |r| r.get(0))?.next().transpose()?;
    Ok(v.unwrap_or(false))
}

/// The own key of `from` that needs a passphrase before it can sign, when the user has not given
/// it yet (the compose sheet asks for it before sending).
pub fn locked_pgp(store: &Store, from: &str, keys: &dyn Keys) -> Result<Option<String>> {
    for fp in own_pgp(store, Some(from))? {
        if let Some(armored) = keys.pgp_secret(&fp) {
            return Ok(match unlocked(&fp, &armored, keys) {
                Err(_) if pgp::read_secret(&armored).is_ok_and(|k| pgp::is_protected(&k)) => Some(fp),
                _ => None,
            });
        }
    }
    Ok(None)
}

/// The own secret keys that can decrypt now, and the fingerprints of those still locked.
fn own_secrets(store: &Store, keys: &dyn Keys) -> (Vec<pgp::Secret>, Vec<String>) {
    let mut open = vec![];
    let mut locked = vec![];
    for fp in own_pgp(store, None).unwrap_or_default() {
        let Some(armored) = keys.pgp_secret(&fp) else { continue };
        match unlocked(&fp, &armored, keys) {
            Ok(s) => open.push(s),
            Err(_) if pgp::read_secret(&armored).is_ok() => locked.push(fp),
            Err(_) => {}
        }
    }
    (open, locked)
}

/// The fingerprints of the user's own OpenPGP keys for an address (every own key without one).
pub fn own_pgp(store: &Store, email: Option<&str>) -> Result<Vec<String>> {
    let mut st = store.conn().prepare(
        "SELECT fingerprint FROM pgp_keys k WHERE own = 1
           AND (?1 IS NULL OR email = ?1 OR EXISTS (SELECT 1 FROM pgp_key_emails e WHERE e.fingerprint = k.fingerprint AND e.email = ?1))
         ORDER BY updated_at DESC",
    )?;
    let rows = st.query_map([email.map(|e| e.trim().to_ascii_lowercase())], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Keeps an S/MIME certificate (DER). Only a `trusted` one (an own one, one that chains to a
/// trusted root, one the user trusted) is used to encrypt.
pub fn save_cert(store: &Store, der: &[u8], own: bool, trusted: bool) -> Result<smime::CertInfo> {
    let info = smime::cert_info(der)?;
    let email = info.emails.first().cloned().ok_or_else(|| {
        Error::Invalid(
            tr!("Das Zertifikat nennt keine E-Mail-Adresse.", "The certificate names no e-mail address.").into(),
        )
    })?;
    store.conn().execute(
        "INSERT INTO smime_certs (fingerprint, email, subject, issuer, not_after, der, own, created_at, trusted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (fingerprint) DO UPDATE SET own = MAX(own, excluded.own), trusted = MAX(trusted, excluded.trusted)",
        params![info.fingerprint, email, info.subject, info.issuer, info.not_after, der, own, now(), own || trusted],
    )?;
    Ok(info)
}

/// The trusted certificates of an address that have not ended, newest first.
pub fn certs_for(store: &Store, email: &str) -> Result<Vec<Vec<u8>>> {
    let mut st = store.conn().prepare(
        "SELECT der FROM smime_certs WHERE email = ?1 AND not_after > ?2 AND (trusted = 1 OR own = 1)
         ORDER BY own DESC, not_after DESC",
    )?;
    let rows = st.query_map(params![email.trim().to_ascii_lowercase(), now()], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn own_certs(store: &Store, email: Option<&str>) -> Result<Vec<String>> {
    let mut st = store.conn().prepare(
        "SELECT fingerprint FROM smime_certs WHERE own = 1 AND (?1 IS NULL OR email = ?1) AND not_after > ?2 ORDER BY not_after DESC",
    )?;
    let rows = st.query_map(params![email.map(|e| e.trim().to_ascii_lowercase()), now()], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every key and certificate.
pub fn list(store: &Store) -> Result<Vec<StoredKey>> {
    let mut out = vec![];
    let mut st =
        store.conn().prepare("SELECT fingerprint, email, name, own, source FROM pgp_keys ORDER BY own DESC, email")?;
    let rows = st.query_map([], |r| {
        Ok(StoredKey {
            kind: "pgp".into(),
            fingerprint: r.get(0)?,
            email: r.get(1)?,
            name: r.get(2)?,
            own: r.get(3)?,
            source: r.get(4)?,
            expires: None,
        })
    })?;
    out.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
    let mut st = store
        .conn()
        .prepare("SELECT fingerprint, email, subject, own, not_after FROM smime_certs ORDER BY own DESC, email")?;
    let rows = st.query_map([], |r| {
        Ok(StoredKey {
            kind: "smime".into(),
            fingerprint: r.get(0)?,
            email: r.get(1)?,
            name: r.get(2)?,
            own: r.get(3)?,
            source: "mail".into(),
            expires: r.get(4)?,
        })
    })?;
    out.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
    Ok(out)
}

pub fn remove(store: &Store, kind: &str, fingerprint: &str) -> Result<()> {
    let table = if kind == "smime" { "smime_certs" } else { "pgp_keys" };
    store.conn().execute(&format!("DELETE FROM {table} WHERE fingerprint = ?1"), [fingerprint])?;
    if kind != "smime" {
        store.conn().execute("DELETE FROM pgp_key_emails WHERE fingerprint = ?1", [fingerprint])?;
    }
    Ok(())
}

/// The armored public key of a stored OpenPGP key (for export).
pub fn public_armored(store: &Store, fingerprint: &str) -> Result<String> {
    Ok(store.conn().query_row("SELECT public FROM pgp_keys WHERE fingerprint = ?1", [fingerprint], |r| r.get(0))?)
}

/// What can be done for a set of recipients: sign (an own key or certificate for the sender) and
/// encrypt (a key or certificate for every recipient); the addresses without one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Abilities {
    pub pgp_sign: bool,
    pub pgp_encrypt: bool,
    pub pgp_missing: Vec<String>,
    pub smime_sign: bool,
    pub smime_encrypt: bool,
    pub smime_missing: Vec<String>,
    /// Every recipient's key prefers encrypted mail (Autocrypt `prefer-encrypt=mutual`): the
    /// compose sheet starts encrypted.
    pub pgp_prefer_encrypt: bool,
}

pub fn abilities(store: &Store, from: &str, recipients: &[String]) -> Result<Abilities> {
    let pgp_sign = !own_pgp(store, Some(from))?.is_empty();
    let smime_sign = !own_certs(store, Some(from))?.is_empty();
    let mut a = Abilities { pgp_sign, smime_sign, ..Abilities::default() };
    for r in recipients.iter().map(|r| r.trim().to_ascii_lowercase()).filter(|r| !r.is_empty()) {
        if pgp_keys_for(store, &r)?.iter().all(|k| !pgp::info(k).can_encrypt) {
            a.pgp_missing.push(r.clone());
        }
        if certs_for(store, &r)?.is_empty() {
            a.smime_missing.push(r.clone());
        }
    }
    let any = recipients.iter().any(|r| !r.trim().is_empty());
    a.pgp_encrypt = pgp_sign && any && a.pgp_missing.is_empty();
    let mut prefer = a.pgp_encrypt;
    for r in recipients.iter().filter(|r| !r.trim().is_empty()) {
        prefer = prefer && prefers_encrypt(store, r)?;
    }
    a.pgp_prefer_encrypt = prefer;
    a.smime_encrypt = smime_sign && any && a.smime_missing.is_empty();
    Ok(a)
}

// ---------------------------------------------------------------------------- MIME

/// The header fields of a message (each with its folded lines) and where its body starts.
fn split(raw: &[u8]) -> (Vec<String>, &[u8]) {
    // The first empty line, whichever line ends the message uses.
    let crlf = find(raw, b"\r\n\r\n");
    let lf = find(raw, b"\n\n");
    let (head, body) = match (crlf, lf) {
        (Some(a), Some(b)) if a < b => (&raw[..a], &raw[a + 4..]),
        (_, Some(b)) => (&raw[..b], &raw[b + 2..]),
        (Some(a), None) => (&raw[..a], &raw[a + 4..]),
        (None, None) => (raw, &raw[raw.len()..]),
    };
    let text = String::from_utf8_lossy(head);
    let mut fields: Vec<String> = vec![];
    for line in text.split('\n').map(|l| l.trim_end_matches('\r')) {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = fields.last_mut() {
                last.push_str("\r\n");
                last.push_str(line);
            }
        } else if !line.is_empty() {
            fields.push(line.to_string());
        }
    }
    (fields, body)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn field_name(f: &str) -> String {
    f.split(':').next().unwrap_or_default().trim().to_ascii_lowercase()
}

fn is_content(f: &str) -> bool {
    let n = field_name(f);
    n.starts_with("content-") || n == "mime-version"
}

/// Line ends as CRLF (the canonical form signatures are made over).
pub fn canonical(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 40);
    let mut prev = 0u8;
    for &b in data {
        if b == b'\n' && prev != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        prev = b;
    }
    out
}

/// The body of a message as a MIME entity of its own: its content fields and the body.
pub fn entity_of(raw: &[u8]) -> Vec<u8> {
    let (fields, body) = split(raw);
    let mut out = String::new();
    for f in fields.iter().filter(|f| is_content(f) && field_name(f) != "mime-version") {
        out.push_str(f);
        out.push_str("\r\n");
    }
    if !fields.iter().any(|f| field_name(f) == "content-type") {
        out.push_str("Content-Type: text/plain; charset=us-ascii\r\n");
    }
    out.push_str("\r\n");
    let mut v = out.into_bytes();
    v.extend_from_slice(body);
    canonical(&v)
}

/// The message with its own header (everything but the content fields) over a new body.
fn rebuild(raw: &[u8], content_type: &str, extra: &[&str], body: &[u8]) -> Vec<u8> {
    let (fields, _) = split(raw);
    let mut out = String::new();
    for f in fields.iter().filter(|f| !is_content(f)) {
        out.push_str(f);
        out.push_str("\r\n");
    }
    out.push_str("MIME-Version: 1.0\r\n");
    out.push_str(&format!("Content-Type: {content_type}\r\n"));
    for e in extra {
        out.push_str(e);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut v = out.into_bytes();
    v.extend_from_slice(body);
    v
}

/// A header field added to a message (before its body).
pub fn add_header(raw: &[u8], name: &str, value: &str) -> Vec<u8> {
    let (fields, body) = split(raw);
    let mut out = String::new();
    for f in &fields {
        out.push_str(f);
        out.push_str("\r\n");
    }
    out.push_str(&format!("{name}: {value}\r\n\r\n"));
    let mut v = out.into_bytes();
    v.extend_from_slice(body);
    v
}

fn boundary() -> String {
    use rand::RngCore;
    let mut b = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut b);
    format!("=-arcalo-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

fn base64_lines(data: &[u8]) -> String {
    use base64::Engine;
    let s = base64::engine::general_purpose::STANDARD.encode(data);
    s.as_bytes().chunks(76).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("\r\n")
}

/// `multipart/signed` around `entity` with the signature part.
fn signed_body(entity: &[u8], sig_type: &str, sig_extra: &str, sig: &str) -> (String, Vec<u8>) {
    let b = boundary();
    let mut body = format!("This is an OpenPGP/MIME or S/MIME signed message.\r\n--{b}\r\n").into_bytes();
    body.extend_from_slice(entity);
    body.extend_from_slice(
        format!("\r\n--{b}\r\nContent-Type: {sig_type}\r\n{sig_extra}\r\n{sig}\r\n--{b}--\r\n").as_bytes(),
    );
    (b, body)
}

/// How a message is protected when it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Pgp,
    Smime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protect {
    pub method: Method,
    pub sign: bool,
    pub encrypt: bool,
}

fn missing_keys(list: &[String]) -> Error {
    Error::Invalid(trf!(
        "Für {} fehlt ein Schlüssel zum Verschlüsseln.",
        "A key to encrypt is missing for {}.",
        list.join(", ")
    ))
}

/// The error for an own key that needs its passphrase first.
pub fn locked_error(fingerprint: &str) -> Error {
    Error::Invalid(trf!(
        "Der OpenPGP-Schlüssel {fingerprint} ist mit einer Passphrase geschützt. Gib sie ein, um ihn zu nutzen.",
        "The OpenPGP key {fingerprint} is protected with a passphrase. Enter it to use the key."
    ))
}

/// An own secret key, unlocked: a protected key needs the passphrase the user gave.
fn unlocked(fingerprint: &str, armored: &str, keys: &dyn Keys) -> Result<pgp::Secret> {
    let key = pgp::read_secret(armored)?;
    if !pgp::is_protected(&key) {
        return Ok(pgp::Secret::plain(key));
    }
    match keys.pgp_passphrase(fingerprint) {
        Some(p) if pgp::unlocks(&key, &p) => Ok(pgp::Secret { key, password: p.as_str().into() }),
        _ => Err(locked_error(fingerprint)),
    }
}

/// The own key `from` signs with, unlocked.
fn own_pgp_secret(store: &Store, from: &str, keys: &dyn Keys) -> Result<pgp::Secret> {
    for fp in own_pgp(store, Some(from))? {
        if let Some(armored) = keys.pgp_secret(&fp) {
            return unlocked(&fp, &armored, keys);
        }
    }
    Err(Error::Invalid(trf!(
        "Für {} gibt es keinen eigenen OpenPGP-Schlüssel.",
        "There is no own OpenPGP key for {}.",
        from
    )))
}

fn own_identity(store: &Store, from: &str, keys: &dyn Keys) -> Result<smime::Identity> {
    for fp in own_certs(store, Some(from))? {
        if let Some((p12, pw)) = keys.smime_secret(&fp) {
            return smime::read_p12(&p12, &pw);
        }
    }
    Err(Error::Invalid(trf!(
        "Für {} gibt es kein eigenes S/MIME-Zertifikat.",
        "There is no own S/MIME certificate for {}.",
        from
    )))
}

/// Signs and/or encrypts a finished message (`raw`, as compose built it) from `from` to
/// `recipients`. An encrypted message is also encrypted to the sender, so the copy in Sent
/// stays readable.
pub fn protect(
    store: &Store,
    from: &str,
    raw: &[u8],
    recipients: &[String],
    how: &Protect,
    keys: &dyn Keys,
) -> Result<Vec<u8>> {
    if !how.sign && !how.encrypt {
        return Ok(raw.to_vec());
    }
    let entity = entity_of(raw);
    match how.method {
        Method::Pgp => {
            let secret = if how.sign { Some(own_pgp_secret(store, from, keys)?) } else { None };
            if !how.encrypt {
                let secret = secret.as_ref().expect("signing");
                let sig = pgp::sign_detached(secret, &entity)?;
                let (b, body) = signed_body(
                    &entity,
                    "application/pgp-signature; name=\"signature.asc\"",
                    "Content-Description: OpenPGP digital signature\r\nContent-Disposition: attachment; filename=\"signature.asc\"\r\n",
                    sig.trim_end(),
                );
                return Ok(rebuild(
                    raw,
                    &format!(
                        "multipart/signed; micalg=pgp-sha256; protocol=\"application/pgp-signature\"; boundary=\"{b}\""
                    ),
                    &[],
                    &body,
                ));
            }
            // The user's own key too, so the copy in Sent stays readable.
            let own = match &secret {
                Some(s) => s.key.to_public_key(),
                None => {
                    let fp = own_pgp(store, Some(from))?.into_iter().next().ok_or_else(|| {
                        Error::Invalid(trf!(
                            "Für {} gibt es keinen eigenen OpenPGP-Schlüssel.",
                            "There is no own OpenPGP key for {}.",
                            from
                        ))
                    })?;
                    pgp::read_public(public_armored(store, &fp)?.as_bytes())?
                }
            };
            let mut list = vec![own];
            let mut missing = vec![];
            for r in recipients.iter().map(|r| r.trim().to_ascii_lowercase()).filter(|r| !r.is_empty()) {
                match pgp_keys_for(store, &r)?.into_iter().find(|k| pgp::info(k).can_encrypt) {
                    Some(k) => list.push(k),
                    None => missing.push(r),
                }
            }
            if !missing.is_empty() {
                return Err(missing_keys(&missing));
            }
            let armored = pgp::encrypt(&entity, &list, secret.as_ref())?;
            let b = boundary();
            let body = format!(
                "This is an OpenPGP/MIME encrypted message (RFC 4880 and 3156)\r\n--{b}\r\nContent-Type: application/pgp-encrypted\r\nContent-Description: PGP/MIME version identification\r\n\r\nVersion: 1\r\n\r\n--{b}\r\nContent-Type: application/octet-stream; name=\"encrypted.asc\"\r\nContent-Description: OpenPGP encrypted message\r\nContent-Disposition: inline; filename=\"encrypted.asc\"\r\n\r\n{}\r\n--{b}--\r\n",
                armored.trim_end().replace("\r\n", "\n").replace('\n', "\r\n")
            );
            Ok(rebuild(
                raw,
                &format!("multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"{b}\""),
                &[],
                body.as_bytes(),
            ))
        }
        Method::Smime => {
            let id = own_identity(store, from, keys)?;
            let signed = if how.sign {
                let der = smime::sign_detached(&id, &entity)?;
                let (b, body) = signed_body(
                    &entity,
                    "application/pkcs7-signature; name=\"smime.p7s\"",
                    "Content-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\n",
                    &base64_lines(&der),
                );
                Some((
                    format!(
                        "multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"{b}\""
                    ),
                    body,
                ))
            } else {
                None
            };
            if !how.encrypt {
                let (ct, body) = signed.expect("signed");
                return Ok(rebuild(raw, &ct, &[], &body));
            }
            let inner = match &signed {
                Some((ct, body)) => {
                    let mut v = format!("Content-Type: {ct}\r\n\r\n").into_bytes();
                    v.extend_from_slice(body);
                    v
                }
                None => entity,
            };
            let mut certs = vec![id.cert.clone()];
            let mut missing = vec![];
            for r in recipients.iter().map(|r| r.trim().to_ascii_lowercase()).filter(|r| !r.is_empty()) {
                match certs_for(store, &r)?.into_iter().next() {
                    Some(c) => certs.push(c),
                    None => missing.push(r),
                }
            }
            if !missing.is_empty() {
                return Err(missing_keys(&missing));
            }
            let der = smime::encrypt(&inner, &certs)?;
            Ok(rebuild(
                raw,
                "application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\"",
                &["Content-Transfer-Encoding: base64", "Content-Disposition: attachment; filename=\"smime.p7m\""],
                format!("{}\r\n", base64_lines(&der)).as_bytes(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------- reading

/// What the reading pane says about a message's protection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Protection {
    /// `pgp` or `smime`.
    pub method: String,
    pub encrypted: bool,
    /// It could be opened here.
    pub decrypted: bool,
    pub signed: bool,
    /// `valid`, `untrusted` (S/MIME: no trusted root), `mismatch` (signed by another address),
    /// `unknown_key` (no key to check it), `invalid`.
    pub signature: Option<String>,
    pub signer: Option<String>,
    pub fingerprint: Option<String>,
    /// Why it could not be opened.
    pub error: Option<String>,
    /// An own key that could open it is protected with a passphrase not given yet.
    pub locked: Option<String>,
    /// Text outside the signed or encrypted block is shown too: no badge for the whole message.
    pub partial: bool,
}

/// An opened message: the entity to show (`None` when it could not be opened) and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub entity: Option<Vec<u8>>,
    pub protection: Protection,
    /// S/MIME: the signer's certificate (DER), for trusting it.
    pub cert: Option<Vec<u8>>,
}

/// The type of an entity (`multipart/signed`) and its parameters (lower-case names).
fn content_type(raw: &[u8]) -> (String, Vec<(String, String)>) {
    let (fields, _) = split(raw);
    let Some(f) = fields.iter().find(|f| field_name(f) == "content-type") else { return ("text/plain".into(), vec![]) };
    let value = f.split_once(':').map(|(_, v)| v).unwrap_or_default().replace("\r\n", " ");
    let mut parts = split_params(&value).into_iter();
    let mime = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().trim_matches('"').to_string()))
        })
        .collect();
    (mime, params)
}

fn split_params(v: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    for c in v.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// The parts of a multipart entity, exactly as sent (each with its header; the line break before
/// a delimiter belongs to the delimiter).
fn parts(raw: &[u8], boundary: &str) -> Vec<Vec<u8>> {
    let (_, body) = split(raw);
    let body = canonical(body);
    let delim = format!("--{boundary}");
    let mut out = vec![];
    let mut starts = vec![];
    let mut i = 0;
    while let Some(p) = find(&body[i..], delim.as_bytes()) {
        let at = i + p;
        // A delimiter starts a line.
        if at == 0 || body[..at].ends_with(b"\r\n") {
            starts.push(at);
        }
        i = at + delim.len();
    }
    for w in starts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut s = a + delim.len();
        // The rest of the delimiter line.
        if let Some(nl) = find(&body[s..], b"\r\n") {
            s += nl + 2;
        }
        let end = b.saturating_sub(2).max(s);
        out.push(body[s..end].to_vec());
    }
    out
}

/// The decoded content of a single-part entity.
fn decoded(entity: &[u8]) -> Vec<u8> {
    MessageParser::default().parse(entity).and_then(|m| m.part(0).map(|p| p.contents().to_vec())).unwrap_or_default()
}

fn pgp_verdict(p: &mut Protection, v: pgp::Verdict, sender: Option<&str>) {
    p.signed = true;
    match v {
        pgp::Verdict::Valid { fingerprint, signer, emails } => {
            let matches = sender.is_none_or(|s| emails.iter().any(|e| e.eq_ignore_ascii_case(s.trim())));
            p.signature = Some(if matches { "valid" } else { "mismatch" }.into());
            p.signer = Some(signer);
            p.fingerprint = Some(fingerprint);
        }
        pgp::Verdict::UnknownKey { fingerprint } => {
            p.signature = Some("unknown_key".into());
            p.fingerprint = fingerprint;
        }
        pgp::Verdict::Invalid => p.signature = Some("invalid".into()),
    }
}

fn smime_verdict(store: &Store, p: &mut Protection, v: smime::Verdict, sender: Option<&str>) -> Option<Vec<u8>> {
    p.signed = true;
    match v {
        smime::Verdict::Valid { signer, fingerprint, emails, trusted, cert } => {
            let matches = sender.is_none_or(|s| emails.iter().any(|e| e.eq_ignore_ascii_case(s)));
            p.signature = Some(
                if !matches {
                    "mismatch"
                } else if trusted {
                    "valid"
                } else {
                    "untrusted"
                }
                .into(),
            );
            p.signer = Some(match emails.first() {
                Some(e) if *e != signer.to_ascii_lowercase() => format!("{signer} <{e}>"),
                _ => signer,
            });
            p.fingerprint = Some(fingerprint);
            // The sender's certificate, for answering encrypted: only one that chains to a
            // trusted root (an untrusted one waits until the user trusts it).
            if matches && trusted {
                let _ = save_cert(store, &cert, false, true);
            }
            matches.then_some(cert)
        }
        smime::Verdict::Invalid => {
            p.signature = Some("invalid".into());
            None
        }
    }
}

/// The verdict of an inner signed layer taken over by the encrypted message around it.
fn take_signature(p: &mut Protection, nested: Opened) -> Opened {
    p.signed = true;
    p.signature = nested.protection.signature;
    p.signer = nested.protection.signer;
    p.fingerprint = nested.protection.fingerprint;
    Opened { entity: nested.entity, protection: std::mem::take(p), cert: nested.cert }
}

/// Decrypts OpenPGP data with the own keys that are unlocked; says which locked key might open it.
fn pgp_decrypt(
    store: &Store,
    data: &[u8],
    sender_keys: &[pgp::SignedPublicKey],
    keys: &dyn Keys,
    p: &mut Protection,
) -> Option<pgp::Decrypted> {
    let (secrets, locked) = own_secrets(store, keys);
    match pgp::decrypt(data, &secrets, sender_keys) {
        Ok(d) => Some(d),
        Err(_) if !locked.is_empty() => {
            p.error = Some(locked_error(&locked[0]).to_string());
            p.locked = Some(locked[0].clone());
            None
        }
        Err(e) => {
            p.error = Some(e.to_string());
            None
        }
    }
}

/// Unwraps a protected message. `None` for a message without OpenPGP or S/MIME.
pub fn open(store: &Store, raw: &[u8], sender: Option<&str>, keys: &dyn Keys) -> Option<Opened> {
    let (mime, params) = content_type(raw);
    let protocol = param(&params, "protocol").unwrap_or_default().to_ascii_lowercase();
    let sender_keys = || sender.map(|s| pgp_keys_for(store, s).unwrap_or_default()).unwrap_or_default();
    let mut p = Protection::default();
    match (mime.as_str(), protocol.as_str()) {
        ("multipart/encrypted", "application/pgp-encrypted") => {
            p.method = "pgp".into();
            p.encrypted = true;
            let list = parts(raw, param(&params, "boundary")?);
            let data = decoded(list.get(1)?);
            let Some(d) = pgp_decrypt(store, &data, &sender_keys(), keys, &mut p) else {
                return Some(Opened { entity: None, protection: p, cert: None });
            };
            p.decrypted = true;
            if let Some(v) = d.signature {
                pgp_verdict(&mut p, v, sender);
            }
            let inner = canonical(&d.data);
            // Signed, then encrypted as two layers (RFC 3156, 6.1).
            if let Some(nested) = open(store, &inner, sender, keys).filter(|o| o.protection.signed) {
                return Some(take_signature(&mut p, nested));
            }
            Some(Opened { entity: Some(inner), protection: p, cert: None })
        }
        ("multipart/signed", "application/pgp-signature") => {
            p.method = "pgp".into();
            let list = parts(raw, param(&params, "boundary")?);
            let (content, sig) = (list.first()?, decoded(list.get(1)?));
            pgp_verdict(&mut p, pgp::verify_detached(&sig, content, &sender_keys()), sender);
            Some(Opened { entity: Some(content.clone()), protection: p, cert: None })
        }
        ("multipart/signed", "application/pkcs7-signature" | "application/x-pkcs7-signature") => {
            p.method = "smime".into();
            let list = parts(raw, param(&params, "boundary")?);
            let (content, sig) = (list.first()?, decoded(list.get(1)?));
            let cert = match smime::verify(&sig, Some(content), &smime::system_anchors(), now()) {
                Ok((v, _)) => smime_verdict(store, &mut p, v, sender),
                Err(_) => {
                    p.signature = Some("invalid".into());
                    None
                }
            };
            Some(Opened { entity: Some(content.clone()), protection: p, cert })
        }
        ("application/pkcs7-mime" | "application/x-pkcs7-mime", _) => {
            p.method = "smime".into();
            let der = decoded(&entity_of(raw));
            let kind = param(&params, "smime-type").unwrap_or("enveloped-data").to_ascii_lowercase();
            if kind == "signed-data" {
                return match smime::verify(&der, None, &smime::system_anchors(), now()) {
                    Ok((v, inner)) => {
                        let cert = smime_verdict(store, &mut p, v, sender);
                        Some(Opened { entity: inner.map(|i| canonical(&i)), protection: p, cert })
                    }
                    Err(e) => {
                        p.error = Some(e.to_string());
                        Some(Opened { entity: None, protection: p, cert: None })
                    }
                };
            }
            p.encrypted = true;
            let ids: Vec<smime::Identity> = own_certs(store, None)
                .unwrap_or_default()
                .iter()
                .filter_map(|fp| keys.smime_secret(fp))
                .filter_map(|(d, pw)| smime::read_p12(&d, &pw).ok())
                .collect();
            match smime::decrypt(&der, &ids) {
                Ok(inner) => {
                    p.decrypted = true;
                    let inner = canonical(&inner);
                    if let Some(nested) = open(store, &inner, sender, keys).filter(|o| o.protection.signed) {
                        return Some(take_signature(&mut p, nested));
                    }
                    Some(Opened { entity: Some(inner), protection: p, cert: None })
                }
                Err(e) => {
                    p.error = Some(e.to_string());
                    Some(Opened { entity: None, protection: p, cert: None })
                }
            }
        }
        _ => None,
    }
}

/// Trusts the S/MIME certificate a signed message from `sender` was signed with (the user
/// checked it), so mail to the sender can be encrypted with it.
pub fn trust_signer(store: &Store, raw: &[u8], sender: &str, keys: &dyn Keys) -> Result<smime::CertInfo> {
    let cert = open(store, raw, Some(sender), keys).and_then(|o| o.cert).ok_or_else(|| {
        Error::Invalid(
            tr!(
                "Die Nachricht ist nicht mit einem Zertifikat des Absenders signiert.",
                "The message is not signed with a certificate of its sender."
            )
            .into(),
        )
    })?;
    save_cert(store, &cert, false, true)
}

/// Inline OpenPGP in a text body: an encrypted block (decrypted to text) or a cleartext
/// signature (checked, the armor taken off). `None` for text without either. The text returned
/// is what the reading pane shows instead of the body. An encrypted block shows alone. A signed
/// block with unsigned text around it shows with that text, and its protection says `partial`:
/// a good signature then counts for part of what is shown only, never as „valid“.
pub fn open_inline(store: &Store, text: &str, sender: Option<&str>, keys: &dyn Keys) -> Option<(String, Protection)> {
    let sender_keys = sender.map(|s| pgp_keys_for(store, s).unwrap_or_default()).unwrap_or_default();
    let mut p = Protection { method: "pgp".into(), ..Protection::default() };
    if let (Some(a), Some(b)) = (text.find("-----BEGIN PGP MESSAGE-----"), text.find("-----END PGP MESSAGE-----")) {
        p.encrypted = true;
        let end = b + "-----END PGP MESSAGE-----".len();
        let block = text.get(a..end)?;
        p.partial = !text[..a].trim().is_empty() || !text[end..].trim().is_empty();
        let Some(d) = pgp_decrypt(store, block.as_bytes(), &sender_keys, keys, &mut p) else {
            return Some((String::new(), p));
        };
        p.decrypted = true;
        if let Some(v) = d.signature {
            pgp_verdict(&mut p, v, sender);
        }
        return Some((String::from_utf8_lossy(&d.data).into_owned(), p));
    }
    let c = pgp::verify_cleartext(text, &sender_keys)?;
    pgp_verdict(&mut p, c.verdict, sender);
    if c.whole {
        return Some((c.text, p));
    }
    p.partial = true;
    if p.signature.as_deref() == Some("valid") {
        p.signature = Some("partial".into());
    }
    Some((format!("{}{}{}", c.around.0, c.text, c.around.1), p))
}

/// Keeps the key of an incoming message's `Autocrypt` header (Autocrypt 1.1): only for the
/// address it names when that is the sender's (never an own address), filed under that address
/// alone, and only when the message is newer than the one that brought the address's current
/// Autocrypt key. A key the user imported keeps its place.
pub fn ingest_autocrypt(store: &Store, raw: &[u8], sender: &str) -> Result<()> {
    let (fields, _) = split(raw);
    let me = crate::calendar::my_addresses(store)?;
    // The message's date, never later than now; without one the header counts for nothing.
    let Some(date) = MessageParser::default().parse_headers(raw).and_then(|m| m.date().map(|d| d.to_timestamp()))
    else {
        return Ok(());
    };
    let seen = date.min(now());
    for f in fields.iter().filter(|f| field_name(f) == "autocrypt") {
        let value = f.split_once(':').map(|(_, v)| v).unwrap_or_default().replace("\r\n", "");
        if let Some((addr, mutual, key)) = pgp::parse_autocrypt(&value)
            && addr.eq_ignore_ascii_case(sender.trim())
            && !me.iter().any(|m| m.eq_ignore_ascii_case(&addr))
        {
            save_autocrypt(store, &key, &addr, mutual, seen)?;
        }
    }
    Ok(())
}

/// The `Autocrypt` header for mail from `from`, when the user has an own key for it.
pub fn autocrypt_for(store: &Store, from: &str) -> Result<Option<String>> {
    let Some(fp) = own_pgp(store, Some(from))?.into_iter().next() else { return Ok(None) };
    let key = pgp::read_public(public_armored(store, &fp)?.as_bytes())?;
    Ok(Some(pgp::autocrypt_header(from, &key, true)?))
}

#[cfg(test)]
mod tests;
