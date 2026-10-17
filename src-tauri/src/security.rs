//! Settings → Sicherheit, as in Arcalo: the encrypted database (the key in the credential store,
//! the recovery screen at the start, switching on and off while the app runs) and the app lock
//! (PIN, Windows Hello, Touch ID, Face ID, the Android fingerprint or face; lock after inactivity,
//! after sleep, when a phone leaves the app).
//!
//! The database key is read before the store opens ([`prepare`]). When the file is encrypted and
//! the key is missing or wrong (a new device, a restored disk image, a portable copy on another
//! computer), the app starts on an empty store and shows the recovery screen ([`gate`]); nothing
//! else runs (no workers, no clock) until the recovery key or the password opened it.
//!
//! The app lock hides the window's content behind the lock screen and refuses the app's commands
//! while locked ([`guard`]); new-mail notifications, reminders and the Android widget say nothing
//! but a count while the lock is on, whether the app runs or not (`mail_core::applock`,
//! `background.rs`).

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use mail_core::Error;
use mail_core::applock::{self, LockConfig};
use mail_core::cipher::{self, Access, DbKey, Direction, FileState, Migration, WrappedKey};
use mail_core::store::Store;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, Wry};

use crate::{AppState, secrets};

type Res<T> = Result<T, Error>;

/// What a command refused while locked answers (the UI waits for the unlock instead).
pub const LOCKED_ERROR: &str = "app-locked";
/// Copies of attachments opened in other programs, in the data folder.
pub const OPEN_COPIES: &str = "attachments";

