//! Versions of the stored settings and the ordered steps between them, as in Arcalo.
//!
//! On load, every step from the stored `version` up to [`SETTINGS_VERSION`] runs once on the raw
//! JSON, before the lenient parsing ([`crate::settings::load`]). Rules for a step: it works on
//! the JSON (never on the parsed struct, so keys this version does not know stay), it is
//! idempotent, and it changes nothing that is already in the new shape. Settings of a newer
//! version are not touched.
//!
//! Every change to the shape of [`crate::settings::Settings`] (a renamed key, a new meaning of a
//! value, a different default for existing users) adds a step here and raises the version.

use serde_json::{Map, Value};

/// Version written by this release.
pub const SETTINGS_VERSION: u32 = 8;

/// One step `from → from + 1`: changes the settings object and says what it did (`None`:
/// nothing to do for these settings).
pub struct Step {
    pub from: u32,
    pub name: &'static str,
    pub run: fn(&mut Map<String, Value>) -> Option<String>,
}

/// The chain, in order. `STEPS[i].from == i`.
pub const STEPS: [Step; SETTINGS_VERSION as usize] = [
    Step { from: 0, name: "versioned", run: versioned },
    Step { from: 1, name: "mail_basics", run: mail_basics },
    Step { from: 2, name: "fetch_threads_network", run: fetch_threads_network },
    Step { from: 3, name: "phones", run: phones },
    Step { from: 4, name: "inbox_tools", run: inbox_tools },
    Step { from: 5, name: "power_tools", run: power_tools },
    Step { from: 6, name: "calendar_crypto", run: calendar_crypto },
    Step { from: 7, name: "updates", run: updates },
];

/// Version 0 is settings written before versions existed (development builds): nothing to
/// change, the step only records the start of the chain.
fn versioned(_: &mut Map<String, Value>) -> Option<String> {
    None
}

/// Version 2 (milestone 1): notifications, undo for sending, rich text and the preview line.
/// Written with their defaults, so a later change of a default does not change them for people
/// who had version 2.
fn mail_basics(map: &mut Map<String, Value>) -> Option<String> {
    let mut added = vec![];
    for (key, value) in [
        ("notifications", Value::Bool(true)),
        ("undo_send_secs", Value::from(5)),
        ("compose_rich_text", Value::Bool(true)),
        ("show_snippets", Value::Bool(true)),
    ] {
        if !map.contains_key(key) {
            map.insert(key.into(), value);
            added.push(key);
        }
    }
    (!added.is_empty()).then(|| format!("added {}", added.join(", ")))
}

/// Version 3 (milestone 2): what is fetched ahead, conversations by subject and the network
/// profiles, written with what version 2 did (the newest 60 messages per folder, no
/// subject grouping until now: it is on for everyone, as the plan says; the system's proxy).
fn fetch_threads_network(map: &mut Map<String, Value>) -> Option<String> {
    let mut added = vec![];
    for (key, value) in [
        ("prefetch", Value::from("recent")),
        ("subject_threads", Value::Bool(true)),
        ("network", serde_json::to_value(crate::network::NetworkSettings::default()).unwrap_or(Value::Null)),
    ] {
        if !map.contains_key(key) {
            map.insert(key.into(), value);
            added.push(key);
        }
    }
    (!added.is_empty()).then(|| format!("added {}", added.join(", ")))
}

/// Version 4 (milestone 3): the phones' swipe actions and the background check, written with
/// their defaults (archive to the left, read or unread to the right, the check on).
fn phones(map: &mut Map<String, Value>) -> Option<String> {
    let mut added = vec![];
    for (key, value) in [
        ("swipe_left", Value::from("archive")),
        ("swipe_right", Value::from("read")),
        ("background_check", Value::Bool(true)),
    ] {
        if !map.contains_key(key) {
            map.insert(key.into(), value);
            added.push(key);
        }
    }
    (!added.is_empty()).then(|| format!("added {}", added.join(", ")))
}

