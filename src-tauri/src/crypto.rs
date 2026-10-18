//! The commands of signed and encrypted mail: the user's OpenPGP keys (made here or imported;
//! the secret part only in the credential store), other people's keys (imported, looked up in
//! their domain's Web Key Directory), the user's S/MIME certificate from a PKCS#12 file (file and
//! password in the credential store), what can be done for a message's recipients, and the files
//! inside an encrypted message. The core reads the secrets through [`ShellKeys`].
//!
//! An own key protected with a passphrase (as GnuPG and Thunderbird export them) is kept as it
//! is. Its passphrase, once the user gave it, stays in memory for [`PASSPHRASE_TTL`] only, or in
//! the credential store when the user chose that.

use base64::Engine;
use mail_core::Error;
use mail_core::crypto::{self, Abilities, Keys, StoredKey, pgp, smime};
use tauri::State;

use crate::{AppState, secrets};

type Res<T> = Result<T, Error>;

/// How long a passphrase the user gave stays in memory.
const PASSPHRASE_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Passphrases given in this session, with when they were given.
static PASSPHRASES: std::sync::Mutex<Vec<(String, String, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());

fn remember_for_now(fingerprint: &str, passphrase: &str) {
    let mut list = PASSPHRASES.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|(fp, _, _)| fp != fingerprint);
    list.push((fingerprint.to_string(), passphrase.to_string(), std::time::Instant::now()));
}

fn forget(fingerprint: &str) {
    PASSPHRASES.lock().unwrap_or_else(|e| e.into_inner()).retain(|(fp, _, _)| fp != fingerprint);
    let _ = secrets::delete(&crypto::pgp_passphrase_key(fingerprint));
}

/// The credential store as the core's [`Keys`].
pub struct ShellKeys;

impl Keys for ShellKeys {
    fn pgp_passphrase(&self, fingerprint: &str) -> Option<String> {
        {
            let mut list = PASSPHRASES.lock().unwrap_or_else(|e| e.into_inner());
            list.retain(|(_, _, at)| at.elapsed() < PASSPHRASE_TTL);
            if let Some((_, p, _)) = list.iter().find(|(fp, _, _)| fp == fingerprint) {
                return Some(p.clone());
            }
        }
        secrets::get(&crypto::pgp_passphrase_key(fingerprint)).ok().flatten()
    }

    fn pgp_secret(&self, fingerprint: &str) -> Option<String> {
        secrets::get(&crypto::pgp_secret_key(fingerprint)).ok().flatten()
    }

    fn smime_secret(&self, fingerprint: &str) -> Option<(Vec<u8>, String)> {
        let raw = secrets::get(&crypto::smime_secret_key(fingerprint)).ok().flatten()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let p12 = base64::engine::general_purpose::STANDARD.decode(v["p12"].as_str()?).ok()?;
        Some((p12, v["password"].as_str().unwrap_or_default().to_string()))
    }
}

/// Hands the credential store to the core (at start).
pub fn register() {
    crypto::register(std::sync::Arc::new(ShellKeys));
}

#[tauri::command]
pub fn crypto_keys(state: State<'_, AppState>) -> Res<Vec<StoredKey>> {
    crypto::list(&state.store())
}

/// Makes an OpenPGP key for an account's address.
#[tauri::command]
pub fn pgp_generate(state: State<'_, AppState>, account_id: i64) -> Res<pgp::KeyInfo> {
    let account = state.store().account(account_id)?;
    let name = if account.sender_name.trim().is_empty() { account.name.clone() } else { account.sender_name.clone() };
    let (secret, public) = pgp::generate(&name, &account.email)?;
    let fp = pgp::fingerprint_of(&public);
    secrets::set(&crypto::pgp_secret_key(&fp), &secret)?;
    crypto::save_pgp(&state.store(), &public, true, "generated", true)
}

fn wrong_passphrase() -> Error {
    Error::Invalid(mail_core::tr!("Die Passphrase stimmt nicht.", "The passphrase is not right.").into())
}

/// Keeps a passphrase the user gave: in memory for a while, and in the credential store when
/// the user chose so (else an earlier stored one is removed).
fn keep_passphrase(fingerprint: &str, passphrase: &str, remember: bool) -> Res<()> {
    remember_for_now(fingerprint, passphrase);
    if remember {
        secrets::set(&crypto::pgp_passphrase_key(fingerprint), passphrase)
    } else {
        let _ = secrets::delete(&crypto::pgp_passphrase_key(fingerprint));
        Ok(())
    }
}

