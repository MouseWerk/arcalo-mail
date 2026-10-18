//! The Arcalo Mail shell: opens the store, starts one worker per account (`mail_core::engine`),
//! hands the commands to the core, the secrets to the OS credential store and the workers'
//! notices to the UI (`mail-notice` events), desktop notifications and the unread badge. The
//! same crate builds the desktop app and, through `run` as the mobile entry point, the Android
//! and iOS apps; what the phones do differently is in `mobile` (the workers run while the app is
//! in front, a background check of its own otherwise, the browser session of the sign-in). A
//! clock thread does what the time tools have due (`run_clock`); the calendar has a loop of its
//! own (`calendar`). Desktop updates are `updates` (with `installer` and `portable`), the
//! Android app's update notice `mobile::apknotice`, `mailto:` links and the default mail app
//! `defaultmail`.

mod arcalo;
mod assistant;
mod background;
mod calendar;
mod commands;
mod crypto;
mod defaultmail;
#[cfg(desktop)]
mod drafts;
#[cfg(desktop)]
mod installer;
#[cfg(mobile)]
mod mobile;
#[cfg(desktop)]
mod portable;
mod secrets;
mod security;
mod tools;
#[cfg(desktop)]
mod tray;
#[cfg(desktop)]
mod updates;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use mail_core::account::{Account, Auth, SecretKind, Transport, secret_key};
use mail_core::engine::{Command, NetConnector, Notice, Status, Worker, WorkerHandle};
use mail_core::i18n::{Lang, lang_from_locale, set_lang};
use mail_core::jmap::JmapConnector;
use mail_core::settings::LanguageChoice;
use mail_core::store::Store;
use tauri::{AppHandle, Emitter, Manager};

pub struct AppState {
    store: Mutex<Store>,
    db: PathBuf,
    data_dir: PathBuf,
    workers: Mutex<HashMap<i64, Worker>>,
    /// The last status notice per account, for a UI that starts after it was sent.
    statuses: Mutex<HashMap<i64, Notice>>,
    /// Tokens of a Microsoft sign-in until the account is added (memory only).
    pending_oauth: Mutex<HashMap<String, mail_core::oauth::Tokens>>,
    /// Answers of the AI assistant on their way, by request id: set to cancel.
    ai_jobs: Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
    /// Pictures being fetched (a BIMI domain, a Gravatar address), each once at a time.
    pictures: Mutex<std::collections::HashSet<String>>,
}

impl AppState {
    /// The store; a panic in another command does not lock the app out of its data.
    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn worker(&self, account_id: i64) -> Option<WorkerHandle> {
        self.workers.lock().unwrap_or_else(|e| e.into_inner()).get(&account_id).map(Worker::handle)
    }

    /// Tells the account's worker that its queue has something new.
    pub fn kick(&self, accounts: &[i64]) {
        for a in accounts {
            if let Some(w) = self.worker(*a) {
                w.send(Command::Kick);
            }
        }
    }

    pub fn all_workers(&self) -> Vec<WorkerHandle> {
        self.workers.lock().unwrap_or_else(|e| e.into_inner()).values().map(Worker::handle).collect()
    }
}

/// The system's language tag (`ARCALO_MAIL_LOCALE` stands in for it in tests).
pub fn system_locale() -> String {
    std::env::var("ARCALO_MAIL_LOCALE")
        .ok()
        .filter(|l| !l.is_empty())
        .or_else(sys_locale::get_locale)
        .unwrap_or_default()
}

/// The language of backend texts: the setting, else the system's.
pub fn apply_language(choice: LanguageChoice) {
    set_lang(match choice {
        LanguageChoice::De => Lang::De,
        LanguageChoice::En => Lang::En,
        LanguageChoice::System => lang_from_locale(&system_locale()),
    });
}

