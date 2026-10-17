//! Updates of the desktop app, as Arcalo's `updates.rs`: the app asks the release feed
//! (`latest.json` of the newest GitHub release, through `mail_core::network` like every other
//! connection) at the start and every few hours, downloads the update in the background (it can
//! be paused and continues where it stopped) and checks its signature against the compiled-in
//! public key. In the mode „Automatisch“ the update is installed when the app quits, or at once
//! with „Jetzt neu starten“; in the mode „Nur benachrichtigen“ the window tells about it and the
//! user installs it with a click. The first start after an update shows „Neu in Arcalo Mail“.
//!
//! Only release builds with the update key compiled in (`ARCALO_MAIL_UPDATER_PUBKEY`, set by the
//! release workflow from `plugins.updater.pubkey` in `tauri.conf.json`) switch any of this on;
//! other builds never contact the feed. A copy that cannot replace its own program files says
//! why and offers the release page instead: the portable ZIP, a .deb or .rpm package, an app
//! macOS runs from the disk image or the Downloads folder, an AppImage in a folder without write
//! permission (`mail_core::update::Manual`).
//!
//! Before installing, the update waits for messages that are being sent (the mail counterpart of
//! Arcalo's wait for running terminal commands), at most a minute, with a hint in the window; it
//! never drops one (the outbox keeps whatever did not go out for the next start).
//!
//! The installation itself is `installer.rs`. The updater plugin is registered for its
//! configuration and target names only; it never fetches anything.
//!
//! Debug builds take a test feed, key, version and reason from `ARCALO_MAIL_UPDATE_ENDPOINT`,
//! `ARCALO_MAIL_UPDATE_PUBKEY`, `ARCALO_MAIL_UPDATE_CURRENT` and `ARCALO_MAIL_UPDATE_MANUAL`, the
//! wait from `ARCALO_MAIL_UPDATE_WAIT_SECS`, and with `ARCALO_MAIL_UPDATE_FAKE_INSTALL=1` note the
//! install in `updates/fake-install.json` instead of running it (the end-to-end tests); release
//! builds ignore them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use mail_core::network::Service;
use mail_core::update::{self as core, AfterUpdate, Manual, UpdateMode, UpdateState, Verdict};
use mail_core::update_feed::{self as feed, FeedError};
use mail_core::{Error, tr};
use serde::{Deserialize, Serialize};
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::installer::{self, InstallError};

type Res<T> = Result<T, Error>;

/// The record of a verified download (`updates/` in the data folder).
const STAGED_FILE: &str = "staged.json";

/// How long an install waits for messages that are being sent.
const SEND_WAIT: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn test_var(name: &str) -> Option<String> {
    core::test_override(cfg!(debug_assertions), std::env::var(name).ok().as_deref()).map(str::to_string)
}

/// The public key of the update signatures, compiled in by the release build.
pub fn pubkey() -> Option<&'static str> {
    static KEY: OnceLock<Option<String>> = OnceLock::new();
    KEY.get_or_init(|| {
        test_var("ARCALO_MAIL_UPDATE_PUBKEY")
            .or_else(|| core::configured_pubkey(option_env!("ARCALO_MAIL_UPDATER_PUBKEY")).map(str::to_string))
    })
    .as_deref()
}

/// The updater plugin, or `None` in builds without a key (the app then runs without it).
pub fn plugin<R: Runtime>() -> Option<TauriPlugin<R, tauri_plugin_updater::Config>> {
    pubkey().map(|key| tauri_plugin_updater::Builder::new().pubkey(key).build())
}

fn feed_url() -> String {
    test_var("ARCALO_MAIL_UPDATE_ENDPOINT").unwrap_or_else(|| core::FEED_URL.to_string())
}

/// The version this copy runs (debug builds: `ARCALO_MAIL_UPDATE_CURRENT` stands in for it).
pub fn current_version(app: &AppHandle) -> String {
    test_var("ARCALO_MAIL_UPDATE_CURRENT").unwrap_or_else(|| app.package_info().version.to_string())
}

