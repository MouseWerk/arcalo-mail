//! Jonas, the other side of encrypted mail for the e2e tests: an OpenPGP key made at start,
//! published at `GET /pgp/public`; `GET /pgp/sent?i=N` opens the app's sent message N with it
//! (and checks its signature with the key from the message's Autocrypt header);
//! `POST /pgp/deliver?subject=…` (the text as the body) delivers a message from Jonas, signed
//! and encrypted to the app's key, into the inbox.

use std::collections::HashMap;

use mail_core::account::{AccountDraft, Provider, preset};
use mail_core::crypto::{self, Keys, Method, Protect, pgp};
use mail_core::store::Store;
use serde_json::json;

pub const JONAS: &str = "jonas@example.org";

struct Vault(HashMap<String, String>);

impl Keys for Vault {
    fn pgp_secret(&self, fp: &str) -> Option<String> {
        self.0.get(fp).cloned()
    }
    fn smime_secret(&self, _: &str) -> Option<(Vec<u8>, String)> {
        None
    }
}

pub struct Peer {
    store: Store,
    vault: Vault,
    public: String,
    account: mail_core::account::Account,
}

impl Peer {
    pub fn new() -> Self {
        let store = Store::open_in_memory().expect("store");
        let id = store
            .add_account(&AccountDraft {
                provider: Provider::Imap,
                name: "Jonas".into(),
                email: JONAS.into(),
                sender_name: "Jonas Berger".into(),
                transport: preset(Provider::Imap, JONAS).expect("preset"),
            })
            .expect("account");
        let account = store.account(id).expect("account");
        let (secret, public) = pgp::generate("Jonas Berger", JONAS).expect("key");
        let info = crypto::save_pgp(&store, &public, true, "generated", true).expect("save");
        let mut v = HashMap::new();
        v.insert(info.fingerprint, secret);
        Peer { store, vault: Vault(v), public: pgp::armored_public(&public).expect("armor"), account }
    }

    pub fn public(&self) -> &str {
        &self.public
    }

    /// Learns the app's key from a message's Autocrypt header.
    fn learn(&self, raw: &[u8], from: &str) {
        let _ = crypto::ingest_autocrypt(&self.store, raw, from);
    }

    /// What Jonas reads in a message the app sent.
    pub fn read(&self, raw: &[u8], from: &str) -> serde_json::Value {
        self.learn(raw, from);
        match crypto::open(&self.store, raw, Some(from), &self.vault) {
            Some(o) => {
                let text = o
                    .entity
                    .as_deref()
                    .and_then(|e| mail_core::parse::parse(e).ok())
                    .and_then(|p| p.text)
                    .unwrap_or_default();
                json!({
                    "encrypted": o.protection.encrypted,
                    "decrypted": o.protection.decrypted,
                    "signature": o.protection.signature,
                    "text": text,
                    "error": o.protection.error,
                })
            }
            None => json!({ "encrypted": false, "decrypted": false, "signature": null, "text": null, "error": null }),
        }
    }

    /// A message from Jonas to `to`, signed and encrypted to the key learned for `to`.
    pub fn letter(&self, to: &str, subject: &str, text: &str, date: i64) -> Result<Vec<u8>, String> {
        let d = mail_core::compose::Draft {
            account_id: self.account.id,
            to: vec![to.into()],
            subject: subject.into(),
            text: text.into(),
            ..Default::default()
        };
        let mid = mail_core::compose::new_message_id(JONAS);
        let raw = mail_core::compose::build(&self.account, &d, &[], &mid, date, false).map_err(|e| e.to_string())?;
        let header = crypto::autocrypt_for(&self.store, JONAS).map_err(|e| e.to_string())?.unwrap_or_default();
        let raw = crypto::add_header(&raw, "Autocrypt", &header);
        crypto::protect(
            &self.store,
            JONAS,
            &raw,
            &[to.into()],
            &Protect { method: Method::Pgp, sign: true, encrypt: true },
            &self.vault,
        )
        .map_err(|e| e.to_string())
    }

    /// Takes the app's key from the newest sent message that carries one.
    pub fn learn_from(&self, sent: &[(String, Vec<u8>)]) {
        for (from, raw) in sent.iter().rev() {
            self.learn(raw, from);
        }
    }
}

impl Default for Peer {
    fn default() -> Self {
        Self::new()
    }
}