fn invalid(text: impl Into<String>) -> Error {
    Error::Invalid(text.into())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------ database key

fn stored_key() -> Option<DbKey> {
    secrets::get(cipher::KEY_ENTRY).ok().flatten().and_then(|h| DbKey::from_hex(&h))
}

/// Stores `key` in the credential store and reads it back (a store that drops it silently would
/// lock the user out at the next start).
fn store_key(key: &DbKey) -> Res<()> {
    secrets::set(cipher::KEY_ENTRY, key.to_hex().as_str())?;
    if stored_key().as_ref() != Some(key) {
        return Err(invalid(mail_core::tr!(
            "Der Schlüssel konnte nicht im Schlüsselbund des Systems abgelegt werden.",
            "The key could not be saved in the credential store."
        )));
    }
    Ok(())
}

/// Before the store opens: what the start found.
pub struct Prepared {
    /// Shown in the app once it runs (a switch finished or failed at this start).
    pub notice: Option<String>,
    /// The recovery screen is needed (missing or wrong key).
    pub blocked: Option<Access>,
}

/// Before the store opens (the app's start and the phones' background check): reads the key,
/// finishes a switch a crash interrupted, and tells whether the database can be opened.
pub fn prepare(dir: &Path) -> Prepared {
    let key = stored_key();
    cipher::set_key(key.clone());
    let notice = match cipher::run_pending(dir, key.as_ref()) {
        Ok(Some(o)) => {
            mail_core::diag::log("cipher", &format!("{:?} finished at the start", o.direction));
            Some(
                match o.direction {
                    Direction::Encrypt => mail_core::tr!(
                        "Die Verschlüsselung der Datenbank wurde abgeschlossen.",
                        "Encrypting the database was finished."
                    ),
                    Direction::Decrypt => mail_core::tr!(
                        "Die Datenbank ist wieder unverschlüsselt.",
                        "The database is unencrypted again."
                    ),
                }
                .to_string(),
            )
        }
        Ok(None) => None,
        Err(e) => {
            mail_core::diag::log("cipher", &format!("switching the encryption at the start: {e}"));
            Some(mail_core::trf!(
                "Die Verschlüsselung wurde nicht umgestellt, die Datenbank ist unverändert: {e}",
                "The encryption was not switched; the database is unchanged: {e}"
            ))
        }
    };
    let access = cipher::access(&dir.join(cipher::DB_FILE), key.as_ref());
    Prepared { notice, blocked: matches!(access, Access::MissingKey | Access::WrongKey).then_some(access) }
}

/// The store opened: the previous file of a switch is deleted, and with an encrypted database
/// the copies of attachments opened in other programs.
pub fn opened(dir: &Path) {
    match cipher::confirm_open(dir) {
        Ok(true) => mail_core::diag::log("cipher", "previous database file overwritten and deleted"),
        Ok(false) => {}
        Err(e) => mail_core::diag::log("cipher", &format!("previous database file not deleted: {e}")),
    }
    if cipher::file_state(&dir.join(cipher::DB_FILE)) == FileState::Encrypted {
        cipher::secure_delete_dir(&dir.join(OPEN_COPIES));
    }
}

// ------------------------------------------------------- recovery screen

/// Why the start shows the recovery screen (none: the database is open).
static GATE: Mutex<Option<Access>> = Mutex::new(None);
/// A notice of the start for the UI (taken once).
static NOTICE: Mutex<Option<String>> = Mutex::new(None);

pub fn set_gate(reason: Option<Access>) {
    *lock(&GATE) = reason;
}

pub fn gate() -> Option<Access> {
    *lock(&GATE)
}

pub fn set_notice(text: Option<String>) {
    *lock(&NOTICE) = text;
}

/// The key from the recovery key or the password; it must open the database.
fn key_from(dir: &Path, method: &str, secret: &str) -> Res<DbKey> {
    let key = match method {
        "password" => WrappedKey::read(dir)
            .ok_or_else(|| invalid(mail_core::tr!("Kein Passwort eingerichtet.", "No password set up.")))?
            .unwrap_key(secret)?,
        _ => DbKey::from_recovery_code(secret)?,
    };
    if !cipher::key_opens(&dir.join(cipher::DB_FILE), &key) {
        return Err(invalid(cipher::wrong_key_text()));
    }
    Ok(key)
}

/// „Öffnen“ on the recovery screen: checks the key, keeps it (in the credential store when
/// `remember`, else for this run only), opens the store and starts the app.
#[tauri::command(async)]
pub fn keygate_unlock(
    app: AppHandle,
    state: State<'_, AppState>,
    method: String,
    secret: String,
    remember: bool,
) -> Res<()> {
    if gate().is_none() {
        return Ok(());
    }
    let dir = state.data_dir.clone();
    let key = key_from(&dir, &method, &secret)?;
    if remember {
        store_key(&key)?;
    }
    cipher::set_key(Some(key));
    let store = Store::open(&state.db)?;
    mail_core::outbox::recover(&store)?;
    let settings = mail_core::settings::load(&store)?;
    *state.store() = store;
    set_gate(None);
    opened(&dir);
    crate::apply_language(settings.language);
    mail_core::diag::log("cipher", &format!("database opened with the {method} (remembered: {remember})"));
    init_lock(&app);
    crate::begin(&app, settings);
    let _ = app.emit("security-changed", ());
    Ok(())
}

#[tauri::command]
pub fn keygate_quit(app: AppHandle) {
    app.exit(0);
}

// --------------------------------------------------- encryption settings

#[derive(Serialize)]
pub struct CipherStatus {
    state: FileState,
    /// A key is in the credential store.
    key_stored: bool,
    /// Linux without a Secret Service: the key lies next to the database (protects less).
    store_file: bool,
    /// A password-wrapped key lies in the data folder.
    password: bool,
    pending: Option<Migration>,
    /// The previous file of a switch waits for the next start.
    old_left: bool,
    /// A notice of this start (a switch a crash interrupted).
    notice: Option<String>,
}

#[tauri::command]
pub fn cipher_status(state: State<'_, AppState>) -> CipherStatus {
    let dir = &state.data_dir;
    CipherStatus {
        state: cipher::file_state(&state.db),
        key_stored: stored_key().is_some(),
        store_file: matches!(secrets::kind(), secrets::Kind::File { .. }),
        password: WrappedKey::read(dir).is_some(),
        pending: cipher::read_marker(dir),
        old_left: cipher::old_left(dir),
        notice: lock(&NOTICE).take(),
    }
}

/// The recovery key of the database key (a new key is created and stored first when there is none
/// yet and `create` is set).
#[tauri::command]
pub fn cipher_recovery_key(create: bool) -> Res<String> {
    let key = match cipher::key().or_else(stored_key) {
        Some(k) => k,
        None if create => {
            let k = DbKey::generate()?;
            store_key(&k)?;
            mail_core::diag::log("cipher", "database key created and stored");
            k
        }
        None => return Err(invalid(cipher::missing_key_text())),
    };
    Ok(key.recovery_code())
}

/// „Als Datei speichern“: the recovery key as a text file with a short explanation.
#[tauri::command]
pub fn cipher_recovery_save(path: String, code: String) -> Res<()> {
    DbKey::from_recovery_code(&code)?;
    let text = mail_core::trf!(
        "Arcalo Mail – Wiederherstellungsschlüssel\n\n{code}\n\nDamit öffnet Arcalo Mail die verschlüsselte Datenbank, wenn der Schlüssel \
         auf einem Gerät fehlt. Bewahre diese Datei getrennt vom Gerät auf (Passwortmanager, Ausdruck im Schrank). \
         Wer ihn hat, kann die Datenbank lesen.\n",
        "Arcalo Mail – recovery key\n\n{code}\n\nWith it, Arcalo Mail opens the encrypted database when the key is missing on a \
         device. Keep this file away from the device (password manager, a printout in a drawer). Whoever has it can read the \
         database.\n"
    );
    let p = std::path::PathBuf::from(path.trim());
    std::fs::write(&p, text).map_err(|e| invalid(format!("{}: {e}", p.display())))
}

/// How long a switch waits for the account workers to close their connections.
const SWITCH_WAIT: Duration = Duration::from_secs(20);

/// „Datenbank verschlüsseln“ / „Verschlüsselung entfernen“, while the app runs: the workers stop,
/// the app's own connection closes, the switch runs (crash-safe, `mail_core::cipher`), the store
/// opens again and the workers start again. After decrypting, the key and its password file are
/// removed.
#[tauri::command(async)]
pub fn cipher_switch(app: AppHandle, state: State<'_, AppState>, encrypt: bool) -> Res<CipherStatus> {
    let dir = state.data_dir.clone();
    let key = cipher::key().or_else(stored_key).ok_or_else(|| invalid(cipher::missing_key_text()))?;
    let direction = if encrypt { Direction::Encrypt } else { Direction::Decrypt };
    mail_core::diag::log("cipher", &format!("{direction:?} asked for"));
    crate::stop_workers(&app);
    // Meanwhile the app's commands wait (the guard refuses them) and its own connection is
    // closed; threads that use the store see an empty one in memory for these seconds.
    SWITCHING.store(true, Ordering::SeqCst);
    *state.store() = Store::open_in_memory()?;
    cipher::set_key(Some(key.clone()));
    let result = cipher::switch(&dir, direction, &key, mail_core::store::now(), SWITCH_WAIT);
    // Whatever happened, the store opens again (the file is either switched or unchanged).
    let reopened = Store::open(&state.db);
    let reopened = match reopened {
        Ok(s) => {
            *state.store() = s;
            Ok(())
        }
        Err(e) => {
            mail_core::diag::log("cipher", &format!("store not opened again after the switch: {e}"));
            Err(e)
        }
    };
    SWITCHING.store(false, Ordering::SeqCst);
    reopened?;
    crate::start_workers(&app);
    let _ = app.emit("security-changed", ());
    let outcome = result?;
    mail_core::diag::log("cipher", &format!("{:?} done", outcome.direction));
    match outcome.direction {
        Direction::Encrypt => cipher::secure_delete_dir(&dir.join(OPEN_COPIES)),
        Direction::Decrypt => {
            cipher::set_key(None);
            let _ = WrappedKey::remove(&dir);
            if let Err(e) = secrets::delete(cipher::KEY_ENTRY) {
                mail_core::diag::log("cipher", &format!("key not removed from the credential store: {e}"));
            }
        }
    }
    Ok(cipher_status(state))
}

/// „Schlüssel mit Passwort schützen“: keeps the key wrapped with `password` in the data folder
/// (`None` removes it).
#[tauri::command(async)]
pub fn cipher_password(state: State<'_, AppState>, password: Option<String>) -> Res<()> {
    let dir = &state.data_dir;
    match password.filter(|p| !p.is_empty()) {
        None => WrappedKey::remove(dir),
        Some(p) => {
            let key = cipher::key().or_else(stored_key).ok_or_else(|| invalid(cipher::missing_key_text()))?;
            WrappedKey::wrap(&key, &p, cipher::WRAP_COST)?.write(dir)
        }
    }
}

/// „Jetzt sicher löschen“: the previous file of a switch now instead of at the next start.
#[tauri::command]
pub fn cipher_drop_old(state: State<'_, AppState>) -> Res<()> {
    cipher::drop_old(&state.data_dir)
}

// --------------------------------------------------------------- app lock

static LOCKED: AtomicBool = AtomicBool::new(false);
/// The encryption is being switched: the app's commands wait.
static SWITCHING: AtomicBool = AtomicBool::new(false);
/// A lock is asked for and waits for the window to save what is being written.
static PENDING: AtomicBool = AtomicBool::new(false);

struct Watch {
    /// A PIN is stored (cached: the credential store is not asked every few seconds).
    has_pin: bool,
    /// The last input the window reported.
    last_input: Option<Instant>,
    /// The last check of the clock (a much larger gap: the computer slept).
    last_tick: Option<SystemTime>,
    /// A phone went to the background at this time.
    away_since: Option<Instant>,
    /// Debug builds: the idle time the next check sees (end-to-end tests).
    fake_idle: Option<Duration>,
}

static WATCH: Mutex<Watch> =
    Mutex::new(Watch { has_pin: false, last_input: None, last_tick: None, away_since: None, fake_idle: None });

/// How often the lock looks at the idle time and the clock.
const TICK: Duration = Duration::from_secs(5);
/// A clock jump this large between two checks means the computer slept.
const SLEEP_GAP: Duration = Duration::from_secs(90);
/// How long the window gets to save the message being written before the lock comes anyway.
const FLUSH_WAIT: Duration = Duration::from_secs(3);

pub fn is_locked() -> bool {
    LOCKED.load(Ordering::SeqCst)
}

/// Whether notifications and the widget must stay quiet about content: the lock is on.
pub fn private(store: &Store) -> bool {
    applock::config(store).enabled()
}

/// Commands the lock screen and the recovery screen use; everything else waits. Changing the lock
/// itself (a new PIN, switching it off) needs the unlocked app.
fn allowed(cmd: &str, gated: bool) -> bool {
    let shared = matches!(cmd, "app_info" | "get_settings" | "set_in_front" | "drafts_listen" | "drafts_saved");
    if gated {
        return shared || cmd.starts_with("keygate_") || cmd == "security_state";
    }
    shared
        || (cmd.starts_with("applock_")
            && !matches!(cmd, "applock_configure" | "applock_lock_now" | "applock_test_idle"))
        || cmd == "security_state"
}

/// Wraps the app's command handler: while locked or on the recovery screen, other commands are
/// refused.
pub fn guard<F>(inner: F) -> impl Fn(tauri::ipc::Invoke<Wry>) -> bool + Send + Sync + 'static
where
    F: Fn(tauri::ipc::Invoke<Wry>) -> bool + Send + Sync + 'static,
{
    move |invoke| {
        let gated = gate().is_some();
        let busy = SWITCHING.load(Ordering::SeqCst) && invoke.message.command() != "security_state";
        if busy || ((gated || is_locked()) && !allowed(invoke.message.command(), gated)) {
            invoke.resolver.reject(LOCKED_ERROR);
            return true;
        }
        inner(invoke)
    }
}

/// At the start, once the store is open: locked from the first frame when the lock is on.
pub fn init_lock(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let config = applock::config(&state.store());
    let has_pin = secrets::get(applock::PIN_ENTRY).ok().flatten().is_some();
    {
        let mut w = lock(&WATCH);
        w.has_pin = has_pin;
        w.last_input = Some(Instant::now());
    }
    if config.enabled() && has_pin {
        LOCKED.store(true, Ordering::SeqCst);
        mail_core::diag::log("applock", "locked at start");
    }
}

fn set_locked(app: &AppHandle, locked: bool) {
    if !locked {
        PENDING.store(false, Ordering::SeqCst);
        lock(&WATCH).last_input = Some(Instant::now());
    }
    if LOCKED.swap(locked, Ordering::SeqCst) == locked {
        return;
    }
    mail_core::diag::log("applock", if locked { "locked" } else { "unlocked" });
    if locked {
        // The window switcher shows the title: no unread count while locked.
        #[cfg(desktop)]
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_title("Arcalo Mail");
        }
        // Attachments opened in other programs go (their programs may keep them open).
        if let Some(state) = app.try_state::<AppState>() {
            cipher::secure_delete_dir(&state.data_dir.join(OPEN_COPIES));
        }
    }
    let _ = app.emit("applock-changed", locked);
    if !locked {
        crate::update_badge(app);
    }
}

