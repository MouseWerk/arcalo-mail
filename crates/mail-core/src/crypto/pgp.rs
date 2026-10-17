//! OpenPGP with rPGP: keys made here (Ed25519 for signing, Curve25519 for encryption, readable
//! by GnuPG and every current client), keys imported as armored text, detached signatures for
//! PGP/MIME (RFC 3156), encryption to the recipients' encryption subkeys (and the user's own,
//! so the copy in Sent stays readable) with a signature inside, decryption and verification,
//! cleartext-signed text, and Autocrypt's header (`addr`, `prefer-encrypt`, `keydata`).

use std::io::Cursor;

use ::pgp::composed::{
    ArmorOptions, CleartextSignedMessage, Deserializable, DetachedSignature, EncryptionCaps, KeyType, Message,
    MessageBuilder, SecretKeyParamsBuilder, SubkeyParamsBuilder,
};
use ::pgp::crypto::ecc_curve::ECCCurve;
use ::pgp::crypto::hash::HashAlgorithm;
use ::pgp::crypto::sym::SymmetricKeyAlgorithm;
use ::pgp::ser::Serialize as _;
use ::pgp::types::{CompressionAlgorithm, KeyDetails, Password, VerifyingKey};
use base64::Engine;
use serde::Serialize;

pub use ::pgp::composed::{SignedPublicKey, SignedSecretKey};

use crate::error::{Error, Result};
use crate::{tr, trf};

fn pgp_error(e: impl std::fmt::Display) -> Error {
    Error::Invalid(trf!("OpenPGP: {}", "OpenPGP: {}", e))
}

/// A key as the settings list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyInfo {
    /// Upper-case hex, 40 characters for v4 keys.
    pub fingerprint: String,
    pub name: String,
    pub email: String,
    /// Unix seconds.
    pub created: i64,
    pub can_encrypt: bool,
}

/// `Name <a@b>` split into name and lower-case address.
fn split_user(uid: &str) -> (String, String) {
    let u = uid.trim();
    match u.rsplit_once('<') {
        Some((name, rest)) => {
            (name.trim().trim_matches('"').to_string(), rest.trim_end_matches('>').trim().to_ascii_lowercase())
        }
        None if u.contains('@') => (String::new(), u.to_ascii_lowercase()),
        None => (u.to_string(), String::new()),
    }
}

pub fn fingerprint_of(key: &SignedPublicKey) -> String {
    format!("{:X}", key.primary_key.fingerprint())
}

/// The user ids of a key: (name, address).
pub fn users(key: &SignedPublicKey) -> Vec<(String, String)> {
    key.details.users.iter().map(|u| split_user(&String::from_utf8_lossy(u.id.id()))).collect()
}

fn encryption_capable(key: &SignedPublicKey) -> bool {
    key.public_subkeys.iter().any(|s| {
        s.key.algorithm().can_encrypt()
            && s.signatures.iter().any(|g| {
                let f = g.key_flags();
                f.encrypt_comms() || f.encrypt_storage()
            })
    }) || key.primary_key.algorithm().can_encrypt()
}

pub fn info(key: &SignedPublicKey) -> KeyInfo {
    let (name, email) = users(key).into_iter().find(|(_, e)| !e.is_empty()).unwrap_or_default();
    KeyInfo {
        fingerprint: fingerprint_of(key),
        name,
        email,
        created: key.primary_key.created_at().as_secs() as i64,
        can_encrypt: encryption_capable(key),
    }
}

/// Makes a key for `name <email>`; returns the armored secret key and its public part.
pub fn generate(name: &str, email: &str) -> Result<(String, SignedPublicKey)> {
    let uid = if name.trim().is_empty() {
        format!("<{}>", email.trim())
    } else {
        format!("{} <{}>", name.trim(), email.trim())
    };
    let mut enc = SubkeyParamsBuilder::default();
    enc.key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy))
        .can_sign(false)
        .can_encrypt(EncryptionCaps::All)
        .can_authenticate(false);
    let mut params = SecretKeyParamsBuilder::default();
    params
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .can_encrypt(EncryptionCaps::None)
        .primary_user_id(uid)
        // Without these, GnuPG falls back to SHA-1, which Ed25519 signatures must not use.
        .preferred_symmetric_algorithms(vec![SymmetricKeyAlgorithm::AES256, SymmetricKeyAlgorithm::AES128].into())
        .preferred_hash_algorithms(vec![HashAlgorithm::Sha256, HashAlgorithm::Sha512, HashAlgorithm::Sha384].into())
        .preferred_compression_algorithms(vec![CompressionAlgorithm::ZLIB, CompressionAlgorithm::ZIP].into())
        .subkeys(vec![enc.build().map_err(pgp_error)?]);
    let key = params.build().map_err(pgp_error)?.generate(rand::thread_rng()).map_err(pgp_error)?;
    let armored = key.to_armored_string(ArmorOptions::default()).map_err(pgp_error)?;
    Ok((armored, key.to_public_key()))
}