/// Version 5: the inbox that sorts itself and the time tools, written with their defaults (the
/// Screener and Focus off, the unified inbox and the newsletter bundles on).
fn inbox_tools(map: &mut Map<String, Value>) -> Option<String> {
    let mut added = vec![];
    for (key, value) in [
        ("screener", Value::Bool(false)),
        ("focus", Value::Bool(false)),
        ("unified_inbox", Value::Bool(true)),
        ("bundles", Value::Bool(true)),
    ] {
        if !map.contains_key(key) {
            map.insert(key.into(), value);
            added.push(key);
        }
    }
    (!added.is_empty()).then(|| format!("added {}", added.join(", ")))
}

/// Version 6: the AI assistant (off: no provider), its automatic summaries, priority and tone
/// (on once a provider is set up), and the profile pictures (BIMI on, Gravatar off, as decided).
fn power_tools(map: &mut Map<String, Value>) -> Option<String> {
    let mut added = vec![];
    for (key, value) in [
        ("ai", serde_json::to_value(crate::ai::AiSettings::default()).unwrap_or(Value::Null)),
        ("bimi", Value::Bool(true)),
        ("gravatar", Value::Bool(false)),
    ] {
        if !map.contains_key(key) {
            map.insert(key.into(), value);
            added.push(key);
        }
    }
    (!added.is_empty()).then(|| format!("added {}", added.join(", ")))
}

/// Version 7: the calendar (new appointments of 30 minutes, Monday to Friday, the first writable
/// calendar for them), Autocrypt on (it only shares a public key the user made), nothing signed
/// by default, „An Arcalo senden“ makes a page.
fn calendar_crypto(map: &mut Map<String, Value>) -> Option<String> {
    let mut added = vec![];
    for (key, value) in [
        ("calendar", serde_json::to_value(crate::settings::CalendarSettings::default()).unwrap_or(Value::Null)),
        ("autocrypt", Value::Bool(true)),
        ("sign_by_default", Value::Null),
        ("arcalo_target", Value::from("page")),
    ] {
        if !map.contains_key(key) {
            map.insert(key.into(), value);
            added.push(key);
        }
    }
    (!added.is_empty()).then(|| format!("added {}", added.join(", ")))
}

/// Version 8 (the first release): desktop updates download in the background and install when
/// the app quits.
fn updates(map: &mut Map<String, Value>) -> Option<String> {
    if map.contains_key("updates") {
        return None;
    }
    map.insert("updates".into(), Value::from("auto"));
    Some("added updates".into())
}