/// The app's data folder: `ARCALO_MAIL_DATA_DIR` (tests), `data` next to a portable copy
/// (`portable.rs`), else the user's application data folder.
fn data_dir(app: &AppHandle) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(d) = std::env::var_os("ARCALO_MAIL_DATA_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    #[cfg(desktop)]
    if let Some(d) = portable::detect() {
        return Ok(d);
    }
    Ok(app.path().app_data_dir()?)
}

/// The unread messages of all inboxes on the app icon (where the platform shows a badge), and
/// on phones the home-screen widget's file.
pub fn update_badge(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let unread = state.store().unread_in_inboxes().unwrap_or(0);
    show_unread(app, unread);
    // Phones: the home-screen widget's file (Android shows it; iOS's widget is planned).
    #[cfg(mobile)]
    write_widget(&state.store(), &state.data_dir);
    #[cfg(target_os = "android")]
    mobile::android::refresh_widget();
}

/// `widget.json` next to the database: what the Android widget shows (`mail_core::widget`).
#[cfg_attr(desktop, allow(dead_code))]
pub(crate) fn write_widget(store: &Store, dir: &Path) {
    let Ok(w) = mail_core::widget::snapshot(store) else { return };
    let path = dir.join("widget.json");
    let tmp = dir.join("widget.json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec(&w).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// The desktop: the badge on the dock or taskbar icon and the count in the window title.
#[cfg(desktop)]
fn show_unread(app: &AppHandle, unread: u32) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_badge_count(if unread == 0 { None } else { Some(unread as i64) });
        let title = if unread == 0 { "Arcalo Mail".to_string() } else { format!("Arcalo Mail ({unread})") };
        let _ = w.set_title(&title);
    }
    tray::show_unread(unread);
}

/// Android: the count on the new-mail notification (launchers show it as the badge); iOS: the
/// number on the app icon.
#[cfg(mobile)]
fn show_unread(_app: &AppHandle, unread: u32) {
    mobile::show_unread(unread);
}

/// A notification for new mail, when the window is not in front (on a phone the workers run
/// only while the app is in front; the background check tells about mail otherwise).
fn notify_new_mail(app: &AppHandle, account_id: i64, messages: &[i64]) {
    use tauri_plugin_notification::NotificationExt;
    let Some(state) = app.try_state::<AppState>() else { return };
    if !mail_core::settings::load(&state.store()).map(|s| s.notifications).unwrap_or(true) {
        return;
    }
    #[cfg(desktop)]
    if app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false) {
        return;
    }
    #[cfg(mobile)]
    if mobile::in_front() {
        return;
    }
    let store = state.store();
    // Focus and the Screener decide which new messages are worth telling about.
    let messages = mail_core::triage::worth_telling(&store, account_id, messages).unwrap_or_else(|_| messages.to_vec());
    let list = store.summaries(&messages).unwrap_or_default();
    let account = store.account(account_id).ok();
    let private = security::private(&store);
    drop(store);
    // With the app lock on: how many, nothing else, no buttons.
    if private {
        if !list.is_empty() {
            let (title, body) = mail_core::applock::private_notification(list.len());
            let _ = app.notification().builder().title(title).body(body).show();
        }
        return;
    }
    let Some((title, body)) = background::new_mail_text(&list, account.as_ref()) else { return };
    // One message: its buttons (archive, mark read, reply) where the desktop shows them.
    #[cfg(any(windows, target_os = "linux"))]
    if let [m] = list.as_slice()
        && notify_with_actions(app, &title, &body, m.id)
    {
        return;
    }
    let _ = app.notification().builder().title(title).body(body).show();
}

