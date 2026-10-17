//! App-Sperre, as in Arcalo: when Arcalo Mail locks (at the start, after minutes without input,
//! after the computer slept or the phone left the app), the PIN as an Argon2id hash, and the
//! growing wait after wrong PINs.
//!
//! The configuration and the count of wrong attempts are kept in the store's settings table under
//! their own keys (not in the app settings, so „Rückgängig“ in the settings never switches the
//! lock), the PIN hash in the credential store (the shell, entry [`PIN_ENTRY`]). The lock is a
//! privacy screen for the running app: it hides the mail, refuses the app's commands while
//! locked, and makes notifications and the widget say nothing but a count. It does not encrypt
//! anything; that is [`crate::cipher`]'s job.

use std::time::Duration;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::Store;
use crate::{tr, trf};

const CONFIG_KEY: &str = "applock";
const ATTEMPTS_KEY: &str = "applock.attempts";
/// The credential store entry of the PIN hash.
pub const PIN_ENTRY: &str = "app-lock-pin";
/// Shortest PIN.
pub const MIN_PIN: usize = 4;
/// Longest PIN (a passphrase is fine, but not a novel).
pub const MAX_PIN: usize = 64;
/// Wrong PINs before the first wait.
pub const FREE_ATTEMPTS: u32 = 3;
/// The longest wait between attempts.
pub const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockMode {
    #[default]
    Off,
    /// Locked at every start.
    Start,
    /// At the start and after `idle_minutes` without input (and after sleep, if chosen).
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LockConfig {
    pub mode: LockMode,
    /// Minutes without input (desktop) or away from the app (phones) before locking (mode `Idle`).
    pub idle_minutes: u32,
    /// Desktop: lock after the computer slept (a jump of the clock). Phones: lock as soon as the
    /// app goes to the background.
    pub on_sleep: bool,
    /// Offer Windows Hello, Touch ID, Face ID or the Android fingerprint next to the PIN.
    pub biometric: bool,
}

impl Default for LockConfig {
    fn default() -> Self {
        LockConfig { mode: LockMode::Off, idle_minutes: 5, on_sleep: true, biometric: false }
    }
}

impl LockConfig {
    pub fn normalize(&mut self) {
        self.idle_minutes = self.idle_minutes.clamp(1, 240);
    }

    pub fn enabled(&self) -> bool {
        self.mode != LockMode::Off
    }

    /// Time without input after which to lock (`None`: not by idle time).
    pub fn idle_limit(&self) -> Option<Duration> {
        (self.mode == LockMode::Idle).then(|| Duration::from_secs(u64::from(self.idle_minutes) * 60))
    }

    /// Whether the app locks now: after `idle` without input, or because the computer `slept`.
    pub fn due(&self, idle: Duration, slept: bool) -> bool {
        self.enabled() && (self.idle_limit().is_some_and(|l| idle >= l) || (self.on_sleep && slept))
    }
}

/// Wrong attempts so far (reset by a correct PIN or another way of unlocking).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempts {
    pub failures: u32,
    /// Unix seconds of the last wrong attempt.
    pub last: Option<i64>,
}

/// The wait after `failures` wrong attempts: none for the first [`FREE_ATTEMPTS`], then 5 s,
/// doubling each time, at most [`MAX_WAIT`].
pub fn wait_after(failures: u32) -> Duration {
    if failures < FREE_ATTEMPTS {
        return Duration::ZERO;
    }
    let doublings = (failures - FREE_ATTEMPTS).min(16);
    Duration::from_secs(5u64 << doublings).min(MAX_WAIT)
}

impl Attempts {
    /// How long until the next attempt is accepted (`now`: Unix seconds).
    pub fn remaining(&self, now: i64) -> Duration {
        let Some(last) = self.last else { return Duration::ZERO };
        let wait = wait_after(self.failures);
        let passed = Duration::from_secs(now.saturating_sub(last).max(0) as u64);
        wait.saturating_sub(passed)
    }

    pub fn failed(&mut self, now: i64) {
        self.failures = self.failures.saturating_add(1);
        self.last = Some(now);
    }
}

/// Checks the form of a new PIN.
pub fn check_new_pin(pin: &str) -> Result<()> {
    let n = pin.chars().count();
    if n < MIN_PIN {
        return Err(Error::Invalid(
            tr!("Die PIN braucht mindestens 4 Zeichen.", "The PIN needs at least 4 characters.").into(),
        ));
    }
    if n > MAX_PIN {
        return Err(Error::Invalid(tr!("Die PIN ist zu lang.", "The PIN is too long.").into()));
    }
    Ok(())
}

fn hasher() -> argon2::Argon2<'static> {
    // OWASP's recommendation for Argon2id: 19 MiB, 2 passes, 1 lane.
    let params = argon2::Params::new(19 * 1024, 2, 1, None).unwrap_or_default();
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
}

/// The PIN as an Argon2id hash in the PHC format (`$argon2id$v=19$…`), with a random salt.
pub fn hash_pin(pin: &str) -> Result<String> {
    check_new_pin(pin)?;
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| Error::Invalid(e.to_string()))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(hasher().hash_password(pin.as_bytes(), &salt).map_err(|e| Error::Invalid(e.to_string()))?.to_string())
}

