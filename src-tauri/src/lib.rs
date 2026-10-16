//! The Arcalo Mail shell: opens the store, starts one worker per account (`mail_core::engine`),
//! hands the commands to the core, the secrets to the OS credential store and the workers'
//! notices to the UI (`mail-notice` events), desktop notifications and the unread badge. The
//! same crate builds the desktop app and, through `run` as the mobile entry point, the Android
//! and iOS apps; what the phones do differently is in `mobile` (the workers run while the app is
//! in front, a background check of its own otherwise, the browser session of the sign-in).

mod background;
mod commands;
#[cfg(mobile)]
mod mobile;
mod secrets;
#[cfg(desktop)]
mod tray;

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

/// The app's data folder (`ARCALO_MAIL_DATA_DIR` for tests and portable use).
fn data_dir(app: &AppHandle) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(d) = std::env::var_os("ARCALO_MAIL_DATA_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    Ok(app.path().app_data_dir()?)
}

/// The unread messages of all inboxes on the app icon (where the platform shows a badge).
pub fn update_badge(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let unread = state.store().unread_in_inboxes().unwrap_or(0);
    show_unread(app, unread);
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
    let list = store.summaries(messages).unwrap_or_default();
    let account = store.account(account_id).ok();
    drop(store);
    let Some((title, body)) = background::new_mail_text(&list, account.as_ref()) else { return };
    let _ = app.notification().builder().title(title).body(body).show();
}

/// How a worker reaches an account's servers, with its secret from the credential store.
pub(crate) enum Connection {
    Net(NetConnector),
    Jmap(JmapConnector),
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
            Notice::NewMail { account_id, messages } => notify_new_mail(&sink_app, *account_id, messages),
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(mobile)]
    mobile::before_launch();
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = { builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app))) };
    #[cfg(mobile)]
    let builder = builder.on_window_event(|window, event| mobile::window_event(window.app_handle(), event));
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            #[cfg(mobile)]
            mobile::setup(app.handle());
            let dir = data_dir(app.handle())?;
            std::fs::create_dir_all(&dir)?;
            let db = dir.join("mail.db");
            let store = Store::open(&db)?;
            mail_core::outbox::recover(&store)?;
            let settings = mail_core::settings::load(&store)?;
            apply_language(settings.language);
            secrets::init(&dir);
            app.manage(AppState {
                store: Mutex::new(store),
                db,
                data_dir: dir,
                workers: Mutex::default(),
                statuses: Mutex::default(),
                pending_oauth: Mutex::default(),
            });
            // A phone launched in the background for its check starts no workers.
            #[cfg(mobile)]
            let in_front = mobile::launched_in_front();
            #[cfg(desktop)]
            let in_front = true;
            let handle = app.handle().clone();
            let begin = move || {
                apply_network(&settings.network);
                if in_front {
                    start_workers(&handle);
                }
                update_badge(&handle);
                watch_network(handle);
            };
            // On a phone the credential store must not be asked on the main thread.
            #[cfg(desktop)]
            begin();
            #[cfg(mobile)]
            std::thread::spawn(begin);
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
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::get_settings,
            commands::set_settings,
            commands::discover,
            commands::check_account,
            commands::add_account,
            commands::update_account,
            commands::update_servers,
            commands::remove_account,
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
            commands::suggest_contacts,
            commands::send_message,
            commands::cancel_send,
            commands::save_draft,
            commands::discard_draft,
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
        ])
        .run(tauri::generate_context!())
        .expect("Arcalo Mail failed to start");
}