/// A notification with buttons (Linux: the notification server's actions; Windows: the toast's
/// buttons); `false` when the desktop has none (no notification server), so the plain one goes.
#[cfg(any(windows, target_os = "linux"))]
fn notify_with_actions(app: &AppHandle, title: &str, body: &str, message_id: i64) -> bool {
    let mut n = notify_rust::Notification::new();
    n.summary(title)
        .body(body)
        .action("default", mail_core::tr!("Öffnen", "Open"))
        .action("archive", mail_core::tr!("Archivieren", "Archive"))
        .action("read", mail_core::tr!("Gelesen", "Mark read"))
        .action("reply", mail_core::tr!("Antworten", "Reply"));
    #[cfg(target_os = "linux")]
    n.appname("Arcalo Mail");
    #[cfg(windows)]
    n.app_id(&app.config().identifier);
    let Ok(handle) = n.show() else { return false };
    let app = app.clone();
    let _ = std::thread::Builder::new().name("notification".into()).spawn(move || {
        handle.wait_for_action(|action| notification_action(&app, message_id, action));
    });
    true
}

/// What a notification's button asked for: archived or read here (the worker carries it to the
/// server), or the message opened (to reply to it).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn notification_action(app: &AppHandle, message_id: i64, action: &str) {
    let Some(state) = app.try_state::<AppState>() else { return };
    match action {
        "archive" | "read" => {
            let accounts = match mail_core::actions::quick_action(&state.store(), message_id, action) {
                Ok(a) => a,
                Err(e) => {
                    mail_core::diag::log("notification", &e.to_string());
                    return;
                }
            };
            state.kick(&accounts);
            for a in &accounts {
                let _ = app.emit("mail-notice", &Notice::Changed { account_id: *a, mailboxes: vec![] });
            }
            update_badge(app);
        }
        "default" | "reply" => {
            #[cfg(desktop)]
            tray::show_main(app);
            commands::open_message(Some(app), message_id, action == "reply");
        }
        _ => {}
    }
}

/// How often the clock looks at the time tools while the app runs (`ARCALO_MAIL_TICK_SECS`
/// shortens it for the e2e tests).
fn tick_interval() -> std::time::Duration {
    let secs = std::env::var("ARCALO_MAIL_TICK_SECS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(10);
    std::time::Duration::from_secs(secs.clamp(1, 60))
}

/// Does what the time tools have due (`mail_core::later::tick`): messages to send later go to
/// the queue, snoozed messages come back, reminders without an answer are told. The accounts'
/// workers hear of their queue, the UI of the changed lists (`mail-notice`) and of the
/// reminders (`reminders`).
pub fn tick_now(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let (tick, notifications, private) = {
        let store = state.store();
        let tick = match mail_core::later::tick(&store, mail_core::store::now()) {
            Ok(t) => t,
            Err(e) => {
                mail_core::diag::log("clock", &format!("time tools: {e}"));
                return;
            }
        };
        (tick, mail_core::settings::load(&store).map(|s| s.notifications).unwrap_or(true), security::private(&store))
    };
    if !tick.changed() {
        return;
    }
    state.kick(&tick.accounts);
    for a in &tick.accounts {
        let _ = app.emit("mail-notice", &Notice::Changed { account_id: *a, mailboxes: vec![] });
    }
    if !tick.reminders.is_empty() {
        let _ = app.emit("reminders", &tick.reminders);
        #[cfg(desktop)]
        let in_front = app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false);
        #[cfg(mobile)]
        let in_front = mobile::in_front();
        if notifications && !in_front {
            use tauri_plugin_notification::NotificationExt;
            for r in &tick.reminders {
                let (title, body) =
                    if private { mail_core::applock::private_reminder() } else { background::reminder_text(r) };
                let _ = app.notification().builder().title(title).body(body).show();
            }
        }
    }
    update_badge(app);
}

/// The clock: [`tick_now`] at the start (a message whose time came while the computer was off
/// goes out now) and every few seconds while the app runs. Started with the workers, so a
/// phone launched only for its background check leaves the time tools to that check.
fn run_clock(app: AppHandle) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("clock".into()).spawn(move || {
        loop {
            tick_now(&app);
            std::thread::sleep(tick_interval());
        }
    });
}

