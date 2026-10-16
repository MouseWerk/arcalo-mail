//! The commands the UI calls (`ui/src/lib/api.ts`). Each one is a thin call into `mail_core`;
//! errors reach the UI as their text.

use mail_core::Error;
use mail_core::account::{Account, AccountDraft, Auth, Provider, SecretKind, Transport, secret_key};
use mail_core::outbox::{self, Op};
use mail_core::sanitize::{Sanitized, sanitize_html, text_to_html};
use mail_core::settings::Settings;
use mail_core::store::{Mailbox, MessageDetail, MessageSummary};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{AppState, apply_language, secrets};

type Res<T> = Result<T, Error>;

#[derive(Serialize)]
pub struct AppInfo {
    version: &'static str,
    platform: &'static str,
    schema: u32,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        schema: mail_core::store::SCHEMA_VERSION,
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Res<Settings> {
    mail_core::settings::load(&state.store())
}

/// Applies at once; the UI keeps the previous value for „Rückgängig“.
#[tauri::command]
pub fn set_settings(state: State<'_, AppState>, settings: Settings) -> Res<()> {
    mail_core::settings::save(&state.store(), &settings)?;
    apply_language(settings.language);
    Ok(())
}

#[tauri::command]
pub fn account_preset(provider: Provider, email: String) -> Res<Transport> {
    mail_core::account::preset(provider, &email)
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Res<Vec<Account>> {
    state.store().accounts()
}

/// Stores the account and its password (IMAP, JMAP); OAuth accounts sign in afterwards
/// (milestone 2). When the credential store refuses, the account is not kept.
#[tauri::command]
pub fn add_account(state: State<'_, AppState>, draft: AccountDraft, password: Option<String>) -> Res<i64> {
    let store = state.store();
    let id = store.add_account(&draft)?;
    let wants_password = matches!(
        draft.transport,
        Transport::Imap { auth: Auth::Password, .. } | Transport::Jmap { auth: Auth::Password, .. }
    );
    if wants_password
        && let Some(pw) = password.filter(|p| !p.is_empty())
        && let Err(e) = secrets::set(&secret_key(id, SecretKind::Password), &pw)
    {
        store.remove_account(id)?;
        return Err(e);
    }
    Ok(id)
}

#[tauri::command]
pub fn remove_account(state: State<'_, AppState>, id: i64) -> Res<()> {
    state.store().remove_account(id)?;
    secrets::delete(&secret_key(id, SecretKind::Password))?;
    secrets::delete(&secret_key(id, SecretKind::RefreshToken))
}

/// Whether the credential store has what the account needs to connect.
#[tauri::command]
pub fn account_signed_in(id: i64) -> Res<bool> {
    Ok(secrets::get(&secret_key(id, SecretKind::Password))?.is_some()
        || secrets::get(&secret_key(id, SecretKind::RefreshToken))?.is_some())
}

#[tauri::command]
pub fn list_mailboxes(state: State<'_, AppState>, account_id: i64) -> Res<Vec<Mailbox>> {
    state.store().mailboxes(account_id)
}

#[tauri::command]
pub fn list_messages(state: State<'_, AppState>, mailbox_id: i64, limit: u32, offset: u32) -> Res<Vec<MessageSummary>> {
    state.store().messages(mailbox_id, limit.min(500), offset)
}

#[tauri::command]
pub fn list_thread(state: State<'_, AppState>, thread_id: i64) -> Res<Vec<MessageSummary>> {
    state.store().thread(thread_id)
}

#[derive(Serialize)]
pub struct MessageView {
    #[serde(flatten)]
    detail: MessageDetail,
    /// The body as sanitized HTML for the sandboxed frame.
    body: Sanitized,
}

#[tauri::command]
pub fn get_message(state: State<'_, AppState>, id: i64) -> Res<MessageView> {
    let mut detail = state.store().message(id)?;
    let body = match (&detail.html, &detail.text) {
        (Some(html), _) => sanitize_html(html, detail.remote_allowed),
        (None, Some(text)) => Sanitized { html: text_to_html(text), blocked_remote: 0 },
        (None, None) => Sanitized { html: text_to_html(&detail.summary.snippet), blocked_remote: 0 },
    };
    // The raw HTML stays in the backend.
    detail.html = None;
    Ok(MessageView { detail, body })
}

#[tauri::command]
pub fn allow_remote_content(state: State<'_, AppState>, id: i64) -> Res<()> {
    state.store().allow_remote(id)
}

/// Marks a message read or unread at once and queues the change for the server.
#[tauri::command]
pub fn mark_seen(state: State<'_, AppState>, id: i64, seen: bool) -> Res<()> {
    let store = state.store();
    store.mark_seen(id, seen)?;
    let m = store.message(id)?;
    let mb = store.mailbox(m.summary.mailbox_id)?;
    let op = Op::Flags { mailbox: mb.name, uids: vec![m.summary.uid], seen: Some(seen), flagged: None };
    outbox::enqueue(&store, mb.account_id, &op)?;
    Ok(())
}

#[tauri::command]
pub fn search(state: State<'_, AppState>, query: String, account_id: Option<i64>) -> Res<Vec<MessageSummary>> {
    mail_core::search::search(&state.store(), &query, account_id, 200)
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct ComposeDraft {
    account_id: i64,
    to: Vec<String>,
    cc: Vec<String>,
    subject: String,
    body: String,
}

/// Sending arrives with milestone 1 (MIME building, SMTP through `mail_core::network`, the
/// offline queue); until then the compose sheet keeps the draft open.
#[tauri::command]
pub fn send_message(draft: ComposeDraft) -> Res<()> {
    let _ = draft;
    Err(Error::Invalid(
        mail_core::tr!(
            "Senden ist in dieser Vorschauversion noch nicht verfügbar.",
            "Sending is not available in this preview version yet."
        )
        .into(),
    ))
}
