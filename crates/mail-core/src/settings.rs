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
    /// Blocked; „Bilder laden“ per message, „Immer für diesen Absender“ per sender.
    #[default]
    Ask,
    /// The same, named before senders could be trusted one by one (kept for old settings).
    TrustedSenders,
    /// Always loaded (the sender sees when the message was read).
    Always,
}

/// Which bodies are fetched ahead after a sync (for the list's first lines, search and reading
/// offline); the others when a message is opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Prefetch {
    /// Only what is opened.
    None,
    /// The newest 60 messages per folder up to 256 KB each.
    #[default]
    Recent,
    /// Everything of the last 30 days up to 2 MB each.
    Month,
}

/// What a swipe over a row of the list does on a touch screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwipeAction {
    #[default]
    Archive,
    Delete,
    /// Read or unread, whichever the message is not.
    Read,
    Flag,
    None,
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
    /// A desktop notification for new mail in an inbox.
    pub notifications: bool,
    /// Seconds a sent message waits for „Rückgängig“ before it goes out (0: at once).
    pub undo_send_secs: u32,
    /// New messages start as rich text (bold, lists, links); off: plain text.
    pub compose_rich_text: bool,
    /// The preview line under the subject in the list.
    pub show_snippets: bool,
    pub prefetch: Prefetch,
    /// Headers a first sync fetches per folder, the newest first (a large mailbox is usable
    /// within seconds); the older mail follows in the background (`sync::backfill`).
    pub initial_headers: u32,
    /// Replies without reply headers join their conversation by subject (`thread.rs`).
    pub subject_threads: bool,
    /// Proxy profiles and which service uses which (`network::NetworkSettings`).
    pub network: crate::network::NetworkSettings,
    /// Phones and tablets: a swipe from right to left over a row of the list.
    pub swipe_left: SwipeAction,
    /// Phones and tablets: a swipe from left to right.
    pub swipe_right: SwipeAction,
    /// Phones: look for new mail in the background (Android about every 15 minutes, iOS when
    /// the system allows) and tell about it.
    pub background_check: bool,
    /// The Screener: mail from first-time senders waits in „Neue Absender“ until the user lets
    /// the sender in or turns it away (`triage`). Off by default.
    pub screener: bool,
    /// When the Screener was switched on (Unix seconds): mail from before stays where it is.
    pub screener_since: Option<i64>,
    /// Focus: the inbox shows people and pinned messages only, and only they notify.
    pub focus: bool,
    /// With two accounts or more, the inbox of all accounts („Alle Posteingänge“) opens first.
    pub unified_inbox: bool,
    /// The newsletters tab shows one row per sender, opened as a stream of its issues.
    pub bundles: bool,
    /// The AI assistant: providers, which account's mail each may see, the automatic parts.
    pub ai: crate::ai::AiSettings,
    /// Profile pictures from BIMI: a sender's verified brand logo, fetched once per organizational
    /// domain, only for senders whose mail passed DMARC at the user's provider (`avatars::lookup`).
    pub bimi: bool,
    /// Profile pictures from Gravatar. Off by default: it tells Gravatar which addresses write
    /// to the user.
    pub gravatar: bool,
    /// The calendar: where new appointments go, their length, the work week.
    pub calendar: CalendarSettings,
    /// Mail from an address with an own OpenPGP key carries it in an `Autocrypt` header, so
    /// replies can be encrypted.
    pub autocrypt: bool,
    /// New messages are signed when there is an own key or certificate (`pgp`, `smime`, none).
    pub sign_by_default: Option<crate::crypto::Method>,
    /// „An Arcalo senden“: the mail becomes a page or a task in Arcalo (through its `arcalo`
    /// command), else a Markdown file is saved.
    pub arcalo_target: ArcaloTarget,
    /// Desktop updates: downloaded in the background and installed on quit, or only announced.
    pub updates: crate::update::UpdateMode,
    /// „Startanimation“: the mark draws itself while the app loads (never longer than that;
    /// skipped with reduced motion).
    pub startup_animation: bool,
    /// The phishing warning above a message that looks like it is not from whom it claims to be
    /// (`phishing`). On by default; it never blocks anything.
    pub phishing_warning: bool,
}

/// The calendar's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarSettings {
    /// Where new appointments go (a calendar id); the first writable one when it is gone.
    pub default_calendar: Option<i64>,
    /// Length of an appointment made with a double click or N, in minutes.
    pub entry_minutes: u32,
    /// The days of the work week, 1 (Monday) to 7.
    pub workdays: Vec<u8>,
}

impl Default for CalendarSettings {
    fn default() -> Self {
        CalendarSettings { default_calendar: None, entry_minutes: 30, workdays: vec![1, 2, 3, 4, 5] }
    }
}

/// What „An Arcalo senden“ makes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcaloTarget {
    #[default]
    Page,
    Task,
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
            notifications: true,
            undo_send_secs: 5,
            compose_rich_text: true,
            show_snippets: true,
            prefetch: Prefetch::Recent,
            initial_headers: crate::sync::FIRST_HEADERS,
            subject_threads: true,
            network: crate::network::NetworkSettings::default(),
            swipe_left: SwipeAction::Archive,
            swipe_right: SwipeAction::Read,
            background_check: true,
            screener: false,
            screener_since: None,
            focus: false,
            unified_inbox: true,
            bundles: true,
            ai: crate::ai::AiSettings::default(),
            bimi: true,
            gravatar: false,
            calendar: CalendarSettings::default(),
            autocrypt: true,
            sign_by_default: None,
            arcalo_target: ArcaloTarget::Page,
            updates: crate::update::UpdateMode::Auto,
            startup_animation: true,
            phishing_warning: true,
        }
    }
}

impl Settings {
    /// Notes when the Screener was switched on (it holds back mail from then on), and forgets it
    /// when the Screener goes off.
    pub fn mark_screener(&mut self, at: i64) {
        match (self.screener, self.screener_since) {
            (true, None) => self.screener_since = Some(at),
            (false, Some(_)) => self.screener_since = None,
            _ => {}
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
    fn the_screener_remembers_when_it_started() {
        let mut s = Settings { screener: true, ..Settings::default() };
        s.mark_screener(100);
        s.mark_screener(200);
        assert_eq!(s.screener_since, Some(100), "kept while on");
        s.screener = false;
        s.mark_screener(300);
        assert_eq!(s.screener_since, None);
    }

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