/// Runs the steps the settings still need; returns what they did.
pub fn migrate(map: &mut Map<String, Value>) -> Vec<String> {
    let from = map.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    if from >= SETTINGS_VERSION {
        return vec![];
    }
    let mut done = vec![];
    for step in STEPS.iter().skip(from as usize) {
        if let Some(what) = (step.run)(map) {
            done.push(format!("{}: {what}", step.name));
        }
    }
    map.insert("version".into(), SETTINGS_VERSION.into());
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_in_order() {
        for (i, s) in STEPS.iter().enumerate() {
            assert_eq!(s.from as usize, i, "{}", s.name);
        }
    }

    #[test]
    fn mail_basics_keep_what_is_there() {
        let mut m: Map<String, Value> = serde_json::from_str(r#"{"version":1,"undo_send_secs":10}"#).unwrap();
        let done = migrate(&mut m);
        // mail_basics, fetch_threads_network and the later steps.
        assert_eq!(done.len(), 7, "{done:?}");
        assert!(done[0].starts_with("mail_basics: "), "{done:?}");
        assert_eq!(m["undo_send_secs"], 10);
        assert_eq!(m["notifications"], true);
        assert!(migrate(&mut m).is_empty(), "idempotent");
    }

    #[test]
    fn version_3_adds_fetching_threads_and_network_once() {
        let mut m: Map<String, Value> = serde_json::from_str(r#"{"version":2,"prefetch":"none"}"#).unwrap();
        let done = migrate(&mut m);
        assert_eq!(done[0], "fetch_threads_network: added subject_threads, network");
        assert_eq!(done.len(), 6, "and the later steps: {done:?}");
        assert_eq!(m["prefetch"], "none", "kept");
        assert_eq!(m["network"]["profiles"][0]["id"], "standard");
        assert!(migrate(&mut m).is_empty(), "idempotent");
    }

    #[test]
    fn version_4_adds_the_swipes_and_the_background_check_once() {
        let mut m: Map<String, Value> = serde_json::from_str(r#"{"version":3,"swipe_left":"delete"}"#).unwrap();
        let done = migrate(&mut m);
        assert_eq!(done[0], "phones: added swipe_right, background_check");
        assert_eq!(m["swipe_left"], "delete", "kept");
        assert_eq!(m["swipe_right"], "read");
        assert_eq!(m["background_check"], true);
        assert!(migrate(&mut m).is_empty(), "idempotent");
    }

    #[test]
    fn version_5_adds_the_inbox_tools_once() {
        let mut m: Map<String, Value> = serde_json::from_str(r#"{"version":4,"focus":true}"#).unwrap();
        let done = migrate(&mut m);
        assert_eq!(
            done,
            [
                "inbox_tools: added screener, unified_inbox, bundles",
                "power_tools: added ai, bimi, gravatar",
                "calendar_crypto: added calendar, autocrypt, sign_by_default, arcalo_target",
                "updates: added updates"
            ]
        );
        assert_eq!(m["focus"], true, "kept");
        assert_eq!(m["screener"], false);
        assert!(migrate(&mut m).is_empty(), "idempotent");
    }

    #[test]
    fn version_6_adds_the_assistant_off_and_the_pictures_once() {
        let mut m: Map<String, Value> = serde_json::from_str(r#"{"version":5,"gravatar":true}"#).unwrap();
        let done = migrate(&mut m);
        assert_eq!(done[0], "power_tools: added ai, bimi");
        assert_eq!(m["gravatar"], true, "kept");
        assert_eq!(m["ai"]["providers"], serde_json::json!([]), "off until a provider is set up");
        assert_eq!(m["bimi"], true);
        assert!(migrate(&mut m).is_empty(), "idempotent");
    }

    #[test]
    fn version_7_adds_the_calendar_and_autocrypt_once() {
        let mut m: Map<String, Value> = serde_json::from_str(r#"{"version":6,"autocrypt":false}"#).unwrap();
        let done = migrate(&mut m);
        assert_eq!(done, ["calendar_crypto: added calendar, sign_by_default, arcalo_target", "updates: added updates"]);
        assert_eq!(m["autocrypt"], false, "kept");
        assert_eq!(m["calendar"]["entry_minutes"], 30);
        assert_eq!(m["calendar"]["workdays"], serde_json::json!([1, 2, 3, 4, 5]));
        assert!(migrate(&mut m).is_empty(), "idempotent");
    }

    #[test]
    fn version_8_adds_the_update_mode_once() {
        let mut m: Map<String, Value> = serde_json::from_str(r#"{"version":7}"#).unwrap();
        assert_eq!(migrate(&mut m), ["updates: added updates"]);
        assert_eq!(m["updates"], "auto");
        let mut kept: Map<String, Value> = serde_json::from_str(r#"{"version":7,"updates":"notify"}"#).unwrap();
        assert!(migrate(&mut kept).is_empty());
        assert_eq!(kept["updates"], "notify", "kept");
        assert!(migrate(&mut m).is_empty(), "idempotent");
    }

    #[test]
    fn raises_the_version_and_leaves_newer_settings_alone() {
        let mut old = Map::new();
        migrate(&mut old);
        assert_eq!(old["version"], SETTINGS_VERSION);
        let mut newer: Map<String, Value> = serde_json::from_str(r#"{"version":999,"x":1}"#).unwrap();
        assert!(migrate(&mut newer).is_empty());
        assert_eq!(newer["version"], 999);
    }
}