/// Every lock (idle time, sleep, „Jetzt sperren“, a phone leaving the app): the window first
/// saves the message being written (`applock-locking`, answered by [`applock_flushed`]), because
/// the lock screen closes it. Without an answer the lock comes after [`FLUSH_WAIT`] anyway.
fn request_lock(app: &AppHandle) {
    if is_locked() || PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    if app.emit("applock-locking", ()).is_err() {
        PENDING.store(false, Ordering::SeqCst);
        set_locked(app, true);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FLUSH_WAIT);
        if PENDING.swap(false, Ordering::SeqCst) {
            mail_core::diag::log("applock", "the window did not confirm its draft, locking anyway");
            set_locked(&app, true);
        }
    });
}

/// The window saved the message being written: the requested lock comes now.
#[tauri::command]
pub fn applock_flushed(app: AppHandle) {
    if PENDING.swap(false, Ordering::SeqCst) {
        set_locked(&app, true);
    }
}

/// Input in the window (keys, pointer, touch), reported at most every few seconds.
#[tauri::command]
pub fn applock_activity() {
    lock(&WATCH).last_input = Some(Instant::now());
}

/// One look at the idle time and the clock (every [`TICK`]).
fn tick(app: &AppHandle) {
    let now = SystemTime::now();
    let (idle, slept, has_pin) = {
        let mut w = lock(&WATCH);
        let slept = w.last_tick.and_then(|t| now.duration_since(t).ok()).is_some_and(|d| d > SLEEP_GAP);
        w.last_tick = Some(now);
        let idle = w.fake_idle.take().or_else(|| w.last_input.map(|t| t.elapsed())).unwrap_or_default();
        (idle, slept, w.has_pin)
    };
    if is_locked() || !has_pin || gate().is_some() {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else { return };
    let config = applock::config(&state.store());
    // On a phone the lock follows leaving the app (`on_front`), not the clock.
    let slept = slept && cfg!(desktop);
    if config.due(idle, slept) {
        mail_core::diag::log("applock", if slept { "the computer slept" } else { "idle time reached" });
        request_lock(app);
    }
}

/// The lock's clock: started once with the app.
pub fn spawn_watch(app: &AppHandle) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let _ = std::thread::Builder::new().name("applock".into()).spawn(move || {
        loop {
            std::thread::sleep(TICK);
            tick(&app);
        }
    });
}

