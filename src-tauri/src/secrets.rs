//! Passwords and OAuth refresh tokens in the platform's credential store (service
//! `mail_core::account::CREDENTIAL_SERVICE`, account names from `mail_core::account::secret_key`):
//! Windows Credential Manager, the macOS Keychain, the Secret Service on Linux (GNOME Keyring,
//! KWallet, KeePassXC), the iOS Keychain and the Android Keystore. Nothing secret is written to
//! the database, the settings or a log.
//!
//! As in Arcalo: when no Secret Service answers on Linux (a desktop without a keyring, a
//! headless machine, `ARCALO_MAIL_SECRET_STORE=file`), the secrets go to `secrets.json` in the
//! app's data folder, readable by the user only; Settings → Datenschutz says which store is
//! used.
//!
//! Android, as Arcalo's companion app: an AES key that is created in the Android Keystore and
//! never leaves it encrypts each secret (`KeystoreCipher.kt`, reached through
//! `mobile::android`); only the ciphertexts are kept, in `secrets.sealed.json` in the app's
//! private folder. A value that cannot be decrypted any more (the key is gone after a restore
//! onto another device) reads as missing, so the account asks for its sign-in again.
//!
//! iOS: the Keychain, with items readable after the first unlock since the start of the phone
//! and never moved to another device (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`), so the
//! background check can sign in while the phone is locked.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use mail_core::Error;
use serde::Serialize;

/// The store this process uses (decided once at start).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    /// Credential Manager, the Keychain (macOS, iOS) or the Secret Service.
    System,
    /// `secrets.json`, owner-only; `reason`: why the system store is not used.
    File {
        reason: String,
    },
    /// Android: encrypted with a key of the Android Keystore.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Keystore,
    Unavailable,
}

/// Turns a secret into a ciphertext and back (Android: a key of the Keystore).
pub trait Cipher: Send + Sync {
    fn seal(&self, plain: &str) -> Result<String, String>;
    fn open(&self, sealed: &str) -> Result<String, String>;
}

/// Where secrets are kept.
trait Vault: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// A JSON file in the app's data folder, readable by the user only: the values as they are
/// (the Linux fallback) or sealed by a [`Cipher`] (Android).
struct FileVault {
    file: PathBuf,
    cipher: Option<Box<dyn Cipher>>,
    lock: Mutex<()>,
}

impl FileVault {
    fn read(&self) -> serde_json::Map<String, serde_json::Value> {
        std::fs::read_to_string(&self.file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn write(&self, map: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &self.file).map_err(|e| e.to_string())
    }
}

impl Vault for FileVault {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(stored) = self.read().get(account).and_then(|v| v.as_str()).map(str::to_string) else {
            return Ok(None);
        };
        match &self.cipher {
            None => Ok(Some(stored)),
            Some(c) => match c.open(&stored) {
                Ok(plain) => Ok(Some(plain).filter(|s| !s.is_empty())),
                Err(e) => {
                    // Unreadable (the key is gone): treated as not set, the account asks again.
                    mail_core::diag::log("secrets", &format!("{account} cannot be decrypted: {e}"));
                    Ok(None)
                }
            },
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let value = match &self.cipher {
            None => secret.to_string(),
            Some(c) => c.seal(secret)?,
        };
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.read();
        map.insert(account.into(), value.into());
        self.write(&map)
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.read();
        if map.remove(account).is_some() {
            self.write(&map)?;
        }
        Ok(())
    }
}

/// Windows, macOS and Linux: the `keyring` crate.
#[cfg(desktop)]
struct KeyringVault;

#[cfg(desktop)]
impl KeyringVault {
    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(mail_core::account::CREDENTIAL_SERVICE, account).map_err(|e| e.to_string())
    }
}

