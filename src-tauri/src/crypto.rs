//! The commands of signed and encrypted mail: the user's OpenPGP keys (made here or imported;
//! the secret part only in the credential store), other people's keys (imported, looked up in
//! their domain's Web Key Directory), the user's S/MIME certificate from a PKCS#12 file (file and
//! password in the credential store), what can be done for a message's recipients, and the files
//! inside an encrypted message. The core reads the secrets through [`ShellKeys`].

use base64::Engine;
use mail_core::Error;
use mail_core::crypto::{self, Abilities, Keys, StoredKey, pgp, smime};
use tauri::State;

use crate::{AppState, secrets};

type Res<T> = Result<T, Error>;

/// The credential store as the core's [`Keys`].
pub struct ShellKeys;

impl Keys for ShellKeys {
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

/// Imports keys from text (armored public keys of others, or an own secret key).
#[tauri::command]
pub fn pgp_import(state: State<'_, AppState>, text: String) -> Res<Vec<pgp::KeyInfo>> {
    let text = text.trim();
    if text.contains("-----BEGIN PGP PRIVATE KEY BLOCK-----") {
        let secret = pgp::read_secret(text)?;
        let public = secret.to_public_key();
        let fp = pgp::fingerprint_of(&public);
        secrets::set(&crypto::pgp_secret_key(&fp), text)?;
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
    let info = crypto::save_cert(&state.store(), &id.cert, true)?;
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
    let raw = store.raw_message(message_id)?.ok_or_else(|| Error::NotFound("message".into()))?;
    let sender = store.message(message_id)?.summary.from.map(|a| a.email);
    let opened = crypto::open(&store, &raw, sender.as_deref(), &ShellKeys).and_then(|o| o.entity).ok_or_else(|| {
        Error::Invalid(mail_core::tr!("Die Nachricht lässt sich nicht öffnen.", "The message cannot be opened.").into())
    })?;
    let data = mail_core::parse::attachment_data(&opened, index).ok_or_else(|| Error::NotFound("attachment".into()))?;
    std::fs::write(&path, data).map_err(|e| Error::Invalid(e.to_string()))
}