/// How a worker reaches an account's servers, with its secret from the credential store.
pub(crate) enum Connection {
    Net(NetConnector),
    Jmap(JmapConnector),
    Pop3(mail_core::pop3::Pop3Connector),
}

/// Why an account has no [`Connection`].
pub(crate) enum NoConnection {
    /// No password or refresh token stored: the user signs in.
    SignIn,
    /// The account cannot run in this build (a provider without a client id).
    Unusable(String),
}

/// The connection of an account (the worker's, and the phones' background check).
pub(crate) fn connection_for(account: &Account, db: &Path) -> Result<Connection, NoConnection> {
    let password = || secrets::get(&secret_key(account.id, SecretKind::Password)).ok().flatten();
    match &account.transport {
        Transport::Imap { auth: Auth::OAuth2(provider), .. } => {
            let Some(refresh) = secrets::get(&secret_key(account.id, SecretKind::RefreshToken)).ok().flatten() else {
                return Err(NoConnection::SignIn);
            };
            let cfg = mail_core::oauth::config_for(*provider).map_err(|e| NoConnection::Unusable(e.to_string()))?;
            // A rotated refresh token goes to the credential store at once.
            let key = secret_key(account.id, SecretKind::RefreshToken);
            let save: mail_core::oauth::OnRefreshToken = Arc::new(move |t: &str| {
                if let Err(e) = secrets::set(&key, t) {
                    mail_core::diag::log("secrets", &format!("refresh token not stored: {e}"));
                }
            });
            let tokens = mail_core::oauth::TokenSource::new(cfg, refresh, Some(save));
            NetConnector::with_oauth(account, tokens)
                .map(Connection::Net)
                .map_err(|e| NoConnection::Unusable(e.to_string()))
        }
        Transport::Jmap { .. } => {
            let Some(password) = password() else { return Err(NoConnection::SignIn) };
            JmapConnector::new(account, password, db.to_path_buf())
                .map(Connection::Jmap)
                .map_err(|e| NoConnection::Unusable(e.to_string()))
        }
        Transport::Pop3 { .. } => {
            let Some(password) = password() else { return Err(NoConnection::SignIn) };
            mail_core::pop3::Pop3Connector::new(account, password, db.to_path_buf())
                .map(Connection::Pop3)
                .map_err(|e| NoConnection::Unusable(e.to_string()))
        }
        _ => {
            let Some(password) = password() else { return Err(NoConnection::SignIn) };
            NetConnector::new(account, password).map(Connection::Net).map_err(|e| NoConnection::Unusable(e.to_string()))
        }
    }
}

/// Starts the worker of an account (its password from the credential store); without one the
/// account waits for „Anmelden“.
pub fn start_worker(app: &AppHandle, account: &Account) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let sink_app = app.clone();
    let sink: mail_core::engine::Sink = Arc::new(move |notice: Notice| {
        match &notice {
            Notice::Status { account_id, .. } => {
                if let Some(s) = sink_app.try_state::<AppState>() {
                    s.statuses.lock().unwrap_or_else(|e| e.into_inner()).insert(*account_id, notice.clone());
                }
            }
            Notice::NewMail { account_id, messages } => {
                notify_new_mail(&sink_app, *account_id, messages);
                assistant::judge_new_mail(&sink_app, *account_id, messages);
                // Invitations among them change the calendar.
                calendar::kick();
            }
            _ => {}
        }
        if matches!(notice, Notice::Changed { .. } | Notice::NewMail { .. }) {
            update_badge(&sink_app);
        }
        let _ = sink_app.emit("mail-notice", &notice);
    });
    let tell = |status: Status, error: Option<String>| {
        let n = Notice::Status { account_id: account.id, status, problem: None, error };
        state.statuses.lock().unwrap_or_else(|e| e.into_inner()).insert(account.id, n.clone());
        let _ = app.emit("mail-notice", &n);
    };
    let spawned = match connection_for(account, &state.db) {
        Ok(Connection::Net(c)) => Worker::spawn(account.id, state.db.clone(), c, sink),
        Ok(Connection::Jmap(c)) => Worker::spawn(account.id, state.db.clone(), c, sink),
        Ok(Connection::Pop3(c)) => Worker::spawn(account.id, state.db.clone(), c, sink),
        Err(NoConnection::SignIn) => return tell(Status::SignIn, None),
        Err(NoConnection::Unusable(e)) => return tell(Status::Stopped, Some(e)),
    };
    match spawned {
        Ok(w) => {
            state.workers.lock().unwrap_or_else(|e| e.into_inner()).insert(account.id, w);
        }
        Err(e) => eprintln!("worker of account {}: {e}", account.id),
    }
}