/// Whether `pin` matches `hash` (from [`hash_pin`]). A hash asking for more than the app's own
/// cost (a value put there by someone else) is refused without being tried.
pub fn verify_pin(pin: &str, hash: &str) -> bool {
    let Ok(h) = PasswordHash::new(hash) else { return false };
    let cost = |name: &str| h.params.get_decimal(name).unwrap_or(u32::MAX);
    if cost("m") > 19 * 1024 || cost("t") > 2 || cost("p") > 1 {
        return false;
    }
    hasher().verify_password(pin.as_bytes(), &h).is_ok()
}

pub fn config(store: &Store) -> LockConfig {
    store.setting(CONFIG_KEY).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn set_config(store: &Store, c: &LockConfig) -> Result<()> {
    let mut c = c.clone();
    c.normalize();
    store.set_setting(CONFIG_KEY, &serde_json::to_string(&c)?)
}

pub fn attempts(store: &Store) -> Attempts {
    store.setting(ATTEMPTS_KEY).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn set_attempts(store: &Store, a: &Attempts) -> Result<()> {
    store.set_setting(ATTEMPTS_KEY, &serde_json::to_string(a)?)
}

/// The text of a new-mail notification while the lock is on: the count and nothing else (no
/// sender, no subject, no buttons).
pub fn private_notification(count: usize) -> (String, String) {
    let body = if count == 1 {
        tr!("Neue E-Mail – zum Lesen entsperren", "New e-mail – unlock to read it").to_string()
    } else {
        trf!("{count} neue E-Mails – zum Lesen entsperren", "{count} new e-mails – unlock to read them")
    };
    ("Arcalo Mail".into(), body)
}

/// The text of a reminder while the lock is on.
pub fn private_reminder() -> (String, String) {
    ("Arcalo Mail".into(), tr!("Erinnerung – zum Ansehen entsperren", "Reminder – unlock to see it").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_hashed_with_argon2id_and_verified() {
        assert!(hash_pin("123").is_err(), "too short");
        assert!(hash_pin(&"9".repeat(65)).is_err(), "too long");
        let h = hash_pin("4711").unwrap();
        assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{h}");
        assert!(!h.contains("4711"));
        assert!(verify_pin("4711", &h));
        assert!(!verify_pin("4712", &h));
        assert!(!verify_pin("4711", "not a hash"));
        assert_ne!(h, hash_pin("4711").unwrap(), "a random salt");
        // A hash with a cost far above the app's own is not tried.
        let costly = h.replace("m=19456,t=2", "m=4194304,t=1000");
        let start = std::time::Instant::now();
        assert!(!verify_pin("4711", &costly));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn wrong_pins_make_the_wait_grow_up_to_a_limit() {
        assert_eq!(wait_after(0), Duration::ZERO);
        assert_eq!(wait_after(2), Duration::ZERO);
        assert_eq!(wait_after(3), Duration::from_secs(5));
        assert_eq!(wait_after(4), Duration::from_secs(10));
        assert_eq!(wait_after(6), Duration::from_secs(40));
        assert_eq!(wait_after(20), MAX_WAIT);
        assert_eq!(wait_after(u32::MAX), MAX_WAIT);
        let t0 = 1_000_000;
        let mut a = Attempts::default();
        for _ in 0..3 {
            assert_eq!(a.remaining(t0), Duration::ZERO);
            a.failed(t0);
        }
        assert_eq!(a.remaining(t0), Duration::from_secs(5));
        assert_eq!(a.remaining(t0 + 3), Duration::from_secs(2));
        assert_eq!(a.remaining(t0 + 9), Duration::ZERO);
    }

    #[test]
    fn idle_time_and_sleep_lock_only_when_chosen() {
        let off = LockConfig::default();
        assert!(!off.due(Duration::from_secs(99_999), true));
        let start = LockConfig { mode: LockMode::Start, on_sleep: false, ..LockConfig::default() };
        assert!(!start.due(Duration::from_secs(99_999), true));
        let idle = LockConfig { mode: LockMode::Idle, idle_minutes: 2, on_sleep: true, biometric: false };
        assert!(!idle.due(Duration::from_secs(119), false));
        assert!(idle.due(Duration::from_secs(120), false));
        assert!(idle.due(Duration::ZERO, true));
        let no_sleep = LockConfig { on_sleep: false, ..idle };
        assert!(!no_sleep.due(Duration::ZERO, true));
    }

    #[test]
    fn the_configuration_and_attempts_survive_in_the_store() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(config(&s), LockConfig::default());
        assert!(!config(&s).enabled());
        let c = LockConfig { mode: LockMode::Idle, idle_minutes: 0, on_sleep: false, biometric: true };
        set_config(&s, &c).unwrap();
        let back = config(&s);
        assert_eq!(back.idle_minutes, 1, "clamped");
        assert_eq!(back.idle_limit(), Some(Duration::from_secs(60)));
        assert!(back.biometric);
        let mut a = Attempts::default();
        a.failed(5);
        set_attempts(&s, &a).unwrap();
        assert_eq!(attempts(&s).failures, 1);
        // Not part of the app settings: „Rückgängig“ there never touches the lock.
        crate::settings::save(&s, &crate::settings::Settings::default()).unwrap();
        assert_eq!(config(&s).mode, LockMode::Idle);
        assert!(!s.setting("app").unwrap().unwrap().contains("applock"));
    }

    #[test]
    fn locked_notifications_say_only_how_many() {
        let (title, body) = private_notification(3);
        assert_eq!(title, "Arcalo Mail");
        assert!(body.starts_with("3 neue E-Mails") || body.starts_with("3 new e-mails"), "{body}");
        let (_, one) = private_notification(1);
        assert!(one.starts_with("Neue E-Mail") || one.starts_with("New e-mail"), "{one}");
    }
}
