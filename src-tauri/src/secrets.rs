//! Passwords and OAuth refresh tokens in the OS credential store (service
//! `mail_core::account::CREDENTIAL_SERVICE`, account names from `mail_core::account::secret_key`).
//! Nothing secret is written to the database, the settings or a log.

use mail_core::Error;
use mail_core::account::CREDENTIAL_SERVICE;

#[cfg(not(target_os = "android"))]
fn entry(account: &str) -> Result<keyring::Entry, Error> {
    keyring::Entry::new(CREDENTIAL_SERVICE, account).map_err(store_error)
}

#[cfg(not(target_os = "android"))]
fn store_error(e: keyring::Error) -> Error {
    Error::Secrets(mail_core::trf!(
        "Der Schlüsselbund des Systems hat abgelehnt: {e}",
        "The system's credential store refused: {e}"
    ))
}

#[cfg(not(target_os = "android"))]
pub fn set(account: &str, secret: &str) -> Result<(), Error> {
    entry(account)?.set_password(secret).map_err(store_error)
}

#[cfg(not(target_os = "android"))]
pub fn get(account: &str) -> Result<Option<String>, Error> {
    match entry(account)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(store_error(e)),
    }
}

#[cfg(not(target_os = "android"))]
pub fn delete(account: &str) -> Result<(), Error> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(store_error(e)),
    }
}

#[cfg(target_os = "android")]
fn unavailable() -> Error {
    Error::Secrets(
        mail_core::tr!(
            "Auf Android können noch keine Zugangsdaten gespeichert werden.",
            "Sign-in details cannot be stored on Android yet."
        )
        .into(),
    )
}

#[cfg(target_os = "android")]
pub fn set(_account: &str, _secret: &str) -> Result<(), Error> {
    Err(unavailable())
}

#[cfg(target_os = "android")]
pub fn get(_account: &str) -> Result<Option<String>, Error> {
    Ok(None)
}

#[cfg(target_os = "android")]
pub fn delete(_account: &str) -> Result<(), Error> {
    Ok(())
}