/// Starts the workers of every account (at start; on a phone also when the app comes to the
/// front again).
pub fn start_workers(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    run_clock(app.clone());
    let accounts = state.store().accounts().unwrap_or_default();
    for a in &accounts {
        if state.worker(a.id).is_none() {
            start_worker(app, a);
        }
    }
}

/// Stops every worker (a phone app going to the background: no connection stays open there).
pub fn stop_workers(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.workers.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Hands the network profiles and their proxy passwords (from the credential store) to the
/// core; every connection after this uses them.
pub fn apply_network(net: &mail_core::network::NetworkSettings) {
    let passwords = net
        .profiles
        .iter()
        .filter(|p| !p.proxy_user.trim().is_empty())
        .filter_map(|p| {
            let pw = secrets::get(&mail_core::network::proxy_password_key(&p.id)).ok().flatten()?;
            Some((p.id.clone(), pw))
        })
        .collect();
    mail_core::network::configure(net.clone(), passwords);
}

/// Every account reconnects now (woke up, another network, new proxy settings).
pub fn reconnect_all(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        for w in state.all_workers() {
            w.send(Command::Reconnect);
        }
    }
}

/// Samples the clocks and the network addresses every few seconds; after sleep or a network
/// change the accounts reconnect at once (`mail_core::netwatch`).
fn watch_network(app: AppHandle) {
    let _ = std::thread::Builder::new().name("netwatch".into()).spawn(move || {
        let mut watch = mail_core::netwatch::Watch::default();
        loop {
            let wall =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
            let addresses: Vec<String> = if_addrs::get_if_addrs()
                .map(|list| list.into_iter().filter(|i| !i.is_loopback()).map(|i| i.ip().to_string()).collect())
                .unwrap_or_default();
            let changes = watch.sample(wall, std::time::Instant::now(), addresses);
            if mail_core::netwatch::reconnect(&changes) {
                mail_core::diag::log("network", &format!("{changes:?}: reconnecting"));
                reconnect_all(&app);
            }
            std::thread::sleep(mail_core::netwatch::INTERVAL);
        }
    });
}

/// Stops the worker of an account (removed, or its servers changed).
pub fn stop_worker(app: &AppHandle, account_id: i64) {
    if let Some(state) = app.try_state::<AppState>() {
        state.workers.lock().unwrap_or_else(|e| e.into_inner()).remove(&account_id);
        state.statuses.lock().unwrap_or_else(|e| e.into_inner()).remove(&account_id);
    }
}

/// The app runs: network profiles, the account workers, the address books, the calendar, the
/// badge, the network watch and the app lock's clock. At the start, or once the recovery screen
/// opened the database.
pub fn begin(app: &AppHandle, settings: mail_core::settings::Settings) {
    #[cfg(mobile)]
    let in_front = mobile::in_front();
    #[cfg(desktop)]
    let in_front = true;
    begin_with(app, settings, in_front);
}