/// Installed through a package manager (.deb, .rpm).
fn packaged() -> bool {
    use tauri::utils::config::BundleType;
    matches!(tauri::utils::platform::bundle_type(), Some(BundleType::Deb | BundleType::Rpm))
}

/// Why this copy offers the release page instead of installing, found once per start. Debug
/// builds are not installed: they answer `ARCALO_MAIL_UPDATE_MANUAL` (tests) and install
/// otherwise (through the fake install of the tests).
fn manual(app: &AppHandle) -> Option<Manual> {
    static MANUAL: OnceLock<Option<Manual>> = OnceLock::new();
    *MANUAL.get_or_init(|| {
        if cfg!(debug_assertions) {
            return test_var("ARCALO_MAIL_UPDATE_MANUAL").and_then(|m| Manual::parse(&m));
        }
        let found = core::manual_reason(crate::portable::active(), packaged(), installer::blocked(app));
        if let Some(m) = found {
            mail_core::diag::log("update", &format!("updates are not installed by this copy: {m:?}"));
        }
        found
    })
}

/// The keys of `latest.json` this copy takes (`windows-x86_64-nsis`, `windows-x86_64`).
fn targets() -> Vec<String> {
    use tauri::utils::config::BundleType;
    let installer = match tauri::utils::platform::bundle_type() {
        Some(BundleType::Nsis) => Some("nsis"),
        Some(BundleType::AppImage) => Some("appimage"),
        Some(BundleType::App) => Some("app"),
        _ => None,
    };
    feed::targets(&tauri_plugin_updater::target().unwrap_or_default(), installer)
}

/// The update found by the last check.
#[derive(Clone, Debug)]
struct Offer {
    version: String,
    notes: Option<String>,
    date: Option<String>,
    signature: String,
    url: String,
}

/// A downloaded and verified update file, waiting to be installed.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Staged {
    version: String,
    signature: String,
    path: PathBuf,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Phase {
    #[default]
    Idle,
    Downloading,
    Paused,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Default, Serialize)]
struct DownloadState {
    phase: Phase,
    downloaded: u64,
    total: Option<u64>,
    percent: Option<u8>,
    error: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    offer: Mutex<Option<Offer>>,
    staged: Mutex<Option<Staged>>,
    download: Mutex<DownloadState>,
    pause: AtomicBool,
    downloading: AtomicBool,
    installing: AtomicBool,
    /// The quit already installed (or decided not to): the exit event does nothing more.
    quit_handled: AtomicBool,
    /// This start follows an update (reported once to the UI).
    after: Mutex<Option<AfterUpdate>>,
    /// The install waits for this many messages that are being sent.
    waiting: Mutex<Option<u32>>,
}

impl Updates {
    pub fn after(after: Option<AfterUpdate>) -> Self {
        Updates { after: Mutex::new(after), ..Default::default() }
    }
}

#[derive(Serialize, Clone)]
pub struct UpdateInfo {
    version: String,
    /// Release notes (Markdown) from `latest.json`, without the testers' checklist.
    notes: Option<String>,
    date: Option<String>,
    /// The release page with the full notes and the files.
    url: String,
}

impl UpdateInfo {
    fn of(o: &Offer) -> Self {
        UpdateInfo {
            version: o.version.clone(),
            notes: o.notes.as_deref().map(core::notes_for_users).filter(|n| !n.is_empty()),
            date: o.date.clone(),
            url: core::release_url(&o.version),
        }
    }
}

/// The first start after an update, with the notes of that version.
#[derive(Serialize)]
pub struct Restarted {
    #[serde(flatten)]
    after: AfterUpdate,
    notes: Option<String>,
    url: String,
}

#[derive(Serialize)]
pub struct UpdateStatus {
    /// The build has an update key: checking and installing work.
    enabled: bool,
    current_version: String,
    /// Found by an earlier check and not installed yet.
    available: Option<UpdateInfo>,
    /// Why this copy does not install updates itself, and what to do instead.
    manual: Option<Manual>,
    manual_text: Option<String>,
    /// The first call after a start that followed an update (reported once).
    restarted: Option<Restarted>,
    mode: UpdateMode,
    skipped: Option<String>,
    remind_after: Option<DateTime<Utc>>,
    whats_new_seen: Option<String>,
    download: DownloadState,
    /// A downloaded, verified update waits for the quit (its version).
    ready: Option<String>,
    /// The install waits for this many messages that are being sent.
    waiting: Option<u32>,
    /// The page of the newest release (for a copy that updates by hand).
    release_page: String,
}