/// A phone app went to the background or came back: with „Beim Verlassen sperren“ it locks at
/// once; with the idle time it locks when it was away at least that long.
#[cfg_attr(desktop, allow(dead_code))]
pub fn on_front(app: &AppHandle, visible: bool) {
    let away = {
        let mut w = lock(&WATCH);
        if !visible {
            w.away_since = Some(Instant::now());
            None
        } else {
            w.last_input = Some(Instant::now());
            w.away_since.take().map(|t| t.elapsed())
        }
    };
    if !lock(&WATCH).has_pin || gate().is_some() {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else { return };
    let config = applock::config(&state.store());
    if !config.enabled() {
        return;
    }
    if !visible && config.on_sleep {
        // Locked right away: the system's app switcher shows the lock screen, not the mail.
        set_locked(app, true);
    } else if let Some(away) = away
        && config.idle_limit().is_some_and(|l| away >= l)
    {
        set_locked(app, true);
    }
}

#[derive(Serialize)]
pub struct LockStatus {
    config: LockConfig,
    has_pin: bool,
    locked: bool,
    /// Seconds until the next PIN is accepted.
    wait_secs: u64,
    failures: u32,
    /// `windows_hello`, `touch_id`, `face_id`, `android` or none.
    biometric: Option<&'static str>,
    /// The database is encrypted: the recovery key resets a forgotten PIN.
    encrypted: bool,
    /// Why the recovery screen shows (none: the database is open).
    gate: Option<Access>,
    /// A password-wrapped key lies in the data folder (the recovery screen offers it).
    password: bool,
    /// A phone (the lock follows leaving the app).
    phone: bool,
}

fn status_of(state: &AppState) -> LockStatus {
    let store = state.store();
    let attempts = applock::attempts(&store);
    LockStatus {
        config: applock::config(&store),
        has_pin: lock(&WATCH).has_pin,
        locked: is_locked(),
        wait_secs: attempts.remaining(mail_core::store::now()).as_secs(),
        failures: attempts.failures,
        biometric: os::available(),
        encrypted: cipher::file_state(&state.db) == FileState::Encrypted,
        gate: gate(),
        password: WrappedKey::read(&state.data_dir).is_some(),
        phone: cfg!(mobile),
    }
}

/// What the lock screen and the recovery screen need (allowed while locked).
#[tauri::command]
pub fn security_state(state: State<'_, AppState>) -> LockStatus {
    status_of(&state)
}

#[tauri::command]
pub fn applock_status(state: State<'_, AppState>) -> LockStatus {
    status_of(&state)
}

/// Saves the lock's settings; `pin` sets a new PIN (needed to switch the lock on). Switching it
/// off deletes the PIN.
#[tauri::command(async)]
pub fn applock_configure(
    app: AppHandle,
    state: State<'_, AppState>,
    config: LockConfig,
    pin: Option<String>,
) -> Res<LockStatus> {
    if let Some(pin) = pin.filter(|p| !p.is_empty()) {
        let hash = applock::hash_pin(&pin)?;
        secrets::set(applock::PIN_ENTRY, &hash)?;
        lock(&WATCH).has_pin = true;
    }
    if config.enabled() && !lock(&WATCH).has_pin {
        return Err(invalid(mail_core::tr!("Leg zuerst eine PIN fest.", "Set a PIN first.")));
    }
    if !config.enabled() {
        secrets::delete(applock::PIN_ENTRY)?;
        lock(&WATCH).has_pin = false;
    }
    {
        let store = state.store();
        applock::set_config(&store, &config)?;
        applock::set_attempts(&store, &Default::default())?;
    }
    lock(&WATCH).last_input = Some(Instant::now());
    mail_core::diag::log("applock", &format!("configured: {:?}", config.mode));
    crate::update_badge(&app);
    Ok(status_of(&state))
}

#[derive(Serialize)]
pub struct UnlockReply {
    ok: bool,
    wait_secs: u64,
    failures: u32,
}

/// „Entsperren“ with the PIN. Wrong PINs make the next attempt wait longer (also across starts).
#[tauri::command(async)]
pub fn applock_unlock(app: AppHandle, state: State<'_, AppState>, pin: String) -> Res<UnlockReply> {
    let now = mail_core::store::now();
    let mut attempts = applock::attempts(&state.store());
    let wait = attempts.remaining(now);
    if !wait.is_zero() {
        return Ok(UnlockReply { ok: false, wait_secs: wait.as_secs(), failures: attempts.failures });
    }
    let hash = secrets::get(applock::PIN_ENTRY)?.unwrap_or_default();
    if applock::verify_pin(&pin, &hash) {
        applock::set_attempts(&state.store(), &Default::default())?;
        set_locked(&app, false);
        return Ok(UnlockReply { ok: true, wait_secs: 0, failures: 0 });
    }
    attempts.failed(now);
    applock::set_attempts(&state.store(), &attempts)?;
    mail_core::diag::log("applock", &format!("wrong PIN ({} in a row)", attempts.failures));
    Ok(UnlockReply {
        ok: false,
        wait_secs: applock::wait_after(attempts.failures).as_secs(),
        failures: attempts.failures,
    })
}

fn os_reason() -> &'static str {
    mail_core::tr!("Arcalo Mail entsperren", "Unlock Arcalo Mail")
}