fn begin_with(app: &AppHandle, settings: mail_core::settings::Settings, in_front: bool) {
    apply_network(&settings.network);
    if in_front {
        start_workers(app);
        tools::sync_all_books(app);
        calendar::start_loop(app);
    }
    update_badge(app);
    watch_network(app.clone());
    security::spawn_watch(app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(mobile)]
    mobile::before_launch();
    #[cfg(desktop)]
    portable::prepare();
    let builder = tauri::Builder::default();
    // A second start brings the window up; with a `mailto:` address it opens the compose sheet.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        tray::show_main(app);
        defaultmail::open_from_args(Some(app), &args);
    }));
    // Only in release builds with the update key; others never contact the update feed.
    #[cfg(desktop)]
    let builder = match updates::plugin() {
        Some(updater) => builder.plugin(updater),
        None => builder,
    };
    #[cfg(mobile)]
    let builder = builder.on_window_event(|window, event| mobile::window_event(window.app_handle(), event));
    // Closing the window: the message being written is saved as a draft first.
    #[cfg(desktop)]
    let builder = builder.on_window_event(|window, event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event
            && window.label() == "main"
            && drafts::needs_save()
        {
            api.prevent_close();
            let w = window.clone();
            drafts::save_then(window.app_handle(), move |_| {
                let _ = w.destroy();
            });
        }
    });
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            #[cfg(mobile)]
            mobile::setup(app.handle());
            let dir = data_dir(app.handle())?;
            std::fs::create_dir_all(&dir)?;
            let db = dir.join(mail_core::cipher::DB_FILE);
            // The key of an encrypted database comes from the credential store before the store
            // opens; without it (or with a wrong one) the recovery screen shows on an empty store.
            secrets::init(&dir);
            let prepared = security::prepare(&dir);
            security::set_notice(prepared.notice);
            security::set_gate(prepared.blocked);
            let store = if prepared.blocked.is_some() { Store::open_in_memory()? } else { Store::open(&db)? };
            if prepared.blocked.is_none() {
                security::opened(&dir);
            }
            mail_core::outbox::recover(&store)?;
            let settings = mail_core::settings::load(&store)?;
            apply_language(settings.language);
            crypto::register();
            app.manage(AppState {
                store: Mutex::new(store),
                db,
                data_dir: dir,
                workers: Mutex::default(),
                statuses: Mutex::default(),
                pending_oauth: Mutex::default(),
                ai_jobs: Mutex::default(),
                pictures: Mutex::default(),
            });
            // Started by an update (or after a setup that did not finish), and a download of an
            // earlier session that waits for the quit.
            let dir = app.state::<AppState>().data_dir.clone();
            #[cfg(desktop)]
            {
                let after = mail_core::update::take_restart_marker(&dir, &updates::current_version(app.handle()));
                app.manage(updates::Updates::after(after));
                updates::load_staged(app.handle());
            }
            #[cfg(target_os = "android")]
            {
                app.manage(mobile::apknotice::Notice::new(app.handle(), &dir));
                mobile::apknotice::spawn_scheduler(app.handle().clone());
            }
            // Started for an e-mail link (Windows, Linux): the UI takes it once it is ready.
            #[cfg(desktop)]
            defaultmail::open_from_args(None, &std::env::args().collect::<Vec<_>>());
            // A phone launched in the background for its check starts no workers.
            #[cfg(mobile)]
            let in_front = mobile::launched_in_front();
            #[cfg(desktop)]
            let in_front = true;
            if security::gate().is_none() {
                security::init_lock(app.handle());
                let handle = app.handle().clone();
                // On a phone the credential store must not be asked on the main thread.
                #[cfg(desktop)]
                begin_with(&handle, settings, in_front);
                #[cfg(mobile)]
                std::thread::spawn(move || begin_with(&handle, settings, in_front));
            }
            // The e2e tests of the phone and tablet layouts size the window like a phone.
            #[cfg(desktop)]
            if std::env::var("ARCALO_MAIL_TOUCH").is_ok_and(|v| v == "1")
                && let Some(w) = app.get_webview_window("main")
            {
                let _ = w.set_min_size(None::<tauri::Size>);
            }
            #[cfg(desktop)]
            if let Err(e) = tray::setup(app.handle()) {
                mail_core::diag::log("tray", &format!("no tray icon: {e}"));
            }
            Ok(())
        })
        .invoke_handler(security::guard(tauri::generate_handler![
            commands::app_info,
            commands::get_settings,
            commands::set_settings,
            commands::discover,
            commands::check_account,
            commands::add_account,
            commands::update_account,
            commands::update_servers,
            commands::remove_account,
            commands::sync_details,
            commands::sign_in,
            commands::microsoft_sign_in,
            commands::add_microsoft_account,
            commands::microsoft_reauth,
            commands::check_jmap,
            commands::set_proxy_password,
            commands::system_proxy,
            commands::export_diagnostics,
            commands::list_accounts,
            commands::list_mailboxes,
            commands::list_messages,
            commands::list_thread,
            commands::get_message,
            commands::allow_remote_content,
            commands::allow_sender,
            commands::forbid_sender,
            commands::allowed_senders,
            commands::set_flags,
            commands::move_messages,
            commands::archive_messages,
            commands::delete_messages,
            commands::mark_spam,
            commands::undo,
            commands::search,
            commands::server_search,
            tools::suggest_contacts,
            commands::send_message,
            commands::cancel_send,
            commands::save_draft,
            commands::discard_draft,
            commands::draft_of,
            commands::attachments_by_id,
            #[cfg(desktop)]
            drafts::drafts_listen,
            #[cfg(desktop)]
            drafts::drafts_saved,
            commands::file_infos,
            commands::open_attachment,
            commands::save_attachment,
            commands::attachment_preview,
            commands::create_folder,
            commands::rename_folder,
            commands::delete_folder,
            commands::sync_now,
            commands::outbox_entries,
            commands::retry_outbox,
            commands::discard_outbox,
            commands::set_in_front,
            commands::send_later,
            commands::scheduled_messages,
            commands::unschedule,
            commands::send_scheduled_now,
            commands::inbox_list,
            commands::inbox_counts,
            commands::bundle_messages,
            commands::set_sender_category,
            commands::screener_senders,
            commands::screener_allow,
            commands::screener_block,
            commands::screener_forget,
            commands::blocked_senders,
            commands::subscriptions,
            commands::unsubscribe,
            commands::unsubscribe_plan,
            commands::unsubscribed_by_link,
            commands::set_pinned,
            commands::set_read_later,
            commands::read_later,
            commands::snooze,
            commands::unsnooze,
            commands::snoozed,
            commands::set_followup,
            commands::dismiss_followups,
            commands::followups,
            commands::time_counts,
            commands::tick,
            assistant::ai_run,
            assistant::ai_cancel,
            assistant::ai_destination,
            assistant::ai_ask_providers,
            assistant::ai_thread,
            assistant::ai_models,
            assistant::ai_set_key,
            assistant::ai_has_key,
            assistant::ai_tone,
            assistant::ai_learn_tone,
            assistant::ai_forget_tone,
            assistant::ai_forget_results,
            assistant::set_important,
            assistant::ai_remind,
            assistant::ai_calendar,
            calendar::calendar_overview,
            calendar::calendar_events,
            calendar::calendar_create,
            calendar::calendar_update,
            calendar::calendar_delete,
            calendar::calendar_undo,
            calendar::calendar_resolve,
            calendar::calendar_discard,
            calendar::calendar_set,
            calendar::calendar_sync,
            calendar::caldav_add,
            calendar::ics_subscribe,
            calendar::calendar_account,
            calendar::calendar_remove_source,
            calendar::invitation_answer,
            crypto::crypto_keys,
            crypto::pgp_generate,
            crypto::pgp_import,
            crypto::pgp_needs_passphrase,
            crypto::pgp_unlock,
            crypto::pgp_locked,
            crypto::smime_trust,
            crypto::pgp_export,
            crypto::pgp_lookup,
            crypto::smime_import,
            crypto::crypto_remove,
            crypto::crypto_abilities,
            crypto::save_protected_attachment,
            arcalo::arcalo_status,
            arcalo::send_to_arcalo,
            arcalo::save_markdown,
            commands::take_pending_open,
            tools::rules_list,
            tools::rules_server,
            tools::rule_unsupported,
            tools::rule_save,
            tools::rule_delete,
            tools::rule_enable,
            tools::rules_reorder,
            tools::rule_preview,
            tools::rule_run,
            tools::templates_list,
            tools::template_save,
            tools::template_delete,
            tools::template_render,
            tools::contact_card,
            tools::address_books,
            tools::carddav_add,
            tools::address_book_sync,
            tools::address_book_remove,
            tools::microsoft_contacts,
            tools::picture,
            tools::forget_pictures,
            tools::pop3_preset,
            tools::hub_list,
            tools::hub_senders,
            defaultmail::take_pending_mailto,
            defaultmail::default_mail_status,
            defaultmail::default_mail_set,
            #[cfg(desktop)]
            updates::update_status,
            #[cfg(desktop)]
            updates::update_check,
            #[cfg(desktop)]
            updates::update_download,
            #[cfg(desktop)]
            updates::update_pause,
            #[cfg(desktop)]
            updates::update_install,
            #[cfg(desktop)]
            updates::update_restart_now,
            #[cfg(desktop)]
            updates::update_skip,
            #[cfg(desktop)]
            updates::update_unskip,
            #[cfg(desktop)]
            updates::update_remind,
            #[cfg(desktop)]
            updates::update_whats_new_seen,
            #[cfg(target_os = "android")]
            mobile::apknotice::apk_update_status,
            #[cfg(target_os = "android")]
            mobile::apknotice::apk_update_check,
            #[cfg(target_os = "android")]
            mobile::apknotice::apk_whats_new_seen,
            commands::phishing_ok,
            security::security_state,
            security::keygate_unlock,
            security::keygate_quit,
            security::cipher_status,
            security::cipher_recovery_key,
            security::cipher_recovery_save,
            security::cipher_switch,
            security::cipher_password,
            security::cipher_drop_old,
            security::applock_status,
            security::applock_configure,
            security::applock_unlock,
            security::applock_unlock_biometric,
            security::applock_reset,
            security::applock_lock_now,
            security::applock_flushed,
            security::applock_activity,
            security::applock_test_idle,
        ]))
        .build(tauri::generate_context!())
        .expect("Arcalo Mail failed to start")
        .run(on_run_event);
}