#[derive(Serialize, Clone)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
    percent: Option<u8>,
}

fn not_configured() -> Error {
    Error::Invalid(core::not_configured().into())
}

fn state_error(text: impl Into<String>) -> Error {
    Error::Invalid(text.into())
}

fn feed_failed(what: &str, e: &FeedError) -> Error {
    mail_core::diag::log("update", &format!("{what}: {e:?}"));
    state_error(format!("{what}: {}", e.message()))
}

fn install_failed(what: &str, e: &InstallError) -> Error {
    mail_core::diag::log("update", &format!("{what}: {e:?}"));
    state_error(format!("{what}: {}", e.message()))
}

fn data_dir(app: &AppHandle) -> PathBuf {
    app.state::<crate::AppState>().data_dir.clone()
}

fn updates_dir(app: &AppHandle) -> PathBuf {
    data_dir(app).join(core::UPDATES_DIR)
}

fn mode(app: &AppHandle) -> UpdateMode {
    let state = app.state::<crate::AppState>();
    mail_core::settings::load(&state.store()).map(|s| s.updates).unwrap_or_default()
}

fn state_changed(app: &AppHandle) {
    let _ = app.emit("update://state", ());
}

/// At start: a verified download of an earlier session is still waiting (unless it is old).
pub fn load_staged(app: &AppHandle) {
    if pubkey().is_none() {
        return;
    }
    let dir = updates_dir(app);
    let staged: Option<Staged> =
        std::fs::read_to_string(dir.join(STAGED_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok());
    let current = current_version(app);
    match staged {
        Some(s) if s.path.is_file() && core::is_newer(&current, &s.version) => {
            *lock(&app.state::<Updates>().staged) = Some(s);
        }
        Some(s) => {
            let _ = std::fs::remove_file(&s.path);
            let _ = std::fs::remove_file(dir.join(STAGED_FILE));
        }
        None => {}
    }
}

#[tauri::command]
pub fn update_status(app: AppHandle, updates: State<Updates>) -> UpdateStatus {
    let state = UpdateState::load(&data_dir(&app));
    let staged = lock(&updates.staged).clone();
    let offer = lock(&updates.offer).clone();
    let ready =
        staged.as_ref().filter(|s| offer.as_ref().is_some_and(|o| o.version == s.version)).map(|s| s.version.clone());
    let manual = if pubkey().is_some() { manual(&app) } else { None };
    let restarted = lock(&updates.after).take().map(|after| Restarted {
        notes: state.notes_for(&after.version).map(core::notes_for_users).filter(|n| !n.is_empty()),
        url: core::release_url(&after.version),
        after,
    });
    UpdateStatus {
        enabled: pubkey().is_some(),
        current_version: current_version(&app),
        available: offer.as_ref().map(UpdateInfo::of),
        manual,
        manual_text: manual.map(|m| m.message().to_string()),
        restarted,
        mode: mode(&app),
        skipped: state.skipped,
        remind_after: state.remind_after.filter(|t| *t > Utc::now()),
        whats_new_seen: state.whats_new_seen,
        download: lock(&updates.download).clone(),
        ready,
        waiting: *lock(&updates.waiting),
        release_page: core::latest_release_url(),
    }
}

/// Asks the feed for a newer version. In the mode „Automatisch“ a new version starts downloading
/// in the background; nothing is installed here. `manual`: the user clicked „Nach Updates
/// suchen“ (that also ends a running „Später erinnern“).
#[tauri::command(async)]
pub async fn update_check(
    app: AppHandle,
    updates: State<'_, Updates>,
    manual: Option<bool>,
) -> Res<Option<UpdateInfo>> {
    if pubkey().is_none() {
        return Err(not_configured());
    }
    if updates.installing.load(Ordering::SeqCst) {
        return Err(state_error(tr!("Das Update wird gerade installiert", "The update is being installed")));
    }
    let what = tr!("Update-Prüfung fehlgeschlagen", "Update check failed");
    let client = mail_core::network::client_for(Service::UpdateFeed)?;
    let m = feed::fetch(&client, &feed_url()).await.map_err(|e| feed_failed(what, &e))?;
    let dir = data_dir(&app);
    let mut state = UpdateState::load(&dir);
    if manual == Some(true) && state.remind_after.take().is_some() {
        let _ = state.save(&dir);
    }
    let current = current_version(&app);
    let verdict = core::verdict(&current, &m.version, &state, Utc::now());
    if verdict != Verdict::Offer {
        *lock(&updates.offer) = None;
        state_changed(&app);
        return Ok(None);
    }
    let entry = feed::platform(&m, &targets()).ok_or_else(|| feed_failed(what, &FeedError::NoPlatform))?;
    let offer = Offer {
        version: m.version.trim().trim_start_matches('v').to_string(),
        notes: m.notes.clone().filter(|n| !n.trim().is_empty()),
        date: m.pub_date.as_deref().and_then(|d| d.get(..10)).map(str::to_string),
        signature: entry.signature.clone(),
        url: entry.url.clone(),
    };
    let info = UpdateInfo::of(&offer);
    let changed = lock(&updates.offer).as_ref().is_none_or(|o| o.version != offer.version);
    *lock(&updates.offer) = Some(offer);
    if changed {
        let ready = lock(&updates.staged).as_ref().is_some_and(|s| s.version == info.version);
        *lock(&updates.download) =
            DownloadState { phase: if ready { Phase::Ready } else { Phase::Idle }, ..Default::default() };
    }
    if mode(&app) == UpdateMode::Auto && self::manual(&app).is_none() {
        start_download(&app);
    }
    state_changed(&app);
    Ok(Some(info))
}

/// Starts (or continues) the background download of the offered update.
fn start_download(app: &AppHandle) {
    let updates = app.state::<Updates>();
    if updates.installing.load(Ordering::SeqCst) || updates.downloading.swap(true, Ordering::SeqCst) {
        return;
    }
    updates.pause.store(false, Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let updates = app.state::<Updates>();
        let res = loop {
            let res = ensure_staged(&app).await;
            // A check found a newer version while this one was downloading: that one follows.
            let newer =
                matches!(&res, Ok(Some(s)) if lock(&updates.offer).as_ref().is_some_and(|o| o.version != s.version));
            if !newer || updates.pause.load(Ordering::SeqCst) {
                break res;
            }
        };
        updates.downloading.store(false, Ordering::SeqCst);
        match res {
            Ok(Some(s)) => mail_core::diag::log("update", &format!("{} downloaded and verified", s.version)),
            Ok(None) => mail_core::diag::log("update", "download paused"),
            Err(e) => {
                mail_core::diag::log("update", &format!("background download: {e}"));
                let mut d = lock(&updates.download);
                d.error = Some(e.to_string());
                d.phase = Phase::Failed;
            }
        }
        state_changed(&app);
    });
}