/// „Mit Windows Hello“ / „Mit Touch ID“ / „Mit Face ID“ / „Mit Fingerabdruck“.
#[tauri::command(async)]
pub fn applock_unlock_biometric(app: AppHandle, state: State<'_, AppState>) -> Res<bool> {
    if !applock::config(&state.store()).biometric || !os::verify(&app, os_reason()) {
        return Ok(false);
    }
    applock::set_attempts(&state.store(), &Default::default())?;
    set_locked(&app, false);
    Ok(true)
}

/// „PIN vergessen?“: with the system's sign-in, or with the recovery key of the encrypted
/// database. Switches the lock off and deletes the PIN.
#[tauri::command(async)]
pub fn applock_reset(app: AppHandle, state: State<'_, AppState>, recovery: Option<String>) -> Res<()> {
    let ok = match recovery.filter(|r| !r.trim().is_empty()) {
        Some(code) => {
            let key = DbKey::from_recovery_code(&code)?;
            if cipher::file_state(&state.db) != FileState::Encrypted || cipher::key().as_ref() != Some(&key) {
                return Err(invalid(cipher::wrong_key_text()));
            }
            true
        }
        None => os::verify(&app, os_reason()),
    };
    if !ok {
        return Err(invalid(mail_core::tr!("Nicht bestätigt.", "Not confirmed.")));
    }
    secrets::delete(applock::PIN_ENTRY)?;
    lock(&WATCH).has_pin = false;
    {
        let store = state.store();
        let mut c = applock::config(&store);
        c.mode = applock::LockMode::Off;
        applock::set_config(&store, &c)?;
        applock::set_attempts(&store, &Default::default())?;
    }
    mail_core::diag::log("applock", "lock reset (PIN forgotten)");
    set_locked(&app, false);
    Ok(())
}

