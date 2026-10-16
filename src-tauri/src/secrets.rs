//! Passwords and OAuth refresh tokens in the OS credential store (service
//! `mail_core::account::CREDENTIAL_SERVICE`, account names from `mail_core::account::secret_key`):
//! Windows Credential Manager, macOS Keychain, the Secret Service on Linux (GNOME Keyring,
//! KWallet, KeePassXC). Nothing secret is written to the database, the settings or a log.
//!
//! As in Arcalo: when no Secret Service answers on Linux (a desktop without a keyring, a
//! headless machine, `ARCALO_MAIL_SECRET_STORE=file`), the secrets go to `secrets.json` in the
//! app's data folder, readable by the user only; Settings → Datenschutz says which store is
//! used. Android gets the Keystore in milestone 3; until then nothing is stored there.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use mail_core::Error;
use serde::Serialize;

/// The store this process uses (decided once at start).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    /// Credential Manager, Keychain or the Secret Service.
    System,
    /// `secrets.json`, owner-only; `reason`: why the system store is not used.
    File {
        reason: String,
    },
    Unavailable,
}

struct Backend {
    kind: Kind,
    file: PathBuf,
    lock: Mutex<()>,
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

/// Decides the store; called once at start with the app's data folder.
pub fn init(data_dir: &Path) {
    let forced = std::env::var("ARCALO_MAIL_SECRET_STORE").ok().filter(|v| v.eq_ignore_ascii_case("file"));
    let kind = if cfg!(target_os = "android") {
        Kind::Unavailable
    } else if forced.is_some() {
        Kind::File { reason: "ARCALO_MAIL_SECRET_STORE=file".into() }
    } else {
        #[cfg(target_os = "linux")]
        {
            match probe() {
                Ok(()) => Kind::System,
                Err(reason) => Kind::File { reason },
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Kind::System
        }
    };
    let _ = BACKEND.set(Backend { kind, file: data_dir.join("secrets.json"), lock: Mutex::new(()) });
}

pub fn kind() -> Kind {
    BACKEND.get().map_or(Kind::Unavailable, |b| b.kind.clone())
}

fn backend() -> Result<&'static Backend, Error> {
    BACKEND.get().ok_or_else(|| store_error("not initialized"))
}

fn read_file(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_file(path: &Path, map: &serde_json::Map<String, serde_json::Value>) -> Result<(), Error> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(map).map_err(store_error)?).map_err(store_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(store_error)?;
    }
    std::fs::rename(&tmp, path).map_err(store_error)
}

#[cfg(not(target_os = "android"))]
fn entry(account: &str) -> Result<keyring::Entry, Error> {
    keyring::Entry::new(mail_core::account::CREDENTIAL_SERVICE, account).map_err(store_error)
}

fn unavailable() -> Error {
    Error::Secrets(
        mail_core::tr!(
            "Auf diesem Gerät können noch keine Zugangsdaten gespeichert werden.",
            "Sign-in details cannot be stored on this device yet."
        )
        .into(),
    )
}

pub fn set(account: &str, secret: &str) -> Result<(), Error> {
    let b = backend()?;
    match &b.kind {
        #[cfg(not(target_os = "android"))]
        Kind::System => entry(account)?.set_password(secret).map_err(store_error),
        Kind::File { .. } => {
            let _g = b.lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut map = read_file(&b.file);
            map.insert(account.into(), secret.into());
            write_file(&b.file, &map)
        }
        _ => Err(unavailable()),
    }
}

pub fn get(account: &str) -> Result<Option<String>, Error> {
    let b = backend()?;
    match &b.kind {
        #[cfg(not(target_os = "android"))]
        Kind::System => match entry(account)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_error(e)),
        },
        Kind::File { .. } => {
            let _g = b.lock.lock().unwrap_or_else(|e| e.into_inner());
            Ok(read_file(&b.file).get(account).and_then(|v| v.as_str()).map(str::to_string))
        }
        _ => Ok(None),
    }
}

pub fn delete(account: &str) -> Result<(), Error> {
    let b = backend()?;
    match &b.kind {
        #[cfg(not(target_os = "android"))]
        Kind::System => match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_error(e)),
        },
        Kind::File { .. } => {
            let _g = b.lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut map = read_file(&b.file);
            if map.remove(account).is_some() {
                write_file(&b.file, &map)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