#[cfg(desktop)]
impl Vault for KeyringVault {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        match Self::entry(account)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        Self::entry(account)?.set_password(secret).map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// iOS: generic passwords in the Keychain, readable after the first unlock (background check)
/// and bound to this device.
#[cfg(target_os = "ios")]
struct KeychainVault;

#[cfg(target_os = "ios")]
impl KeychainVault {
    fn options(account: &str) -> security_framework::passwords::PasswordOptions {
        security_framework::passwords::PasswordOptions::new_generic_password(
            mail_core::account::CREDENTIAL_SERVICE,
            account,
        )
    }
}

/// `errSecItemNotFound`.
#[cfg(target_os = "ios")]
const NOT_FOUND: i32 = -25300;

#[cfg(target_os = "ios")]
impl Vault for KeychainVault {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        match security_framework::passwords::generic_password(Self::options(account)) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        use security_framework::access_control::{ProtectionMode, SecAccessControl};
        // Replaced as a whole, so an item of another accessibility never stays behind.
        self.delete(account)?;
        let mut options = Self::options(account);
        let access =
            SecAccessControl::create_with_protection(Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly), 0)
                .map_err(|e| e.to_string())?;
        options.set_access_control(access);
        security_framework::passwords::set_generic_password_options(secret.as_bytes(), options)
            .map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match security_framework::passwords::delete_generic_password_options(Self::options(account)) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == NOT_FOUND => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Nothing can be stored.
struct NoVault;

impl Vault for NoVault {
    fn get(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Err(mail_core::tr!(
            "Auf diesem Gerät können keine Zugangsdaten gespeichert werden.",
            "Sign-in details cannot be stored on this device."
        )
        .into())
    }
    fn delete(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

struct Backend {
    kind: Kind,
    vault: Box<dyn Vault>,
}

static BACKEND: OnceLock<Backend> = OnceLock::new();

fn store_error(e: impl std::fmt::Display) -> Error {
    Error::Secrets(mail_core::trf!(
        "Der Schlüsselbund des Systems hat abgelehnt: {e}",
        "The system's credential store refused: {e}"
    ))
}

/// Looks up an entry that never exists: „not found“ means the Secret Service answers. In a
/// thread with a timeout, so a hanging D-Bus never blocks the start.
#[cfg(target_os = "linux")]
fn probe() -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("secret-probe".into())
        .spawn(move || {
            let r = keyring::Entry::new(mail_core::account::CREDENTIAL_SERVICE, "arcalo-mail-probe")
                .and_then(|e| e.get_password().map(drop));
            let _ = tx.send(match r {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            });
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(3))
        .unwrap_or_else(|_| Err("the Secret Service did not answer within 3 s".into()))
}

#[cfg_attr(target_os = "ios", allow(dead_code))]
fn file_vault(data_dir: &Path, cipher: Option<Box<dyn Cipher>>) -> Box<dyn Vault> {
    let name = if cipher.is_some() { "secrets.sealed.json" } else { "secrets.json" };
    Box::new(FileVault { file: data_dir.join(name), cipher, lock: Mutex::new(()) })
}

/// The store of this platform.
fn choose(data_dir: &Path) -> (Kind, Box<dyn Vault>) {
    #[cfg(target_os = "android")]
    {
        let cipher: Box<dyn Cipher> = Box::new(crate::mobile::android::KeystoreCipher);
        (Kind::Keystore, file_vault(data_dir, Some(cipher)))
    }
    #[cfg(target_os = "ios")]
    {
        let _ = data_dir;
        (Kind::System, Box::new(KeychainVault))
    }
    #[cfg(desktop)]
    {
        let forced = std::env::var("ARCALO_MAIL_SECRET_STORE").ok().filter(|v| v.eq_ignore_ascii_case("file"));
        if forced.is_some() {
            return (Kind::File { reason: "ARCALO_MAIL_SECRET_STORE=file".into() }, file_vault(data_dir, None));
        }
        #[cfg(target_os = "linux")]
        if let Err(reason) = probe() {
            return (Kind::File { reason }, file_vault(data_dir, None));
        }
        (Kind::System, Box::new(KeyringVault))
    }
}

/// Decides the store; called once at start with the app's data folder (again by the phones'
/// background check, which may run without the app: then the first call counts).
pub fn init(data_dir: &Path) {
    if BACKEND.get().is_some() {
        return;
    }
    let (kind, vault) = choose(data_dir);
    let _ = BACKEND.set(Backend { kind, vault });
}

pub fn kind() -> Kind {
    BACKEND.get().map_or(Kind::Unavailable, |b| b.kind.clone())
}

fn backend() -> &'static Backend {
    static NONE: OnceLock<Backend> = OnceLock::new();
    BACKEND.get().unwrap_or_else(|| NONE.get_or_init(|| Backend { kind: Kind::Unavailable, vault: Box::new(NoVault) }))
}

pub fn set(account: &str, secret: &str) -> Result<(), Error> {
    backend().vault.set(account, secret).map_err(store_error)
}

pub fn get(account: &str) -> Result<Option<String>, Error> {
    backend().vault.get(account).map_err(store_error)
}

pub fn delete(account: &str) -> Result<(), Error> {
    backend().vault.delete(account).map_err(store_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A stand-in for the Keystore: reversible, recognizable, and it can lose its key.
    #[derive(Clone, Default)]
    struct FakeCipher {
        key_gone: Arc<AtomicBool>,
        refuse: Arc<AtomicBool>,
    }

    impl Cipher for FakeCipher {
        fn seal(&self, plain: &str) -> Result<String, String> {
            if self.refuse.load(Ordering::SeqCst) {
                return Err("keystore locked".into());
            }
            Ok(format!("sealed:{}", plain.chars().rev().collect::<String>()))
        }
        fn open(&self, sealed: &str) -> Result<String, String> {
            if self.key_gone.load(Ordering::SeqCst) {
                return Err("key permanently invalidated".into());
            }
            sealed.strip_prefix("sealed:").map(|s| s.chars().rev().collect()).ok_or_else(|| "not ours".into())
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("arcalo-mail-secrets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sealed_values_never_reach_the_file_in_plain_text() {
        let d = dir("sealed");
        let cipher = FakeCipher::default();
        let v = file_vault(&d, Some(Box::new(cipher.clone())));
        assert_eq!(v.get("account-1-password").unwrap(), None);
        v.set("account-1-password", "geheim").unwrap();
        v.set("account-1-oauth-refresh-token", "rt-1").unwrap();
        assert_eq!(v.get("account-1-password").unwrap().as_deref(), Some("geheim"));
        let raw = std::fs::read_to_string(d.join("secrets.sealed.json")).unwrap();
        assert!(!raw.contains("geheim") && !raw.contains("rt-1"), "{raw}");
        assert!(raw.contains("sealed:mieheg"), "{raw}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(d.join("secrets.sealed.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // A rotated refresh token replaces the old one; removing an account removes its entries.
        v.set("account-1-oauth-refresh-token", "rt-2").unwrap();
        assert_eq!(v.get("account-1-oauth-refresh-token").unwrap().as_deref(), Some("rt-2"));
        v.delete("account-1-password").unwrap();
        v.delete("account-1-password").unwrap();
        assert_eq!(v.get("account-1-password").unwrap(), None);
        assert_eq!(v.get("account-1-oauth-refresh-token").unwrap().as_deref(), Some("rt-2"), "the other stays");

        // A new vault on the same folder (the app started again, or the background check).
        let again = file_vault(&d, Some(Box::new(cipher.clone())));
        assert_eq!(again.get("account-1-oauth-refresh-token").unwrap().as_deref(), Some("rt-2"));
    }

    #[test]
    fn a_lost_key_reads_as_missing_and_a_refusal_is_an_error() {
        let d = dir("lost");
        let cipher = FakeCipher::default();
        let v = file_vault(&d, Some(Box::new(cipher.clone())));
        v.set("account-2-password", "pw").unwrap();
        cipher.key_gone.store(true, Ordering::SeqCst);
        assert_eq!(v.get("account-2-password").unwrap(), None, "asks for the sign-in again");
        cipher.key_gone.store(false, Ordering::SeqCst);
        cipher.refuse.store(true, Ordering::SeqCst);
        assert!(v.set("account-2-password", "new").is_err(), "not reported as saved");
        assert_eq!(v.get("account-2-password").unwrap().as_deref(), Some("pw"), "the old value is untouched");
    }

    #[test]
    fn the_plain_file_of_linux_desktops_without_a_keyring() {
        let d = dir("plain");
        let v = file_vault(&d, None);
        v.set("proxy-standard-password", "p").unwrap();
        assert_eq!(v.get("proxy-standard-password").unwrap().as_deref(), Some("p"));
        assert!(d.join("secrets.json").exists());
        v.delete("proxy-standard-password").unwrap();
        assert_eq!(v.get("proxy-standard-password").unwrap(), None);
    }

    #[test]
    fn without_a_store_nothing_is_kept() {
        assert!(NoVault.set("a", "b").is_err());
        assert_eq!(NoVault.get("a").unwrap(), None);
        assert!(NoVault.delete("a").is_ok());
    }
}