/// „Jetzt sperren“ (command palette, settings).
#[tauri::command]
pub fn applock_lock_now(app: AppHandle, state: State<'_, AppState>) -> Res<()> {
    if !applock::config(&state.store()).enabled() || !lock(&WATCH).has_pin {
        return Err(invalid(mail_core::tr!("Die App-Sperre ist aus.", "The app lock is off.")));
    }
    request_lock(&app);
    Ok(())
}

/// End-to-end tests (debug builds): the next check sees `seconds` without input and runs now.
#[tauri::command]
pub fn applock_test_idle(app: AppHandle, seconds: u64) -> Res<()> {
    if !cfg!(debug_assertions) {
        return Err(invalid("debug builds only"));
    }
    lock(&WATCH).fake_idle = Some(Duration::from_secs(seconds));
    tick(&app);
    Ok(())
}

// ---------------------------------------------------------- system sign-in

/// Windows Hello, Touch ID or Face ID (or the device's password), the Android fingerprint or face
/// (or the screen lock): the system asks, the app only gets yes or no.
mod os {
    use tauri::AppHandle;

    #[cfg(windows)]
    pub fn available() -> Option<&'static str> {
        use windows::Security::Credentials::UI::{UserConsentVerifier, UserConsentVerifierAvailability};
        let a = UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.get()).ok()?;
        (a == UserConsentVerifierAvailability::Available).then_some("windows_hello")
    }

    #[cfg(windows)]
    pub fn verify(app: &AppHandle, reason: &str) -> bool {
        use tauri::Manager;
        use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier};
        use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
        use windows::core::{HSTRING, factory};
        let message = HSTRING::from(reason);
        // For the main window, so the prompt comes up in front of it.
        let hwnd = app.get_webview_window("main").and_then(|w| w.hwnd().ok());
        let result = match hwnd {
            Some(hwnd) => factory::<UserConsentVerifier, IUserConsentVerifierInterop>().and_then(|interop| {
                let op: windows_future::IAsyncOperation<UserConsentVerificationResult> =
                    unsafe { interop.RequestVerificationForWindowAsync(hwnd, &message)? };
                op.get()
            }),
            None => UserConsentVerifier::RequestVerificationAsync(&message).and_then(|op| op.get()),
        };
        match result {
            Ok(r) => r == UserConsentVerificationResult::Verified,
            Err(e) => {
                mail_core::diag::log("applock", &format!("Windows Hello failed: {e}"));
                false
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn available() -> Option<&'static str> {
        use objc2_local_authentication::{LAContext, LAPolicy};
        let ctx = unsafe { LAContext::new() };
        let ok = unsafe { ctx.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics) }.is_ok();
        ok.then_some(if cfg!(target_os = "ios") { "face_id" } else { "touch_id" })
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn verify(_app: &AppHandle, reason: &str) -> bool {
        use block2::RcBlock;
        use objc2::runtime::Bool;
        use objc2_foundation::{NSError, NSString};
        use objc2_local_authentication::{LAContext, LAPolicy};
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let reply: RcBlock<dyn Fn(Bool, *mut NSError)> = RcBlock::new(move |ok: Bool, _err: *mut NSError| {
            let _ = tx.send(ok.as_bool());
        });
        let ctx = unsafe { LAContext::new() };
        // Touch ID or Face ID, or the device's password when they do not recognize the user.
        unsafe {
            ctx.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthentication,
                &NSString::from_str(reason),
                &reply,
            )
        };
        rx.recv_timeout(std::time::Duration::from_secs(180)).unwrap_or(false)
    }

    #[cfg(target_os = "android")]
    pub fn available() -> Option<&'static str> {
        crate::mobile::android::biometric_available().then_some("android")
    }

    #[cfg(target_os = "android")]
    pub fn verify(_app: &AppHandle, reason: &str) -> bool {
        crate::mobile::android::biometric_verify(reason)
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "ios", target_os = "android")))]
    pub fn available() -> Option<&'static str> {
        None
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "ios", target_os = "android")))]
    pub fn verify(_app: &AppHandle, _reason: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_screens_keep_only_their_own_commands() {
        for ok in ["applock_unlock", "applock_status", "applock_unlock_biometric", "security_state", "get_settings"] {
            assert!(allowed(ok, false), "{ok}");
        }
        for no in [
            "get_message",
            "list_messages",
            "search",
            "cipher_status",
            "cipher_switch",
            "send_message",
            "set_settings",
            "export_diagnostics",
            // A new PIN or switching the lock off from behind the lock screen.
            "applock_configure",
            "keygate_unlock",
        ] {
            assert!(!allowed(no, false), "{no}");
        }
        // The recovery screen: its own commands only, not the lock's.
        assert!(allowed("keygate_unlock", true));
        assert!(allowed("security_state", true));
        assert!(!allowed("applock_unlock", true));
        assert!(!allowed("list_accounts", true));
    }
}
