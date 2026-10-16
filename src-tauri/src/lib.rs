//! The Arcalo Mail shell: opens the store, starts one worker per account (`mail_core::engine`),
//! hands the commands to the core, the secrets to the OS credential store and the workers'
//! notices to the UI (`mail-notice` events), desktop notifications and the unread badge. The
//! same crate builds the desktop app and, through `run` as the mobile entry point, the Android
//! and iOS apps.

mod commands;
mod secrets;
#[cfg(desktop)]
mod tray;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use mail_core::account::{Account, Auth, SecretKind, Transport, secret_key};
use mail_core::engine::{Command, NetConnector, Notice, Status, Worker, WorkerHandle};
use mail_core::i18n::{Lang, lang_from_locale, set_lang};
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

/// Android and iOS have no window badge in Tauri: the launcher badge comes with the
/// notifications of milestone 3 (Android: the count on the new-mail notification; iOS:
/// `applicationIconBadgeNumber` through a small plugin, docs/plan.md). Until then nothing is
/// shown.
#[cfg(mobile)]
fn show_unread(_app: &AppHandle, _unread: u32) {}

/// A desktop notification for new mail, when the window is not in front.
fn notify_new_mail(app: &AppHandle, account_id: i64, messages: &[i64]) {
    use tauri_plugin_notification::NotificationExt;
    let Some(state) = app.try_state::<AppState>() else { return };
    if !mail_core::settings::load(&state.store()).map(|s| s.notifications).unwrap_or(true) {
        return;
    }
    if app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false) {
        return;
    }
    let store = state.store();
    let list = store.summaries(messages).unwrap_or_default();
    let Some(first) = list.first() else { return };
    let account = store.account(account_id).ok();
    let (title, body) = if list.len() == 1 {
        let from = first.from.as_ref().map(|a| a.name.clone().unwrap_or_else(|| a.email.clone())).unwrap_or_default();
        (from, first.subject.clone())
    } else {
        (
            mail_core::trf!("{n} neue Nachrichten", "{n} new messages", n = list.len()),
            account.map(|a| if a.name.is_empty() { a.email } else { a.name }).unwrap_or_default(),
        )
    };
    drop(store);
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Starts the worker of an account (its password from the credential store); without one the
/// account waits for „Anmelden“.
pub fn start_worker(app: &AppHandle, account: &Account) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let password = secrets::get(&secret_key(account.id, SecretKind::Password)).ok().flatten();
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
    let ask = |problem: Option<mail_core::network::Problem>| {
        let n = Notice::Status { account_id: account.id, status: Status::SignIn, problem, error: None };
        state.statuses.lock().unwrap_or_else(|e| e.into_inner()).insert(account.id, n.clone());
        let _ = app.emit("mail-notice", &n);
    };
    let spawned = match &account.transport {
        Transport::Imap { auth: Auth::OAuth2(provider), .. } => {
            let Some(refresh) = secrets::get(&secret_key(account.id, SecretKind::RefreshToken)).ok().flatten() else {
                return ask(None);
            };
            let cfg = match mail_core::oauth::config_for(*provider) {
                Ok(c) => c,
                Err(e) => {
                    let n = Notice::Status {
                        account_id: account.id,
                        status: Status::Stopped,
                        problem: None,
                        error: Some(e.to_string()),
                    };
                    state.statuses.lock().unwrap_or_else(|e| e.into_inner()).insert(account.id, n.clone());
                    let _ = app.emit("mail-notice", &n);
                    return;
                }
            };
            // A rotated refresh token goes to the credential store at once.
            let key = secret_key(account.id, SecretKind::RefreshToken);
            let save: mail_core::oauth::OnRefreshToken = Arc::new(move |t: &str| {
                if let Err(e) = secrets::set(&key, t) {
                    mail_core::diag::log("secrets", &format!("refresh token not stored: {e}"));
                }
            });
            let tokens = mail_core::oauth::TokenSource::new(cfg, refresh, Some(save));
            match NetConnector::with_oauth(account, tokens) {
                Ok(c) => Worker::spawn(account.id, state.db.clone(), c, sink),
                Err(_) => return,
            }
        }
        Transport::Jmap { .. } => {
            let Some(password) = password else { return ask(None) };
            match mail_core::jmap::JmapConnector::new(account, password, state.db.clone()) {
                Ok(c) => Worker::spawn(account.id, state.db.clone(), c, sink),
                Err(_) => return,
            }
        }
        _ => {
            let Some(password) = password else { return ask(None) };
            match NetConnector::new(account, password) {
                Ok(c) => Worker::spawn(account.id, state.db.clone(), c, sink),
                Err(_) => return,
            }
        }
    };
    match spawned {
        Ok(w) => {
            state.workers.lock().unwrap_or_else(|e| e.into_inner()).insert(account.id, w);
        }
        Err(e) => eprintln!("worker of account {}: {e}", account.id),
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
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = { builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app))) };
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let dir = data_dir(app.handle())?;
            std::fs::create_dir_all(&dir)?;
            let db = dir.join("mail.db");
            let store = Store::open(&db)?;
            mail_core::outbox::recover(&store)?;
            let settings = mail_core::settings::load(&store)?;
            apply_language(settings.language);
            secrets::init(&dir);
            apply_network(&settings.network);
            let accounts = store.accounts()?;
            app.manage(AppState {
                store: Mutex::new(store),
                db,
                data_dir: dir,
                workers: Mutex::default(),
                statuses: Mutex::default(),
                pending_oauth: Mutex::default(),
            });
            for a in &accounts {
                start_worker(app.handle(), a);
            }
            update_badge(app.handle());
            watch_network(app.handle().clone());
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
        ])
        .run(tauri::generate_context!())
        .expect("Arcalo Mail failed to start");
}