/// The app's life: closing the window or quitting installs a downloaded update first (desktop);
/// macOS hands over `mailto:` addresses here.
#[allow(unused_variables)]
fn on_run_event(app: &AppHandle, event: tauri::RunEvent) {
    match event {
        // Quitting (Cmd+Q, the last window closed): the message being written is saved as a
        // draft first, then the quit goes on as below.
        #[cfg(desktop)]
        tauri::RunEvent::ExitRequested { code: None, api, .. } if drafts::needs_save() => {
            api.prevent_exit();
            drafts::save_then(app, |app| {
                if updates::installs_on_quit(app) {
                    updates::quit(app);
                } else {
                    app.exit(0);
                }
            });
        }
        // The window was closed (not `app.exit`): with an update to install, the app stays until
        // it is in place.
        #[cfg(desktop)]
        tauri::RunEvent::ExitRequested { code: None, api, .. } if updates::installs_on_quit(app) => {
            api.prevent_exit();
            updates::quit(app);
        }
        #[cfg(desktop)]
        tauri::RunEvent::Exit => updates::on_exit(app),
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Opened { urls } => {
            for url in urls {
                if let Some(m) = mail_core::mailto::parse(url.as_str()) {
                    defaultmail::open(Some(app), m);
                }
            }
        }
        _ => {}
    }
}