/// Whether the text holds an own secret key protected with a passphrase (the import dialog
/// asks for it then).
#[tauri::command]
pub fn pgp_needs_passphrase(text: String) -> bool {
    text.contains("-----BEGIN PGP PRIVATE KEY BLOCK-----")
        && pgp::read_secret(text.trim()).is_ok_and(|k| pgp::is_protected(&k))
}

/// Imports keys from text (armored public keys of others, or an own secret key; a protected
/// one with its `passphrase`, checked here, kept as `remember` says).
#[tauri::command]
pub fn pgp_import(
    state: State<'_, AppState>,
    text: String,
    passphrase: Option<String>,
    remember: Option<bool>,
) -> Res<Vec<pgp::KeyInfo>> {
    let text = text.trim();
    if text.contains("-----BEGIN PGP PRIVATE KEY BLOCK-----") {
        let secret = pgp::read_secret(text)?;
        let public = secret.to_public_key();
        let fp = pgp::fingerprint_of(&public);
        if pgp::is_protected(&secret) {
            let p = passphrase.unwrap_or_default();
            if !pgp::unlocks(&secret, &p) {
                return Err(wrong_passphrase());
            }
            secrets::set(&crypto::pgp_secret_key(&fp), text)?;
            keep_passphrase(&fp, &p, remember.unwrap_or(false))?;
        } else {
            secrets::set(&crypto::pgp_secret_key(&fp), text)?;
        }
        return Ok(vec![crypto::save_pgp(&state.store(), &public, true, "imported", true)?]);
    }
    let mut out = vec![];
    // Several keys in one text: each block on its own.
    for block in text.split("-----END PGP PUBLIC KEY BLOCK-----").map(str::trim).filter(|b| !b.is_empty()) {
        let armored = format!("{block}\n-----END PGP PUBLIC KEY BLOCK-----\n");
        let key = pgp::read_public(armored.as_bytes())?;
        out.push(crypto::save_pgp(&state.store(), &key, false, "imported", false)?);
    }
    if out.is_empty() {
        return Err(Error::Invalid(
            mail_core::tr!("Das ist kein OpenPGP-Schlüssel.", "This is no OpenPGP key.").into(),
        ));
    }
    Ok(out)
}

/// Unlocks an own protected key with its passphrase (checked here) for a while, or for good in
/// the credential store when `remember`.
#[tauri::command]
pub fn pgp_unlock(fingerprint: String, passphrase: String, remember: bool) -> Res<()> {
    let armored = secrets::get(&crypto::pgp_secret_key(&fingerprint))?
        .ok_or_else(|| Error::NotFound(mail_core::tr!("Der Schlüssel fehlt.", "The key is missing.").into()))?;
    if !pgp::unlocks(&pgp::read_secret(&armored)?, &passphrase) {
        return Err(wrong_passphrase());
    }
    keep_passphrase(&fingerprint, &passphrase, remember)
}

/// The own key of the account that needs its passphrase before it can sign (the compose sheet
/// asks for it before sending).
#[tauri::command]
pub fn pgp_locked(state: State<'_, AppState>, account_id: i64) -> Res<Option<String>> {
    let store = state.store();
    let from = store.account(account_id)?.email;
    crypto::locked_pgp(&store, &from, &ShellKeys)
}

/// Trusts the S/MIME certificate a message was signed with (the user checked it).
#[tauri::command]
pub fn smime_trust(state: State<'_, AppState>, message_id: i64) -> Res<smime::CertInfo> {
    let store = state.store();
    let raw = store.raw_message(message_id)?.ok_or_else(|| {
        Error::NotFound(mail_core::tr!("Die Nachricht gibt es nicht mehr.", "The message no longer exists.").into())
    })?;
    let sender = store.message(message_id)?.summary.from.map(|a| a.email).unwrap_or_default();
    crypto::trust_signer(&store, &raw, &sender, &ShellKeys)
}

/// The armored public key (to send or publish it).
#[tauri::command]
pub fn pgp_export(state: State<'_, AppState>, fingerprint: String) -> Res<String> {
    crypto::public_armored(&state.store(), &fingerprint)
}

