//! The update notice of the Android app installed from the GitHub release (the APK), as Arcalo's
//! companion app checks for its updates: `android-latest.json` of the newest release at the start,
//! once a day and on „Nach Updates suchen“. A newer version shows a notice with „Herunterladen“,
//! which opens the APK's address in the browser; Android's installer checks the file and asks
//! before it replaces the app (`mail_core::update_android`).
//!
//! Never for a copy from Google Play (or another store): the installer package Android names for
//! the app decides it, and such a copy never asks the feed. Only release builds with the update
//! key compiled in (`ARCALO_MAIL_UPDATER_PUBKEY`, as on the desktop) check at all; other builds
//! never contact the feed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use mail_core::network::Service;
use mail_core::update_android::{self as core, AndroidRelease, Offer};
use mail_core::update_feed::{self, FeedError};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// The event the UI listens to for every change of [`Status`].
const EVENT: &str = "apkupdate://status";
/// The device's own memory of the notice (never synced).
const PREFS_FILE: &str = "android-update.json";
/// How often the loop looks whether the daily check is due.
const LOOP_EVERY: Duration = Duration::from_secs(60 * 60);
/// The first check after the start, once the app has settled.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The compiled-in public key of the update signatures (release builds only).
pub fn pubkey() -> Option<&'static str> {
    mail_core::update::configured_pubkey(option_env!("ARCALO_MAIL_UPDATER_PUBKEY"))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Nothing to tell (up to date, or not checked yet).
    #[default]
    Idle,
    Checking,
    /// A newer version: „Herunterladen“.
    Available,
    /// A newer version that cannot replace this app (another APK signing key).
    Blocked,
    /// The last check asked for failed (`error` says why).
    Error,
}

/// The newer version on offer.
#[derive(Debug, Clone, Serialize)]
pub struct Offered {
    pub version: String,
    pub notes: Option<String>,
    pub size: Option<u64>,
    pub pub_date: Option<String>,
    /// The APK's address (the browser downloads it).
    pub apk: String,
    /// The release page.
    pub page: String,
}

/// What the UI shows (the notice and Settings → Über).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    /// This build checks for updates (a release build, installed from the GitHub release).
    pub enabled: bool,
    /// Installed from Google Play or another store: the store updates the app.
    pub store: bool,
    pub current: String,
    pub phase: Phase,
    pub offered: Option<Offered>,
    pub error: Option<String>,
    /// Why the offered version cannot be installed over this app.
    pub blocked: Option<String>,
    pub last_check: Option<DateTime<Utc>>,
    /// The app was updated from this version and „Neu in Arcalo Mail“ was not shown since.
    pub updated_from: Option<String>,
    /// The notes of the version running now, kept when it was offered.
    pub notes: Option<String>,
}

/// The device's memory of the notice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Prefs {
    #[serde(default)]
    last_check: Option<DateTime<Utc>>,
    /// The version „Neu in Arcalo Mail“ was last shown for (or the first start's).
    #[serde(default)]
    whats_new_seen: Option<String>,
    /// The notes of the version last offered, for „Neu in Arcalo Mail“ after it.
    #[serde(default)]
    notes: Option<(String, String)>,
}

pub struct Notice {
    status: Mutex<Status>,
    prefs: Mutex<Prefs>,
    prefs_path: PathBuf,
    busy: AtomicBool,
}

impl Notice {
    pub fn new(app: &AppHandle, data_dir: &Path) -> Notice {
        let prefs_path = data_dir.join(PREFS_FILE);
        let mut prefs: Prefs =
            std::fs::read_to_string(&prefs_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let current = app.package_info().version.to_string();
        let store = super::android::installed().is_ok_and(|i| core::from_store(i.installer.as_deref()));
        let (updated_from, record) = core::whats_new_from(prefs.whats_new_seen.as_deref(), &current);
        let notes =
            prefs.notes.as_ref().filter(|(v, _)| *v == current).map(|(_, n)| mail_core::update::notes_for_users(n));
        let status = Status {
            enabled: pubkey().is_some() && !store,
            store,
            current,
            last_check: prefs.last_check,
            updated_from,
            notes: notes.filter(|n| !n.is_empty()),
            ..Status::default()
        };
        let save = record.is_some();
        if save {
            prefs.whats_new_seen = record;
        }
        let n =
            Notice { status: Mutex::new(status), prefs: Mutex::new(prefs), prefs_path, busy: AtomicBool::new(false) };
        if save {
            n.save_prefs();
        }
        n
    }

    fn snapshot(&self) -> Status {
        lock(&self.status).clone()
    }

    fn save_prefs(&self) {
        let text = serde_json::to_string_pretty(&*lock(&self.prefs)).unwrap_or_default();
        let tmp = self.prefs_path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &self.prefs_path)) {
            mail_core::diag::log("update", &format!("update notice not saved: {e}"));
        }
    }
}

