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
pub const SETTINGS_VERSION: u32 = 1;

/// One step `from → from + 1`: changes the settings object and says what it did (`None`:
/// nothing to do for these settings).
pub struct Step {
    pub from: u32,
    pub name: &'static str,
    pub run: fn(&mut Map<String, Value>) -> Option<String>,
}

/// The chain, in order. `STEPS[i].from == i`.
pub const STEPS: [Step; SETTINGS_VERSION as usize] = [Step { from: 0, name: "versioned", run: versioned }];

/// Version 0 is settings written before versions existed (development builds): nothing to
/// change, the step only records the start of the chain.
fn versioned(_: &mut Map<String, Value>) -> Option<String> {
    None
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
    fn raises_the_version_and_leaves_newer_settings_alone() {
        let mut old = Map::new();
        migrate(&mut old);
        assert_eq!(old["version"], SETTINGS_VERSION);
        let mut newer: Map<String, Value> = serde_json::from_str(r#"{"version":999,"x":1}"#).unwrap();
        assert!(migrate(&mut newer).is_empty());
        assert_eq!(newer["version"], 999);
    }
}
