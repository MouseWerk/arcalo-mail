//! Rules of CLAUDE.md that a test can check in the sources, as Arcalo checks its own (the
//! `client_for` check of `network.rs`, `error_texts_are_translated` of `error.rs`, the settings
//! steps of `settings_migrate.rs` there). The check that every connection goes through
//! `network` is `network::tests::no_connection_is_opened_outside_network`.

use std::path::{Path, PathBuf};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The Rust sources of the core and the shell, without test-only files.
fn sources() -> Vec<PathBuf> {
    let mut files = vec![];
    let mut stack = vec![root().join("src"), root().join("../../src-tauri/src")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("tests.rs") && !p.ends_with("fake.rs") {
                files.push(p);
            }
        }
    }
    assert!(files.len() > 50, "sources not found: {}", files.len());
    files
}

/// Migrations are positional: `NNNN_name.sql` numbered from 0001 without a gap, each one in
/// `store::MIGRATIONS` at its place. A gap or a second file with the same number (two branches
/// that both took the next number) fails here instead of in a user's database.
#[test]
fn migrations_are_positional() {
    let dir = root().join("migrations");
    let mut names: Vec<String> =
        std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert!(!names.is_empty());
    for (i, name) in names.iter().enumerate() {
        let (number, rest) = name.split_once('_').unwrap_or((name, ""));
        assert_eq!(number, format!("{:04}", i + 1), "{name}: migrations are numbered 0001, 0002, … without a gap");
        assert!(
            rest.ends_with(".sql")
                && rest.len() > 4
                && rest[..rest.len() - 4].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{name}: name it NNNN_words.sql"
        );
    }
    assert_eq!(mail_core::store::MIGRATIONS.len(), names.len(), "every file in migrations/ is in store::MIGRATIONS");
    for (i, name) in names.iter().enumerate() {
        let text = std::fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(mail_core::store::MIGRATIONS[i], text, "store::MIGRATIONS[{i}] is not {name}");
    }
}

/// Keys the settings had when versions began (version 1): no step writes them.
const SETTINGS_BASELINE: [&str; 6] =
    ["version", "language", "theme", "remote_content", "threads", "mark_read_after_secs"];

/// Keys that are written at run time only and start empty (`None`) for everyone: an older file
/// without them means the same as the default, so they need no step.
const SETTINGS_RUNTIME: [(&str, &str); 1] = [("screener_since", "set when the Screener is switched on")];

/// Settings changes need a step (CLAUDE.md): every key the settings have is either from the start,
/// written at run time only, or added by a step of `settings_migrate` with its value for people
/// who already have settings.
#[test]
fn every_settings_key_comes_with_a_step() {
    let value = serde_json::to_value(mail_core::settings::Settings::default()).unwrap();
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    let code = std::fs::read_to_string(root().join("src/settings_migrate.rs")).unwrap();
    let steps = code.split("#[cfg(test)]").next().unwrap();
    let mut missing = vec![];
    for key in &keys {
        let quoted = format!("\"{key}\"");
        if !SETTINGS_BASELINE.contains(&key.as_str())
            && !SETTINGS_RUNTIME.iter().any(|(k, _)| k == key)
            && !steps.contains(&quoted)
        {
            missing.push(key.as_str());
        }
    }
    assert!(
        missing.is_empty(),
        "settings keys without a step in settings_migrate.rs (raise SETTINGS_VERSION): {missing:?}"
    );
    // The lists stay honest.
    for key in SETTINGS_BASELINE.iter().chain(SETTINGS_RUNTIME.iter().map(|(k, _)| k)) {
        assert!(keys.iter().any(|k| k == key), "{key} is no longer a setting");
    }
    assert_eq!(mail_core::settings_migrate::STEPS.len() as u32, mail_core::settings_migrate::SETTINGS_VERSION);
}

/// Passwords and tokens live in the OS credential store, never in the settings.
#[test]
fn no_secret_in_the_settings() {
    let value = serde_json::to_value(mail_core::settings::Settings::default()).unwrap();
    let mut stack = vec![("".to_string(), &value)];
    while let Some((at, v)) = stack.pop() {
        if let Some(o) = v.as_object() {
            for (k, v) in o {
                let lower = k.to_ascii_lowercase();
                assert!(
                    !["password", "passwort", "secret", "token", "api_key", "apikey", "private_key"]
                        .iter()
                        .any(|w| lower.contains(w)),
                    "{at}.{k}: secrets go to the credential store (secrets.rs)"
                );
                stack.push((format!("{at}.{k}"), v));
            }
        }
    }
}

/// Literals of user-facing errors that are internal on purpose: they cannot reach the user (a
/// state the code rules out before) or are matched by the code.
const INTERNAL: [(&str, &str); 4] = [
    ("graph book without account", "a Microsoft address book is always added with its account"),
    ("graph source without account", "a Microsoft calendar is always added with its account"),
    ("jmap source without account", "a JMAP calendar is always added with its account"),
    ("not a JMAP account", "JMAP calendars are only offered for JMAP accounts"),
];

/// Error texts reach the UI in its language: a literal in `Error::Invalid("…")`,
/// `Error::NotFound("…")`, `Error::Network("…")`, `Error::Secrets("…")`, `Error::Newer("…")` or
/// `Error::Unconfirmed("…")` outside test code goes through `tr!`/`trf!`. `Error::Imap` keeps the
/// server's answer (and the IMAP-style answers of the POP3 and JMAP adapters).
#[test]
fn error_texts_are_translated() {
    let mut stray = vec![];
    for f in sources() {
        let text = std::fs::read_to_string(&f).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap_or_default();
        for variant in ["Invalid", "NotFound", "Network", "Secrets", "Newer", "Unconfirmed"] {
            let pat = format!("Error::{variant}(\"");
            for (at, _) in code.match_indices(&pat) {
                // `FeedError::Invalid` and other error types of their own.
                if code[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let rest = &code[at + pat.len()..];
                let literal = &rest[..rest.find('"').unwrap_or(rest.len())];
                if !INTERNAL.iter().any(|(l, _)| *l == literal) {
                    let line = code[..at].lines().count();
                    stray.push(format!("{}:{line}: {literal}", f.display()));
                }
            }
        }
    }
    assert!(stray.is_empty(), "untranslated error texts (use tr!):\n{}", stray.join("\n"));
    // The list stays honest.
    let all: String = sources().iter().map(|f| std::fs::read_to_string(f).unwrap()).collect();
    for (literal, _) in INTERNAL {
        assert!(all.contains(&format!("\"{literal}\"")), "{literal} is gone: remove it from INTERNAL");
    }
}