/// „Fortsetzen“ after a pause or a failed download.
#[tauri::command]
pub fn update_download(app: AppHandle, updates: State<Updates>) -> Res<()> {
    if lock(&updates.offer).is_none() {
        return Err(state_error(tr!("Kein Update gefunden", "No update found")));
    }
    if let Some(m) = manual(&app) {
        return Err(state_error(m.message()));
    }
    start_download(&app);
    Ok(())
}

/// „Pausieren“ and „Abbrechen“: the download stops after the current chunk; the part file stays,
/// so the next start of the download continues where it stopped.
#[tauri::command]
pub fn update_pause(updates: State<Updates>) {
    updates.pause.store(true, Ordering::SeqCst);
}

/// Downloads (continuing a part) and verifies the offered update; `None` when paused.
async fn ensure_staged(app: &AppHandle) -> Res<Option<Staged>> {
    let updates = app.state::<Updates>();
    let offer =
        lock(&updates.offer).clone().ok_or_else(|| state_error(tr!("Kein Update gefunden", "No update found")))?;
    if let Some(s) = lock(&updates.staged).clone().filter(|s| s.version == offer.version && s.path.is_file()) {
        *lock(&updates.download) = DownloadState { phase: Phase::Ready, ..Default::default() };
        return Ok(Some(s));
    }
    let key = pubkey().ok_or_else(not_configured)?;
    let dir = updates_dir(app);
    let part = dir.join(format!("{}.part", offer.version));
    let what = tr!("Download fehlgeschlagen", "Download failed");
    *lock(&updates.download) = DownloadState { phase: Phase::Downloading, ..Default::default() };
    state_changed(app);
    let client = mail_core::network::client_for(Service::UpdateFeed)?;
    let (mut last, mut reported) = (None, 0u64);
    let step = feed::download(&client, &offer.url, &part, &updates.pause, |downloaded, total| {
        let percent = core::progress_percent(downloaded, total);
        // One event per percent, or per 512 KiB while the size is unknown.
        if percent != last || (percent.is_none() && downloaded - reported >= 512 * 1024) {
            (last, reported) = (percent, downloaded);
            *lock(&updates.download) =
                DownloadState { phase: Phase::Downloading, downloaded, total, percent, error: None };
            let _ = app.emit("update://progress", Progress { downloaded, total, percent });
        }
    })
    .await
    .map_err(|e| {
        *lock(&updates.download) =
            DownloadState { phase: Phase::Failed, error: Some(e.message()), ..Default::default() };
        feed_failed(what, &e)
    })?;
    if step == feed::Step::Paused {
        lock(&updates.download).phase = Phase::Paused;
        return Ok(None);
    }
    let bytes = std::fs::read(&part).map_err(|e| state_error(e.to_string()))?;
    if let Err(e) = feed::verify(&bytes, &offer.signature, key, &offer.version) {
        // A tampered or broken file is never kept, nor continued.
        let _ = std::fs::remove_file(&part);
        *lock(&updates.download) =
            DownloadState { phase: Phase::Failed, error: Some(e.message()), ..Default::default() };
        return Err(feed_failed(what, &e));
    }
    let path = dir.join(format!("{}.update", offer.version));
    std::fs::rename(&part, &path).map_err(|e| state_error(e.to_string()))?;
    // Older downloads go.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p != path && p.extension().is_some_and(|x| x == "update" || x == "part") {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let staged =
        Staged { version: offer.version.clone(), signature: offer.signature.clone(), path, notes: offer.notes.clone() };
    let _ = std::fs::write(dir.join(STAGED_FILE), serde_json::to_string(&staged).unwrap_or_default());
    *lock(&updates.staged) = Some(staged.clone());
    *lock(&updates.download) = DownloadState { phase: Phase::Ready, percent: Some(100), ..Default::default() };
    Ok(Some(staged))
}