/// Looks up the key of an address in its domain's Web Key Directory and keeps it.
#[tauri::command]
pub async fn pgp_lookup(state: State<'_, AppState>, email: String) -> Res<Option<pgp::KeyInfo>> {
    let urls = wkd_urls(&email)?;
    let Some(bytes) = crypto::wkd::fetch(&urls).await? else { return Ok(None) };
    let key = pgp::read_public(&bytes)?;
    if !pgp::users(&key).iter().any(|(_, e)| e.eq_ignore_ascii_case(email.trim())) {
        return Err(Error::Invalid(
            mail_core::tr!(
                "Der gefundene Schlüssel gehört zu einer anderen Adresse.",
                "The key found belongs to another address."
            )
            .into(),
        ));
    }
    Ok(Some(crypto::save_pgp(&state.store(), &key, false, "wkd", false)?))
}

/// The directory's addresses; `ARCALO_MAIL_WKD_URL` (on this computer only) stands in for the
/// domain's server in the e2e tests.
fn wkd_urls(email: &str) -> Res<Vec<String>> {
    if let Ok(base) = std::env::var("ARCALO_MAIL_WKD_URL")
        && base.starts_with("http://127.0.0.1")
    {
        let local = email.split('@').next().unwrap_or_default();
        return Ok(vec![format!("{}/{}", base.trim_end_matches('/'), mail_core::oauth::form_encode(local))]);
    }
    crypto::wkd::urls(email).map(|u| u.to_vec()).ok_or_else(|| {
        Error::Invalid(mail_core::tr!("Das ist keine E-Mail-Adresse.", "This is no e-mail address.").into())
    })
}

/// Imports the user's S/MIME certificate from a PKCS#12 file.
#[tauri::command]
pub fn smime_import(state: State<'_, AppState>, path: String, password: String) -> Res<smime::CertInfo> {
    let data = std::fs::read(&path).map_err(|e| Error::Invalid(e.to_string()))?;
    let id = smime::read_p12(&data, &password)?;
    let info = crypto::save_cert(&state.store(), &id.cert, true, true)?;
    let value =
        serde_json::json!({ "p12": base64::engine::general_purpose::STANDARD.encode(&data), "password": password });
    if let Err(e) = secrets::set(&crypto::smime_secret_key(&info.fingerprint), &value.to_string()) {
        crypto::remove(&state.store(), "smime", &info.fingerprint)?;
        return Err(e);
    }
    Ok(info)
}

#[tauri::command]
pub fn crypto_remove(state: State<'_, AppState>, kind: String, fingerprint: String) -> Res<()> {
    crypto::remove(&state.store(), &kind, &fingerprint)?;
    let key =
        if kind == "smime" { crypto::smime_secret_key(&fingerprint) } else { crypto::pgp_secret_key(&fingerprint) };
    let _ = secrets::delete(&key);
    if kind != "smime" {
        forget(&fingerprint);
    }
    Ok(())
}

/// What the compose sheet may offer for these recipients.
#[tauri::command]
pub fn crypto_abilities(state: State<'_, AppState>, account_id: i64, recipients: Vec<String>) -> Res<Abilities> {
    let store = state.store();
    let from = store.account(account_id)?.email;
    let emails: Vec<String> =
        recipients.iter().filter_map(|r| mail_core::compose::parse_address(r).ok().map(|a| a.email)).collect();
    crypto::abilities(&store, &from, &emails)
}

/// Saves a file inside an encrypted message where the user chose (decrypted again for it; the
/// UI never gets the bytes).
#[tauri::command]
pub fn save_protected_attachment(state: State<'_, AppState>, message_id: i64, index: u32, path: String) -> Res<()> {
    let store = state.store();
    let raw = store.raw_message(message_id)?.ok_or_else(|| {
        Error::NotFound(mail_core::tr!("Die Nachricht gibt es nicht mehr.", "The message no longer exists.").into())
    })?;
    let sender = store.message(message_id)?.summary.from.map(|a| a.email);
    let opened = crypto::open(&store, &raw, sender.as_deref(), &ShellKeys).and_then(|o| o.entity).ok_or_else(|| {
        Error::Invalid(mail_core::tr!("Die Nachricht lässt sich nicht öffnen.", "The message cannot be opened.").into())
    })?;
    let data = mail_core::parse::attachment_data(&opened, index).ok_or_else(|| {
        Error::NotFound(mail_core::tr!("Den Anhang gibt es nicht mehr.", "The attachment no longer exists.").into())
    })?;
    mail_core::files::write_marked(std::path::Path::new(&path), &data).map_err(|e| Error::Invalid(e.to_string()))
}