/// Changes the status and tells the UI.
fn set(app: &AppHandle, f: impl FnOnce(&mut Status)) -> Status {
    let n = app.state::<Notice>();
    let snap = {
        let mut s = lock(&n.status);
        f(&mut s);
        s.clone()
    };
    let _ = app.emit(EVENT, &snap);
    snap
}

fn offered(r: &AndroidRelease) -> Offered {
    Offered {
        version: r.version.clone(),
        notes: r.notes.as_deref().map(mail_core::update::notes_for_users).filter(|n| !n.trim().is_empty()),
        size: r.size,
        pub_date: r.pub_date.clone(),
        apk: r.url.clone(),
        page: mail_core::update::release_url(&r.version),
    }
}

/// Clears `busy` when the check ends, whichever way.
struct Busy<'a>(&'a AtomicBool);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Asks the feed. Runs on a worker thread; a second call while one runs only answers the status.
pub fn check(app: &AppHandle, manual: bool) -> Status {
    let n = app.state::<Notice>();
    if !n.snapshot().enabled || n.busy.swap(true, Ordering::SeqCst) {
        return n.snapshot();
    }
    let _busy = Busy(&n.busy);
    set(app, |s| {
        s.phase = Phase::Checking;
        s.error = None;
    });
    let fetched = mail_core::network::client_for(Service::UpdateFeed)
        .map_err(|e| FeedError::Io(e.to_string()))
        .and_then(|client| {
            tauri::async_runtime::block_on(async move {
                let text = update_feed::get_text(&client, core::FEED_URL).await?;
                core::parse(&text)
            })
        });
    let now = Utc::now();
    lock(&n.prefs).last_check = Some(now);
    n.save_prefs();
    set(app, |s| s.last_check = Some(now));
    let release = match fetched {
        Ok(r) => r,
        // The newest release has no APK feed (its build failed): nothing on offer, quietly.
        Err(FeedError::Missing) => {
            return set(app, |s| {
                s.phase = Phase::Idle;
                s.offered = None;
            });
        }
        Err(e) if !manual => {
            mail_core::diag::log("update", &format!("update check failed: {}", e.message()));
            return set(app, |s| s.phase = if s.offered.is_some() { Phase::Available } else { Phase::Idle });
        }
        Err(e) => {
            return set(app, |s| {
                s.phase = Phase::Error;
                s.error = Some(e.message());
            });
        }
    };
    let installed = match super::android::installed() {
        Ok(i) => i,
        Err(e) => {
            return set(app, |s| {
                s.phase = Phase::Error;
                s.error = Some(e);
            });
        }
    };
    match core::offer(&installed, &release) {
        Offer::UpToDate | Offer::OtherPackage => set(app, |s| {
            s.phase = Phase::Idle;
            s.offered = None;
            s.blocked = None;
        }),
        Offer::OtherSigner => {
            let o = offered(&release);
            set(app, |s| {
                s.phase = Phase::Blocked;
                s.offered = Some(o);
                s.blocked = Some(core::other_signer().into());
            })
        }
        Offer::Update => {
            if let Some(notes) = release.notes.clone() {
                lock(&n.prefs).notes = Some((release.version.clone(), notes));
                n.save_prefs();
            }
            let o = offered(&release);
            set(app, |s| {
                s.phase = Phase::Available;
                s.offered = Some(o);
                s.blocked = None;
            })
        }
    }
}

/// Checks after the start, then whenever a day has passed.
pub fn spawn_scheduler(app: AppHandle) {
    if !app.state::<Notice>().snapshot().enabled {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK_AFTER);
        check(&app, false);
        loop {
            std::thread::sleep(LOOP_EVERY);
            let n = app.state::<Notice>();
            if core::check_due(lock(&n.prefs).last_check, Utc::now()) {
                check(&app, false);
            }
        }
    });
}

#[tauri::command]
pub fn apk_update_status(app: AppHandle) -> Status {
    app.state::<Notice>().snapshot()
}

/// „Nach Updates suchen“ (`manual`), or the check of the app's own schedule.
#[tauri::command]
pub async fn apk_update_check(app: AppHandle, manual: bool) -> Result<Status, mail_core::Error> {
    tauri::async_runtime::spawn_blocking(move || check(&app, manual))
        .await
        .map_err(|e| mail_core::Error::Invalid(e.to_string()))
}

/// „Neu in Arcalo Mail“ was shown after an update (or closed): not again for this version.
#[tauri::command]
pub fn apk_whats_new_seen(app: AppHandle) -> Status {
    let n = app.state::<Notice>();
    let current = n.snapshot().current;
    {
        let mut prefs = lock(&n.prefs);
        prefs.whats_new_seen = Some(current);
        prefs.notes = None;
    }
    n.save_prefs();
    set(&app, |s| {
        s.updated_from = None;
        s.notes = None;
    })
}