/// „Installieren“ (mode „Nur benachrichtigen“): downloads (if needed), verifies and installs the
/// offered update, then restarts. The UI saved an open draft before. On Windows the setup runs
/// passively and starts the new version itself.
#[tauri::command(async)]
pub async fn update_install(app: AppHandle, updates: State<'_, Updates>) -> Res<()> {
    if pubkey().is_none() {
        return Err(not_configured());
    }
    if let Some(m) = manual(&app) {
        return Err(state_error(m.message()));
    }
    if lock(&updates.offer).is_none() {
        return Err(state_error(tr!("Kein Update gefunden", "No update found")));
    }
    if updates.installing.swap(true, Ordering::SeqCst) {
        return Err(state_error(tr!("Das Update wird bereits installiert", "The update is already being installed")));
    }
    let res = async {
        // A background download may be running: it is paused and continued here.
        if updates.downloading.load(Ordering::SeqCst) {
            updates.pause.store(true, Ordering::SeqCst);
            for _ in 0..100 {
                if !updates.downloading.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        // Still running (a stalled chunk): never two downloads into the same part file.
        if updates.downloading.swap(true, Ordering::SeqCst) {
            return Err(state_error(tr!(
                "Der Download läuft noch – versuche es gleich noch einmal",
                "The download is still running – try again in a moment"
            )));
        }
        updates.pause.store(false, Ordering::SeqCst);
        let staged = ensure_staged(&app).await;
        updates.downloading.store(false, Ordering::SeqCst);
        // „Abbrechen“ during the download: nothing is installed; the part stays.
        let staged = staged?.ok_or_else(|| state_error(tr!("Download abgebrochen", "Download cancelled")))?;
        install(&app, &staged, true).await
    }
    .await;
    updates.installing.store(false, Ordering::SeqCst);
    if res.is_err() {
        state_changed(&app);
    }
    res?;
    finish_restart(&app);
    Ok(())
}

/// „Jetzt neu starten“ on the notice of a downloaded update.
#[tauri::command(async)]
pub async fn update_restart_now(app: AppHandle, updates: State<'_, Updates>) -> Res<()> {
    if let Some(m) = manual(&app) {
        return Err(state_error(m.message()));
    }
    let not_loaded = || state_error(tr!("Das Update ist noch nicht geladen", "The update has not been downloaded yet"));
    let staged = lock(&updates.staged).clone().ok_or_else(not_loaded)?;
    // The file went meanwhile (cleaned up by hand): it is downloaded again.
    if !staged.path.is_file() {
        discard_staged(&app, None);
        if lock(&updates.offer).is_some() {
            start_download(&app);
        }
        return Err(not_loaded());
    }
    if updates.installing.swap(true, Ordering::SeqCst) {
        return Err(state_error(tr!("Das Update wird bereits installiert", "The update is already being installed")));
    }
    updates.quit_handled.store(true, Ordering::SeqCst);
    let res = install(&app, &staged, true).await;
    updates.installing.store(false, Ordering::SeqCst);
    if let Err(e) = res {
        updates.quit_handled.store(false, Ordering::SeqCst);
        return Err(e);
    }
    finish_restart(&app);
    Ok(())
}

/// The new version is in place (macOS, Linux; Windows ended this process for its setup): the
/// app starts again. A test run's fake install only ends the app.
fn finish_restart(app: &AppHandle) {
    mail_core::diag::log("update", "installed, restarting");
    if fake_install() {
        app.exit(0);
        return;
    }
    app.restart();
}

fn fake_install() -> bool {
    test_var("ARCALO_MAIL_UPDATE_FAKE_INSTALL").is_some_and(|v| v == "1")
}

/// The update the quit installs (mode „Automatisch“, not snoozed or skipped, a copy that installs
/// itself), marked as being installed; `None`: the quit installs nothing.
fn quit_staged(app: &AppHandle) -> Option<Staged> {
    let updates = app.try_state::<Updates>()?;
    if pubkey().is_none() || updates.quit_handled.load(Ordering::SeqCst) {
        return None;
    }
    let staged = lock(&updates.staged).clone().filter(|s| s.path.is_file())?;
    if mode(app) != UpdateMode::Auto || manual(app).is_some() {
        return None;
    }
    let state = UpdateState::load(&data_dir(app));
    let verdict = core::verdict(&current_version(app), &staged.version, &state, Utc::now());
    if verdict != Verdict::Offer {
        mail_core::diag::log("update", &format!("{} not installed on quit: {verdict:?}", staged.version));
        return None;
    }
    if updates.quit_handled.swap(true, Ordering::SeqCst) || updates.installing.swap(true, Ordering::SeqCst) {
        return None;
    }
    Some(staged)
}

/// Whether quitting now installs an update (the window's close then waits for it).
pub fn installs_on_quit(app: &AppHandle) -> bool {
    let Some(updates) = app.try_state::<Updates>() else { return false };
    if pubkey().is_none() || updates.quit_handled.load(Ordering::SeqCst) || mode(app) != UpdateMode::Auto {
        return false;
    }
    lock(&updates.staged).as_ref().is_some_and(|s| s.path.is_file()) && manual(app).is_none()
}

/// „Beenden“ (the tray, closing the window): installs a waiting update, then ends the app. On
/// Windows the setup ends this process itself. Runs off the main thread: the install may wait for
/// messages that are being sent.
pub fn quit(app: &AppHandle) {
    if app.try_state::<Updates>().is_some_and(|u| u.installing.load(Ordering::SeqCst)) {
        mail_core::diag::log("update", "quit ignored: the update is being installed");
        return;
    }
    let Some(staged) = quit_staged(app) else {
        app.exit(0);
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        install_on_quit(&app, &staged);
        app.exit(0);
    });
}

/// The process ends without the app's own quit (macOS: the Dock's „Beenden“, logging out):
/// replacing the bundle or the AppImage takes a moment and is done here. The Windows setup is not
/// started during a shutdown; the update stays ready for the next quit.
pub fn on_exit(app: &AppHandle) {
    if cfg!(windows) {
        return;
    }
    if let Some(staged) = quit_staged(app) {
        install_on_quit(app, &staged);
    }
}

fn install_on_quit(app: &AppHandle, staged: &Staged) {
    mail_core::diag::log("update", &format!("installing {} on quit", staged.version));
    let res = tauri::async_runtime::block_on(install(app, staged, false));
    app.state::<Updates>().installing.store(false, Ordering::SeqCst);
    if let Err(e) = res {
        mail_core::diag::log("update", &format!("install on quit failed: {e}"));
    }
}

fn wait_limit() -> Duration {
    test_var("ARCALO_MAIL_UPDATE_WAIT_SECS").and_then(|s| s.parse().ok()).map(Duration::from_secs).unwrap_or(SEND_WAIT)
}

/// Waits while messages are being sent (at most [`wait_limit`]), with a hint in the window. A
/// message that did not go out by then stays in the outbox and goes at the next start.
async fn wait_for_sending(app: &AppHandle) {
    let updates = app.state::<Updates>();
    let start = Instant::now();
    loop {
        let sending = {
            let state = app.state::<crate::AppState>();
            mail_core::outbox::sending(&state.store(), mail_core::store::now()).unwrap_or(0)
        };
        if sending == 0 || start.elapsed() >= wait_limit() {
            if sending > 0 {
                mail_core::diag::log(
                    "update",
                    &format!("{sending} messages still being sent: they go at the next start"),
                );
            }
            break;
        }
        if *lock(&updates.waiting) != Some(sending) {
            *lock(&updates.waiting) = Some(sending);
            state_changed(app);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if lock(&updates.waiting).take().is_some() {
        state_changed(app);
    }
}

/// Notes the restart and the notes for „Neu in Arcalo Mail“, then installs the verified file
/// (`installer.rs`). `restart`: the Windows setup starts the new version.
async fn install(app: &AppHandle, staged: &Staged, restart: bool) -> Res<()> {
    let what = tr!("Installation fehlgeschlagen", "Installation failed");
    let key = pubkey().ok_or_else(not_configured)?;
    let bytes = std::fs::read(&staged.path).map_err(|e| state_error(e.to_string()))?;
    // Checked again: the file waited in the data folder since its download. A file that no
    // longer matches goes, so the next check downloads it again instead of every quit failing.
    if let Err(e) = feed::verify(&bytes, &staged.signature, key, &staged.version) {
        discard_staged(app, Some(e.message()));
        return Err(feed_failed(what, &e));
    }
    wait_for_sending(app).await;
    let dir = data_dir(app);
    let from = current_version(app);
    let mut state = UpdateState::load(&dir);
    state.notes = staged.notes.clone().map(|text| core::Notes { version: staged.version.clone(), text });
    if let Err(e) = state.save(&dir) {
        mail_core::diag::log("update", &format!("notes not kept: {e}"));
    }
    // The next start says what happened.
    if let Err(e) = core::write_restart_marker(&dir, &staged.version, &from) {
        mail_core::diag::log("update", &format!("restart note not written: {e}"));
    }
    if fake_install() {
        let note = serde_json::json!({ "version": staged.version, "from": from, "restart": restart });
        std::fs::write(updates_dir(app).join("fake-install.json"), note.to_string())
            .map_err(|e| state_error(e.to_string()))?;
        return Ok(());
    }
    let target = installer::Target::current(app, &staged.version, restart).map_err(|e| {
        core::clear_restart_marker(&dir);
        install_failed(what, &e)
    })?;
    let handle = app.clone();
    // Windows: the tray icon goes right before the setup starts; a started setup ends this process.
    installer::install(&bytes, &target, move || handle.cleanup_before_exit()).map_err(|e| {
        core::clear_restart_marker(&dir);
        install_failed(what, &e)
    })
}

/// Forgets the downloaded update (its file and the record). `error`: why, shown as a failed
/// download (with „Erneut versuchen“); `None`: as if nothing was downloaded yet.
fn discard_staged(app: &AppHandle, error: Option<String>) {
    let updates = app.state::<Updates>();
    if let Some(s) = lock(&updates.staged).take() {
        let _ = std::fs::remove_file(&s.path);
    }
    let _ = std::fs::remove_file(updates_dir(app).join(STAGED_FILE));
    let phase = if error.is_some() { Phase::Failed } else { Phase::Idle };
    *lock(&updates.download) = DownloadState { phase, error, ..Default::default() };
    state_changed(app);
}

/// „Diese Version überspringen“: not offered again (the next newer one is); the download goes.
#[tauri::command]
pub fn update_skip(app: AppHandle, updates: State<Updates>, version: String) -> Res<()> {
    let dir = data_dir(&app);
    let mut state = UpdateState::load(&dir);
    state.skipped = Some(version.trim().trim_start_matches('v').to_string());
    state.save(&dir).map_err(|e| state_error(e.to_string()))?;
    updates.pause.store(true, Ordering::SeqCst);
    if let Some(s) = lock(&updates.staged).take() {
        let _ = std::fs::remove_file(&s.path);
        let _ = std::fs::remove_file(updates_dir(&app).join(STAGED_FILE));
    }
    let _ = std::fs::remove_file(updates_dir(&app).join(format!("{}.part", version.trim())));
    *lock(&updates.offer) = None;
    *lock(&updates.download) = DownloadState::default();
    state_changed(&app);
    Ok(())
}

/// „Rückgängig“ for a skipped version, and the end of „Später erinnern“.
#[tauri::command]
pub fn update_unskip(app: AppHandle) -> Res<()> {
    let dir = data_dir(&app);
    let mut state = UpdateState::load(&dir);
    state.skipped = None;
    state.remind_after = None;
    state.save(&dir).map_err(|e| state_error(e.to_string()))?;
    state_changed(&app);
    Ok(())
}

/// „Später erinnern“: no notice and no install for `days` days (a downloaded file stays).
#[tauri::command]
pub fn update_remind(app: AppHandle, updates: State<Updates>, days: u32) -> Res<DateTime<Utc>> {
    let dir = data_dir(&app);
    let mut state = UpdateState::load(&dir);
    let at = Utc::now() + chrono::Duration::days(days.clamp(1, 30) as i64);
    state.remind_after = Some(at);
    state.save(&dir).map_err(|e| state_error(e.to_string()))?;
    updates.pause.store(true, Ordering::SeqCst);
    *lock(&updates.offer) = None;
    *lock(&updates.download) = DownloadState::default();
    state_changed(&app);
    Ok(at)
}

/// „Neu in Arcalo Mail“ of `version` was shown.
#[tauri::command]
pub fn update_whats_new_seen(app: AppHandle, version: String) -> Res<()> {
    let dir = data_dir(&app);
    let mut state = UpdateState::load(&dir);
    state.whats_new_seen = Some(version);
    state.notes = None;
    state.save(&dir).map_err(|e| state_error(e.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_feed_matches_the_config() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let endpoints: Vec<&str> =
            conf["plugins"]["updater"]["endpoints"].as_array().unwrap().iter().filter_map(|e| e.as_str()).collect();
        assert_eq!(endpoints, [mail_core::update::FEED_URL]);
        assert_eq!(conf["plugins"]["updater"]["windows"]["installMode"], "passive");
    }
}