/// Reads an armored or binary public key (or the public part of a secret key).
pub fn read_public(data: &[u8]) -> Result<SignedPublicKey> {
    let text = String::from_utf8_lossy(data);
    if text.contains("-----BEGIN PGP PRIVATE KEY BLOCK-----") {
        return Ok(read_secret(&text)?.to_public_key());
    }
    let key = if text.contains("-----BEGIN PGP") {
        SignedPublicKey::from_string(&text).map(|(k, _)| k)
    } else {
        SignedPublicKey::from_bytes(Cursor::new(data))
    }
    .map_err(|_| Error::Invalid(tr!("Das ist kein OpenPGP-Schlüssel.", "This is no OpenPGP key.").into()))?;
    key.verify_bindings().map_err(|_| {
        Error::Invalid(
            tr!("Die Signaturen des Schlüssels stimmen nicht.", "The key's signatures do not verify.").into(),
        )
    })?;
    Ok(key)
}

/// Reads an armored secret key.
pub fn read_secret(armored: &str) -> Result<SignedSecretKey> {
    let (key, _) = SignedSecretKey::from_string(armored).map_err(|_| {
        Error::Invalid(tr!("Das ist kein geheimer OpenPGP-Schlüssel.", "This is no OpenPGP secret key.").into())
    })?;
    key.verify_bindings().map_err(pgp_error)?;
    Ok(key)
}

pub fn armored_public(key: &SignedPublicKey) -> Result<String> {
    key.to_armored_string(ArmorOptions::default()).map_err(pgp_error)
}

pub fn binary_public(key: &SignedPublicKey) -> Result<Vec<u8>> {
    key.to_bytes().map_err(pgp_error)
}

/// A detached signature (armored) over `data` with the secret key.
pub fn sign_detached(secret: &SignedSecretKey, data: &[u8]) -> Result<String> {
    let sig = DetachedSignature::sign_binary_data(
        rand::thread_rng(),
        &secret.primary_key,
        &Password::empty(),
        HashAlgorithm::Sha256,
        data,
    )
    .map_err(pgp_error)?;
    sig.to_armored_string(ArmorOptions::default()).map_err(pgp_error)
}

/// What a verification found: the key that made a good signature, or why none did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Valid {
        fingerprint: String,
        signer: String,
    },
    /// No known key made it; the signature names this key.
    UnknownKey {
        fingerprint: Option<String>,
    },
    Invalid,
}

fn signer_of(key: &SignedPublicKey) -> String {
    let i = info(key);
    if i.name.is_empty() { i.email } else { format!("{} <{}>", i.name, i.email) }
}

/// The verifying keys of a certificate: the primary and its subkeys.
fn verifies(key: &SignedPublicKey, check: impl Fn(&dyn VerifyingKey) -> bool) -> bool {
    check(key) || key.public_subkeys.iter().any(|s| check(s))
}

/// Checks a detached signature (armored or binary) over `data` against `keys`.
pub fn verify_detached(signature: &[u8], data: &[u8], keys: &[SignedPublicKey]) -> Verdict {
    let text = String::from_utf8_lossy(signature);
    let sig = if text.contains("-----BEGIN PGP SIGNATURE-----") {
        DetachedSignature::from_string(&text).map(|(s, _)| s)
    } else {
        DetachedSignature::from_bytes(Cursor::new(signature))
    };
    let Ok(sig) = sig else { return Verdict::Invalid };
    for k in keys {
        if sig.signature.verify(k, data).is_ok()
            || k.public_subkeys.iter().any(|s| sig.signature.verify(s, data).is_ok())
        {
            return Verdict::Valid { fingerprint: fingerprint_of(k), signer: signer_of(k) };
        }
    }
    let named = sig.signature.issuer_fingerprint().first().map(|f| format!("{f:X}"));
    if keys.iter().any(|k| Some(fingerprint_of(k)) == named) {
        return Verdict::Invalid;
    }
    Verdict::UnknownKey { fingerprint: named }
}

/// Encrypts `data` (a MIME entity) to the recipients' keys, signed with `signer` when given.
/// Returns the armored message.
pub fn encrypt(data: &[u8], recipients: &[SignedPublicKey], signer: Option<&SignedSecretKey>) -> Result<String> {
    let mut rng = rand::thread_rng();
    let mut b = MessageBuilder::from_bytes("", data.to_vec()).seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    let mut any = false;
    for k in recipients {
        let mut used = false;
        for s in &k.public_subkeys {
            let flags = s.signatures.iter().any(|g| {
                let f = g.key_flags();
                f.encrypt_comms() || f.encrypt_storage()
            });
            if flags && s.key.algorithm().can_encrypt() {
                b.encrypt_to_key(&mut rng, &s.key).map_err(pgp_error)?;
                used = true;
                any = true;
                break;
            }
        }
        if !used && k.primary_key.algorithm().can_encrypt() {
            b.encrypt_to_key(&mut rng, &k.primary_key).map_err(pgp_error)?;
            any = true;
        }
    }
    if !any {
        return Err(Error::Invalid(tr!("Kein Schlüssel kann verschlüsseln.", "No key can encrypt.").into()));
    }
    if let Some(s) = signer {
        b.sign(&s.primary_key, Password::empty(), HashAlgorithm::Sha256);
    }
    b.to_armored_string(&mut rng, ArmorOptions::default()).map_err(pgp_error)
}

