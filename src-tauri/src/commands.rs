//! The commands the UI calls (`ui/src/lib/api.ts`). Each one is a thin call into `mail_core`
//! (the store, `actions`, the account's worker); errors reach the UI as their text.

use base64::Engine;
use mail_core::Error;
use mail_core::account::{Account, AccountDraft, Auth, SecretKind, Transport, secret_key};
use mail_core::actions::{self, MessageView, Undo};
use mail_core::autoconfig::{Discovered, Lookup};
use mail_core::compose::{Draft, FileAttachment, mime_for};
use mail_core::engine::{CheckResult, Command, Notice};
use mail_core::outbox;
use mail_core::settings::Settings;
use mail_core::store::{Contact, Mailbox, MessageSummary};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::{AppState, apply_language, secrets, start_worker, stop_worker, update_badge};

type Res<T> = Result<T, Error>;

#[derive(Serialize)]
pub struct AppInfo {
    version: &'static str,
    platform: &'static str,
    schema: u32,
    /// The system's language tag (the UI's „Wie das System“).
    locale: String,
    secret_store: secrets::Kind,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        schema: mail_core::store::SCHEMA_VERSION,
        locale: crate::system_locale(),
        secret_store: secrets::kind(),
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

// ---------------------------------------------------------------------------- accounts

/// The servers of an address (presets, autoconfig, provider database, DNS, a guess).
#[tauri::command]
pub async fn discover(email: String) -> Res<Discovered> {
    mail_core::autoconfig::discover(&email, &Lookup::default()).await
}

/// Signs in to both servers with the settings of the setup form.
#[tauri::command]
pub async fn check_account(transport: Transport, password: String) -> Res<CheckResult> {
    if let Transport::Imap { imap, smtp, .. } = &transport {
        mail_core::network::check_server(imap)?;
        mail_core::network::check_server(smtp)?;
    }
    Ok(mail_core::engine::check_account(&transport, &password).await)
}

#[derive(Serialize)]
pub struct AccountInfo {
    #[serde(flatten)]
    account: Account,
    /// The worker's last status (`mail-notice` events bring the next ones).
    status: Option<Notice>,
    /// Steps waiting in the offline queue.
    pending: u32,
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Res<Vec<AccountInfo>> {
    let store = state.store();
    let statuses = state.statuses.lock().unwrap_or_else(|e| e.into_inner()).clone();
    store
        .accounts()?
        .into_iter()
        .map(|a| {
            let pending = outbox::pending(&store, a.id)?;
            Ok(AccountInfo { status: statuses.get(&a.id).cloned(), pending, account: a })
        })
        .collect()
}

/// Stores the account and its password, and starts syncing. When the credential store refuses,
/// the account is not kept.
#[tauri::command]
pub fn add_account(app: AppHandle, state: State<'_, AppState>, draft: AccountDraft, password: String) -> Res<Account> {
    let account = {
        let store = state.store();
        let id = store.add_account(&draft)?;
        let wants_password = matches!(draft.transport, Transport::Imap { auth: Auth::Password, .. });
        if wants_password
            && !password.is_empty()
            && let Err(e) = secrets::set(&secret_key(id, SecretKind::Password), &password)
        {
            store.remove_account(id)?;
            return Err(e);
        }
        store.account(id)?
    };
    start_worker(&app, &account);
    Ok(account)
}

#[tauri::command]
pub fn update_account(
    state: State<'_, AppState>,
    id: i64,
    name: String,
    sender_name: String,
    signature: String,
) -> Res<()> {
    state.store().update_account(id, &name, &sender_name, &signature)
}

/// New server settings (and password); the worker starts again with them.
#[tauri::command]
pub fn update_servers(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    transport: Transport,
    password: Option<String>,
) -> Res<()> {
    state.store().update_transport(id, &transport)?;
    if let Some(pw) = password.filter(|p| !p.is_empty()) {
        secrets::set(&secret_key(id, SecretKind::Password), &pw)?;
    }
    stop_worker(&app, id);
    let account = state.store().account(id)?;
    start_worker(&app, &account);
    Ok(())
}

#[tauri::command]
pub fn remove_account(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<()> {
    stop_worker(&app, id);
    state.store().remove_account(id)?;
    secrets::delete(&secret_key(id, SecretKind::Password))?;
    secrets::delete(&secret_key(id, SecretKind::RefreshToken))?;
    update_badge(&app);
    Ok(())
}

/// A new password after „Anmeldung erforderlich“.
#[tauri::command]
pub fn sign_in(app: AppHandle, state: State<'_, AppState>, id: i64, password: String) -> Res<()> {
    secrets::set(&secret_key(id, SecretKind::Password), &password)?;
    match state.worker(id) {
        Some(w) => w.send(Command::SignedIn(password)),
        None => {
            let account = state.store().account(id)?;
            start_worker(&app, &account);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn sync_now(state: State<'_, AppState>, account_id: Option<i64>) -> Res<()> {
    match account_id {
        Some(id) => {
            if let Some(w) = state.worker(id) {
                w.send(Command::SyncNow);
            }
        }
        None => state.all_workers().iter().for_each(|w| w.send(Command::SyncNow)),
    }
    Ok(())
}

// ---------------------------------------------------------------------------- folders and lists

#[tauri::command]
pub fn list_mailboxes(state: State<'_, AppState>, account_id: i64) -> Res<Vec<Mailbox>> {
    state.store().mailboxes(account_id)
}

#[derive(Serialize)]
pub struct Page {
    total: u32,
    offset: u32,
    items: Vec<MessageSummary>,
}

/// A page of a folder's list (the list is virtual: the UI asks for the rows it shows).
#[tauri::command]
pub fn list_messages(state: State<'_, AppState>, mailbox_id: i64, offset: u32, limit: u32, threads: bool) -> Res<Page> {
    let store = state.store();
    let limit = limit.min(500);
    let items =
        if threads { store.threads(mailbox_id, limit, offset)? } else { store.messages(mailbox_id, limit, offset)? };
    Ok(Page { total: store.list_len(mailbox_id, threads)?, offset, items })
}

#[tauri::command]
pub fn list_thread(state: State<'_, AppState>, thread_id: i64) -> Res<Vec<MessageSummary>> {
    state.store().thread(thread_id)
}

#[derive(Serialize)]
pub struct MessageResponse {
    #[serde(flatten)]
    view: MessageView,
    /// Why the body could not be fetched (offline, gone from the server).
    load_error: Option<String>,
}

/// A message for the reading pane; its body is fetched first when it is not here yet.
#[tauri::command]
pub async fn get_message(state: State<'_, AppState>, id: i64) -> Res<MessageResponse> {
    let (has_body, account) = {
        let store = state.store();
        let d = store.message(id)?;
        (d.has_body, d.account_id)
    };
    let mut load_error = None;
    if !has_body {
        match state.worker(account) {
            Some(w) => {
                if let Err(e) = w.fetch_body(id).await {
                    load_error = Some(e.to_string());
                }
            }
            None => {
                load_error =
                    Some(mail_core::tr!("Das Konto ist nicht angemeldet.", "The account is not signed in.").to_string())
            }
        }
    }
    let store = state.store();
    let remote = mail_core::settings::load(&store)?.remote_content;
    Ok(MessageResponse { view: actions::message_view(&store, id, remote)?, load_error })
}

#[tauri::command]
pub fn allow_remote_content(state: State<'_, AppState>, id: i64) -> Res<()> {
    state.store().allow_remote(id)
}

#[tauri::command]
pub fn allow_sender(state: State<'_, AppState>, email: String) -> Res<()> {
    state.store().allow_sender(&email)
}

#[tauri::command]
pub fn forbid_sender(state: State<'_, AppState>, email: String) -> Res<()> {
    state.store().forbid_sender(&email)
}

#[tauri::command]
pub fn allowed_senders(state: State<'_, AppState>) -> Res<Vec<String>> {
    state.store().allowed_senders()
}

// ---------------------------------------------------------------------------- actions

#[tauri::command]
pub fn set_flags(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
    seen: Option<bool>,
    flagged: Option<bool>,
) -> Res<()> {
    let accounts = actions::set_flags(&state.store(), &ids, seen, flagged)?;
    state.kick(&accounts);
    update_badge(&app);
    Ok(())
}

fn after(app: &AppHandle, state: &AppState, undo: Undo) -> Res<Undo> {
    state.kick(&undo.accounts);
    update_badge(app);
    Ok(undo)
}

#[tauri::command]
pub fn move_messages(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>, mailbox_id: i64) -> Res<Undo> {
    let u = actions::move_messages(&state.store(), &ids, mailbox_id)?;
    after(&app, &state, u)
}

#[tauri::command]
pub fn archive_messages(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>) -> Res<Undo> {
    let u = actions::archive(&state.store(), &ids)?;
    after(&app, &state, u)
}

#[tauri::command]
pub fn delete_messages(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>) -> Res<Undo> {
    let u = actions::delete(&state.store(), &ids)?;
    after(&app, &state, u)
}

#[tauri::command]
pub fn mark_spam(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>) -> Res<Undo> {
    let u = actions::mark_spam(&state.store(), &ids)?;
    after(&app, &state, u)
}

/// Takes the last action back; `false` when the server has it already.
#[tauri::command]
pub fn undo(app: AppHandle, state: State<'_, AppState>, token: Undo) -> Res<bool> {
    let done = actions::undo(&state.store(), &token)?;
    state.kick(&token.accounts);
    update_badge(&app);
    Ok(done)
}

#[tauri::command]
pub fn create_folder(state: State<'_, AppState>, account_id: i64, parent: Option<i64>, name: String) -> Res<i64> {
    let id = actions::create_folder(&state.store(), account_id, parent, &name)?;
    state.kick(&[account_id]);
    Ok(id)
}

#[tauri::command]
pub fn rename_folder(state: State<'_, AppState>, mailbox_id: i64, name: String) -> Res<()> {
    let account = state.store().mailbox(mailbox_id)?.account_id;
    actions::rename_folder(&state.store(), mailbox_id, &name)?;
    state.kick(&[account]);
    Ok(())
}

#[tauri::command]
pub fn delete_folder(state: State<'_, AppState>, mailbox_id: i64) -> Res<()> {
    let account = state.store().mailbox(mailbox_id)?.account_id;
    actions::delete_folder(&state.store(), mailbox_id)?;
    state.kick(&[account]);
    Ok(())
}

// ---------------------------------------------------------------------------- search

#[tauri::command]
pub fn search(state: State<'_, AppState>, query: String, account_id: Option<i64>) -> Res<Vec<MessageSummary>> {
    mail_core::search::search(&state.store(), &query, account_id, 200)
}

/// „Auf dem Server suchen“: the folders of an account (or the ones given) on the server.
#[tauri::command]
pub async fn server_search(
    state: State<'_, AppState>,
    account_id: i64,
    query: String,
    mailbox_ids: Option<Vec<i64>>,
) -> Res<Vec<MessageSummary>> {
    let ids = match mailbox_ids {
        Some(ids) => ids,
        None => {
            state.store().mailboxes(account_id)?.into_iter().filter(|m| m.selectable).map(|m| m.id).take(30).collect()
        }
    };
    let w = state.worker(account_id).ok_or_else(|| {
        Error::Network(mail_core::tr!("Das Konto ist nicht verbunden.", "The account is not connected.").into())
    })?;
    let hits = w.server_search(ids, query).await?;
    state.store().summaries(&hits)
}

#[tauri::command]
pub fn suggest_contacts(state: State<'_, AppState>, prefix: String) -> Res<Vec<Contact>> {
    state.store().suggest_contacts(&prefix, 8)
}

// ---------------------------------------------------------------------------- compose

fn read_files(paths: &[String]) -> Res<Vec<FileAttachment>> {
    paths
        .iter()
        .map(|p| {
            let path = std::path::Path::new(p);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            let data = std::fs::read(path).map_err(|e| {
                Error::Invalid(mail_core::trf!("„{name}“ ist nicht lesbar: {e}", "“{name}” cannot be read: {e}"))
            })?;
            Ok(FileAttachment { mime_type: mime_for(&name).into(), name, data })
        })
        .collect()
}

/// The attachments of a forwarded message, fetched when they are not here.
async fn forwarded(state: &AppState, message_id: Option<i64>) -> Res<Vec<FileAttachment>> {
    let Some(id) = message_id else { return Ok(vec![]) };
    let list = actions::message_view(&state.store(), id, mail_core::settings::RemoteContent::Ask)?.files;
    let mut out = vec![];
    for a in list {
        let (a, data) = attachment_bytes(state, a.id).await?;
        let name = a.filename.unwrap_or_else(|| "attachment".into());
        out.push(FileAttachment { name, mime_type: a.mime_type, data });
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct Queued {
    outbox_id: i64,
    message_id: String,
    /// Seconds until it goes out (the undo window).
    delay: u32,
}

#[tauri::command]
pub async fn send_message(state: State<'_, AppState>, draft: Draft) -> Res<Queued> {
    let mut files = read_files(&draft.attachments)?;
    files.extend(forwarded(&state, draft.forward_from).await?);
    let store = state.store();
    let delay = mail_core::settings::load(&store)?.undo_send_secs.min(30);
    let (outbox_id, message_id) = actions::queue_send(&store, &draft, &files, delay)?;
    drop(store);
    state.kick(&[draft.account_id]);
    Ok(Queued { outbox_id, message_id, delay })
}

#[tauri::command]
pub fn cancel_send(state: State<'_, AppState>, outbox_id: i64) -> Res<bool> {
    actions::cancel_send(&state.store(), outbox_id)
}

/// Saves the draft in Drafts; returns its Message-ID for the next save.
#[tauri::command]
pub async fn save_draft(state: State<'_, AppState>, draft: Draft) -> Res<String> {
    let mut files = read_files(&draft.attachments)?;
    files.extend(forwarded(&state, draft.forward_from).await?);
    let mid = actions::queue_draft(&state.store(), &draft, &files)?;
    state.kick(&[draft.account_id]);
    Ok(mid)
}

#[tauri::command]
pub fn discard_draft(state: State<'_, AppState>, account_id: i64, message_id: String) -> Res<()> {
    actions::discard_draft(&state.store(), account_id, &message_id)?;
    state.kick(&[account_id]);
    Ok(())
}

#[derive(Serialize)]
pub struct FileInfo {
    path: String,
    name: String,
    size: u64,
    mime_type: String,
}

/// Name, size and type of files to attach (dropped on the compose sheet or picked).
#[tauri::command]
pub fn file_infos(paths: Vec<String>) -> Res<Vec<FileInfo>> {
    Ok(paths
        .into_iter()
        .filter_map(|p| {
            let meta = std::fs::metadata(&p).ok().filter(|m| m.is_file())?;
            let name = std::path::Path::new(&p).file_name()?.to_string_lossy().into_owned();
            Some(FileInfo { mime_type: mime_for(&name).into(), name, size: meta.len(), path: p })
        })
        .collect())
}

// ---------------------------------------------------------------------------- attachments

async fn attachment_bytes(state: &AppState, id: i64) -> Res<(mail_core::store::Attachment, Vec<u8>)> {
    let (a, account, raw) = {
        let store = state.store();
        let a = store.attachment(id)?;
        let account = store.message(a.message_id)?.account_id;
        let raw = store.raw_message(a.message_id)?;
        (a, account, raw)
    };
    if let Some(raw) = raw
        && let Some(data) = mail_core::parse::attachment_data(&raw, a.part_index)
    {
        return Ok((a, data));
    }
    let w = state.worker(account).ok_or_else(|| {
        Error::Network(mail_core::tr!("Das Konto ist nicht verbunden.", "The account is not connected.").into())
    })?;
    let data = w.fetch_attachment(id).await?;
    Ok((a, data))
}

fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    if n.trim().is_empty() || n.starts_with('.') { format!("attachment{n}") } else { n }
}

/// Opens an attachment in its program (a copy in the app's cache folder).
#[tauri::command]
pub async fn open_attachment(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<String> {
    use tauri_plugin_opener::OpenerExt;
    let (a, data) = attachment_bytes(&state, id).await?;
    let dir = state.data_dir.join("attachments").join(id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| Error::Invalid(e.to_string()))?;
    let path = dir.join(safe_name(a.filename.as_deref().unwrap_or("attachment")));
    std::fs::write(&path, &data).map_err(|e| Error::Invalid(e.to_string()))?;
    let p = path.to_string_lossy().into_owned();
    app.opener().open_path(&p, None::<&str>).map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(p)
}

/// Saves an attachment where the user chose (the save dialog of the UI).
#[tauri::command]
pub async fn save_attachment(state: State<'_, AppState>, id: i64, path: String) -> Res<()> {
    let (_, data) = attachment_bytes(&state, id).await?;
    std::fs::write(&path, &data).map_err(|e| Error::Invalid(e.to_string()))?;
    state.store().set_attachment_path(id, &path)
}

#[derive(Serialize)]
pub struct Preview {
    mime_type: String,
    data_b64: String,
}

/// A picture attachment for the preview in the app.
#[tauri::command]
pub async fn attachment_preview(state: State<'_, AppState>, id: i64) -> Res<Preview> {
    let (a, data) = attachment_bytes(&state, id).await?;
    if !a.mime_type.starts_with("image/") || a.mime_type.contains("svg") || data.len() > 25 * 1024 * 1024 {
        return Err(Error::Invalid(
            mail_core::tr!("Keine Vorschau für diesen Anhang.", "No preview for this attachment.").into(),
        ));
    }
    Ok(Preview { mime_type: a.mime_type, data_b64: base64::engine::general_purpose::STANDARD.encode(data) })
}

// ---------------------------------------------------------------------------- the queue

#[tauri::command]
pub fn outbox_entries(state: State<'_, AppState>) -> Res<Vec<outbox::Entry>> {
    outbox::entries(&state.store())
}

/// „Erneut versuchen“.
#[tauri::command]
pub fn retry_outbox(state: State<'_, AppState>, id: i64) -> Res<()> {
    let account = outbox::entries(&state.store())?.into_iter().find(|e| e.id == id).map(|e| e.account_id);
    outbox::retry(&state.store(), id)?;
    if let Some(a) = account {
        state.kick(&[a]);
    }
    Ok(())
}

/// Drops a step that will not work (the server refused it).
#[tauri::command]
pub fn discard_outbox(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<bool> {
    let store = state.store();
    let Some(op) = outbox::cancel(&store, id)? else { return Ok(false) };
    // A move or deletion that will not happen: the messages show again.
    if let outbox::Op::Move { ids, .. } | outbox::Op::Delete { ids, .. } = op {
        store.set_hidden(&ids, false)?;
    }
    drop(store);
    update_badge(&app);
    Ok(true)
}
