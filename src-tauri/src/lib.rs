//! The Arcalo Mail shell: opens the store, keeps it as app state, hands the commands to the
//! core and the secrets to the OS credential store. The same crate builds the desktop app and,
//! through `run` as the mobile entry point, the Android and iOS apps.

mod commands;
mod secrets;

use std::sync::{Mutex, MutexGuard};

use mail_core::i18n::{Lang, lang_from_locale, set_lang};
use mail_core::settings::LanguageChoice;
use mail_core::store::Store;
use tauri::Manager;

pub struct AppState {
    store: Mutex<Store>,
}

impl AppState {
    /// The store; a panic in another command does not lock the app out of its data.
    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The language of backend texts: the setting, else the system's.
pub fn apply_language(choice: LanguageChoice) {
    set_lang(match choice {
        LanguageChoice::De => Lang::De,
        LanguageChoice::En => Lang::En,
        LanguageChoice::System => lang_from_locale(&sys_locale::get_locale().unwrap_or_default()),
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let store = Store::open(&dir.join("mail.db"))?;
            apply_language(mail_core::settings::load(&store)?.language);
            app.manage(AppState { store: Mutex::new(store) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::get_settings,
            commands::set_settings,
            commands::account_preset,
            commands::list_accounts,
            commands::add_account,
            commands::remove_account,
            commands::account_signed_in,
            commands::list_mailboxes,
            commands::list_messages,
            commands::list_thread,
            commands::get_message,
            commands::allow_remote_content,
            commands::mark_seen,
            commands::search,
            commands::send_message,
        ])
        .run(tauri::generate_context!())
        .expect("Arcalo Mail failed to start");
}
