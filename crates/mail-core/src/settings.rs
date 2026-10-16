//! Application settings: one JSON object in the `settings` table (key `app`), with a top-level
//! `version` ([`crate::settings_migrate::SETTINGS_VERSION`]). Unknown keys are kept, missing
//! ones take their defaults, so an older or newer version can open the same settings. A change
//! applies at once; the UI offers „Rückgängig“ by writing the previous value back.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::Result;
use crate::settings_migrate::{SETTINGS_VERSION, migrate};
use crate::store::Store;

const KEY: &str = "app";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageChoice {
    #[default]
    System,
    De,
    En,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
    Contrast,
}

/// Whether remote images load without asking. Off by default: they reveal that and when a
/// message was read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteContent {
    #[default]
    Ask,
    /// Load for senders the user allowed (milestone 1).
    TrustedSenders,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub language: LanguageChoice,
    pub theme: Theme,
    pub remote_content: RemoteContent,
    /// Group messages of a conversation in the list.
    pub threads: bool,
    /// Mark a message read after it was shown this many seconds (0: at once; `None`: never).
    pub mark_read_after_secs: Option<u32>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: SETTINGS_VERSION,
            language: LanguageChoice::System,
            theme: Theme::System,
            remote_content: RemoteContent::Ask,
            threads: true,
            mark_read_after_secs: Some(0),
        }
    }
}

/// Reads the settings, running the migration steps on the raw JSON first.
pub fn load(store: &Store) -> Result<Settings> {
    let Some(raw) = store.setting(KEY)? else { return Ok(Settings::default()) };
    let mut map: Map<String, Value> = serde_json::from_str(&raw).unwrap_or_default();
    migrate(&mut map);
    Ok(serde_json::from_value(Value::Object(map)).unwrap_or_default())
}

/// Writes the settings, keeping keys this version does not know.
pub fn save(store: &Store, settings: &Settings) -> Result<()> {
    let mut map: Map<String, Value> =
        store.setting(KEY)?.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    if let Value::Object(new) = serde_json::to_value(settings)? {
        map.extend(new);
    }
    map.insert("version".into(), SETTINGS_VERSION.max(settings.version).into());
    store.set_setting(KEY, &Value::Object(map).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_unknown_keys_stay() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(load(&s).unwrap(), Settings::default());
        s.set_setting(KEY, r#"{"version":0,"theme":"dark","from_the_future":42}"#).unwrap();
        let mut loaded = load(&s).unwrap();
        assert_eq!(loaded.theme, Theme::Dark);
        assert!(loaded.threads, "missing keys take their defaults");
        loaded.remote_content = RemoteContent::TrustedSenders;
        save(&s, &loaded).unwrap();
        let raw = s.setting(KEY).unwrap().unwrap();
        assert!(raw.contains("from_the_future"), "{raw}");
        assert_eq!(load(&s).unwrap().remote_content, RemoteContent::TrustedSenders);
    }
}