/// What decryption gave: the plain bytes and, when it was signed inside, the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decrypted {
    pub data: Vec<u8>,
    pub signature: Option<Verdict>,
}

/// Decrypts an armored (or binary) message with one of `secrets`, verifying a signature inside
/// against `keys`.
pub fn decrypt(message: &[u8], secrets: &[SignedSecretKey], keys: &[SignedPublicKey]) -> Result<Decrypted> {
    let not_for_me = || {
        Error::Invalid(
            tr!(
                "Diese Nachricht ist nicht für einen deiner Schlüssel verschlüsselt.",
                "This message is not encrypted for one of your keys."
            )
            .into(),
        )
    };
    if secrets.is_empty() {
        return Err(not_for_me());
    }
    let text = String::from_utf8_lossy(message).into_owned();
    let parse = || -> Result<Message<'_>> {
        if text.contains("-----BEGIN PGP MESSAGE-----") {
            Message::from_string(&text).map(|(m, _)| m).map_err(pgp_error)
        } else {
            Message::from_bytes(Cursor::new(message)).map_err(pgp_error)
        }
    };
    let msg = parse()?;
    if !msg.is_encrypted() {
        return Err(pgp_error("not encrypted"));
    }
    let pw = Password::empty();
    let mut dec = msg
        .decrypt_with_keys(secrets.iter().map(|_| &pw).collect(), secrets.iter().collect())
        .map_err(|_| not_for_me())?;
    if dec.is_compressed() {
        dec = dec.decompress().map_err(pgp_error)?;
    }
    let data = dec.as_data_vec().map_err(pgp_error)?;
    let signature = if dec.is_signed() {
        let mut verdict = Verdict::UnknownKey { fingerprint: None };
        for k in keys {
            if verifies(k, |v| dec.verify(v).is_ok()) {
                verdict = Verdict::Valid { fingerprint: fingerprint_of(k), signer: signer_of(k) };
                break;
            }
        }
        Some(verdict)
    } else {
        None
    };
    Ok(Decrypted { data, signature })
}

/// Checks a cleartext-signed text (`-----BEGIN PGP SIGNED MESSAGE-----`); returns the text
/// without its armor and the verdict.
pub fn verify_cleartext(text: &str, keys: &[SignedPublicKey]) -> Option<(String, Verdict)> {
    let start = text.find("-----BEGIN PGP SIGNED MESSAGE-----")?;
    let end = text.find("-----END PGP SIGNATURE-----")? + "-----END PGP SIGNATURE-----".len();
    let (msg, _) = CleartextSignedMessage::from_string(&text[start..end]).ok()?;
    let plain = msg.text().to_string();
    for k in keys {
        if msg.verify(k).is_ok() || k.public_subkeys.iter().any(|s| msg.verify(s).is_ok()) {
            return Some((plain, Verdict::Valid { fingerprint: fingerprint_of(k), signer: signer_of(k) }));
        }
    }
    let named = msg.signatures().first().and_then(|s| s.issuer_fingerprint().first().map(|f| format!("{f:X}")));
    Some((plain, Verdict::UnknownKey { fingerprint: named }))
}

// ---------------------------------------------------------------------------- Autocrypt

/// The `Autocrypt:` header value for the user's key (Autocrypt 1.1).
pub fn autocrypt_header(email: &str, key: &SignedPublicKey, mutual: bool) -> Result<String> {
    let data = base64::engine::general_purpose::STANDARD.encode(binary_public(key)?);
    let mut out = format!("addr={}; ", email.trim().to_ascii_lowercase());
    if mutual {
        out.push_str("prefer-encrypt=mutual; ");
    }
    out.push_str("keydata=");
    // Folded lines of 76 characters, as the header allows.
    for (i, chunk) in data.as_bytes().chunks(76).enumerate() {
        if i > 0 {
            out.push_str("\r\n ");
        }
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
    }
    Ok(out)
}

/// What an Autocrypt header says: the address, whether the sender prefers encryption, the key.
pub fn parse_autocrypt(value: &str) -> Option<(String, bool, SignedPublicKey)> {
    let mut addr = None;
    let mut mutual = false;
    let mut keydata = None;
    for part in value.split(';') {
        let (k, v) = part.split_once('=')?;
        match k.trim() {
            "addr" => addr = Some(v.trim().to_ascii_lowercase()),
            "prefer-encrypt" => mutual = v.trim() == "mutual",
            "keydata" => keydata = Some(v.chars().filter(|c| !c.is_whitespace()).collect::<String>()),
            k if !k.starts_with('_') => return None,
            _ => {}
        }
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(keydata?).ok()?;
    let key = read_public(&bytes).ok()?;
    let addr = addr?;
    // The key must be for the address it is sent for.
    users(&key).iter().any(|(_, e)| *e == addr).then_some((addr, mutual, key))
}
