//! The commands the UI calls (`ui/src/lib/api.ts`). Each one is a thin call into `mail_core`
//! (the store, `actions`, the account's worker); errors reach the UI as their text.

use base64::Engine;
use mail_core::Error;
use mail_core::account::{
    Account, AccountDraft, Auth, Credential, OAuthProvider, Provider, SecretKind, Transport, secret_key,
};
use mail_core::actions::{self, MessageView, Undo};
use mail_core::autoconfig::{Discovered, Lookup};
use mail_core::compose::{Draft, FileAttachment, mime_for};
use mail_core::engine::{CheckResult, Command, Notice};
use mail_core::later;
use mail_core::outbox;
use mail_core::settings::Settings;
use mail_core::store::{Mailbox, MessageSummary};
use mail_core::triage::{self, Category, InboxQuery};
use mail_core::unsubscribe::Method;
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
    /// A fixed „now“ (Unix seconds) for the screenshot generator, from `ARCALO_MAIL_NOW`; the UI
    /// formats dates against it. Unset in normal use.
    clock: Option<i64>,
    /// This build has a Microsoft client id (`ARCALO_MAIL_MS_CLIENT_ID`, docs/microsoft.md).
    microsoft_sign_in: bool,
    /// A phone or tablet: the touch layout (swipes, pull to refresh, bottom actions).
    /// `ARCALO_MAIL_TOUCH=1` asks for it on a desktop (the e2e tests of the phone layout).
    touch: bool,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        schema: mail_core::store::SCHEMA_VERSION,
        locale: crate::system_locale(),
        secret_store: secrets::kind(),
        clock: std::env::var("ARCALO_MAIL_NOW").ok().and_then(|v| v.parse().ok()),
        microsoft_sign_in: mail_core::oauth::microsoft_client_id().is_some(),
        touch: cfg!(mobile) || std::env::var("ARCALO_MAIL_TOUCH").is_ok_and(|v| v == "1"),
    }
}

/// The UI tells whether it is visible. A phone keeps its connections only while the app is in
/// front and checks in the background otherwise (`mobile`); the desktop keeps them always.
#[tauri::command]
pub fn set_in_front(app: AppHandle, visible: bool) {
    #[cfg(mobile)]
    crate::mobile::set_in_front(&app, visible);
    #[cfg(desktop)]
    let _ = (app, visible);
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Res<Settings> {
    mail_core::settings::load(&state.store())
}

/// Applies at once; the UI keeps the previous value for „Rückgängig“. New network profiles
/// reconnect every account through them.
#[tauri::command]
pub fn set_settings(app: AppHandle, state: State<'_, AppState>, mut settings: Settings) -> Res<()> {
    settings.network.validate()?;
    settings.ai.normalize()?;
    settings.mark_screener(mail_core::store::now());
    let before = mail_core::settings::load(&state.store())?;
    mail_core::settings::save(&state.store(), &settings)?;
    apply_language(settings.language);
    #[cfg(desktop)]
    if before.language != settings.language {
        crate::tray::refresh(&app);
    }
    if before.network != settings.network {
        crate::apply_network(&settings.network);
        crate::reconnect_all(&app);
    }
    // Gravatar switched off: what it said is forgotten.
    if before.gravatar && !settings.gravatar {
        mail_core::avatars::forget(&state.store(), "gravatar:")?;
    }
    if before.prefetch != settings.prefetch || before.subject_threads != settings.subject_threads {
        state.all_workers().iter().for_each(|w| w.send(Command::SyncNow));
    }
    Ok(())
}

/// The password of a proxy profile, into the credential store.
#[tauri::command]
pub fn set_proxy_password(app: AppHandle, state: State<'_, AppState>, profile: String, password: String) -> Res<()> {
    let key = mail_core::network::proxy_password_key(&profile);
    if password.is_empty() {
        secrets::delete(&key)?
    } else {
        secrets::set(&key, &password)?
    }
    crate::apply_network(&mail_core::settings::load(&state.store())?.network);
    crate::reconnect_all(&app);
    Ok(())
}

/// The system's proxy as the app sees it (Settings → Netzwerk).
#[tauri::command]
pub fn system_proxy() -> mail_core::network::SystemProxy {
    mail_core::network::system_proxy()
}

/// Writes the diagnostics report (redacted, `mail_core::diag`) where the user chose.
#[tauri::command]
pub fn export_diagnostics(state: State<'_, AppState>, path: String) -> Res<()> {
    let info = serde_json::to_value(app_info()).unwrap_or_default();
    let text = mail_core::diag::report(&state.store(), &info)?;
    std::fs::write(&path, text).map_err(|e| Error::Invalid(e.to_string()))
}

// ---------------------------------------------------------------------------- accounts

/// Where setup looks for the servers. `ARCALO_MAIL_AUTOCONFIG` (the e2e tests and the screenshot
/// generator) points the lookup at the test server on this computer instead, and
/// `ARCALO_MAIL_JMAP_DISCOVERY` the JMAP lookup (none without it); the network policy still
/// refuses anything but HTTPS or loopback.
fn lookup() -> Lookup {
    match std::env::var("ARCALO_MAIL_AUTOCONFIG") {
        Ok(url) if !url.is_empty() => {
            let jmap: Vec<String> =
                std::env::var("ARCALO_MAIL_JMAP_DISCOVERY").ok().filter(|u| !u.is_empty()).into_iter().collect();
            Lookup { autoconfig: vec![url.clone()], ispdb: url, dns: false, jmap_lookup: !jmap.is_empty(), jmap }
        }
        _ => Lookup::default(),
    }
}

/// The servers of an address (presets, autoconfig, provider database, DNS, a guess).
#[tauri::command]
pub async fn discover(email: String) -> Res<Discovered> {
    mail_core::autoconfig::discover(&email, &lookup()).await
}

/// Signs in to both servers with the settings of the setup form.
#[tauri::command]
pub async fn check_account(transport: Transport, password: String) -> Res<CheckResult> {
    match &transport {
        Transport::Imap { imap: incoming, smtp, .. } | Transport::Pop3 { pop3: incoming, smtp, .. } => {
            mail_core::network::check_server(incoming)?;
            mail_core::network::check_server(smtp)?;
        }
        _ => {}
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
        let wants_password = matches!(
            draft.transport,
            Transport::Imap { auth: Auth::Password, .. }
                | Transport::Pop3 { .. }
                | Transport::Jmap { auth: Auth::Password | Auth::Token, .. }
        );
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
    // A phone asks whether it may tell about new mail once there is an account.
    #[cfg(mobile)]
    crate::mobile::ask_for_notifications(&app);
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

// ---------------------------------------------------------------------------- sign-in with Microsoft

/// The result of signing in with Microsoft in the browser: the account's address and the check
/// of its servers, or why it did not work.
#[derive(Serialize)]
pub struct MicrosoftSignIn {
    /// Hands the tokens to `add_microsoft_account` (kept in memory only).
    session: Option<String>,
    email: Option<String>,
    check: Option<CheckResult>,
    problem: Option<mail_core::network::Problem>,
    error: Option<String>,
}

fn sign_in_failed(e: Error) -> MicrosoftSignIn {
    let (problem, error) = match e {
        Error::Connection(p) => (Some(p), None),
        other => (None, Some(other.to_string())),
    };
    MicrosoftSignIn { session: None, email: None, check: None, problem, error }
}

/// The servers of a Microsoft account (the e2e tests' on this computer, `oauth::test_servers`).
fn microsoft_transport(email: &str) -> Res<Transport> {
    let mut transport = mail_core::account::preset(Provider::Microsoft, email)?;
    if let (Some((test_imap, test_smtp)), Transport::Imap { imap, smtp, .. }) =
        (mail_core::oauth::test_servers(), &mut transport)
    {
        (*imap, *smtp) = (test_imap, test_smtp);
    }
    Ok(transport)
}

/// Opens Microsoft's sign-in in the browser and waits for it: on the desktop with the loopback
/// redirect, on a phone in the platform's browser session with the app's own redirect.
async fn browser_sign_in(app: &AppHandle, hint: &str) -> Res<mail_core::oauth::Tokens> {
    let cfg = mail_core::oauth::config_for(OAuthProvider::Microsoft)?;
    #[cfg(mobile)]
    {
        let _ = app;
        mail_core::oauth::sign_in_with_session(
            &cfg,
            hint,
            mail_core::oauth::MOBILE_REDIRECT,
            crate::mobile::auth_session,
        )
        .await
    }
    #[cfg(desktop)]
    {
        use tauri_plugin_opener::OpenerExt;
        mail_core::oauth::sign_in(&cfg, hint, |url| {
            app.opener().open_url(url, None::<&str>).map_err(|e| Error::Invalid(e.to_string()))
        })
        .await
    }
}

/// „Mit Microsoft anmelden“: the browser sign-in, then IMAP and SMTP checked with the token.
#[tauri::command]
pub async fn microsoft_sign_in(app: AppHandle, state: State<'_, AppState>, email: String) -> Res<MicrosoftSignIn> {
    let tokens = match browser_sign_in(&app, &email).await {
        Ok(t) => t,
        Err(e) => return Ok(sign_in_failed(e)),
    };
    let address = tokens.email.clone().filter(|e| e.contains('@')).unwrap_or_else(|| email.trim().to_string());
    let transport = microsoft_transport(&address)?;
    let check =
        mail_core::engine::check_account_with(&transport, &Credential::XOAuth2(tokens.access_token.clone())).await;
    let session = format!(
        "ms-{}-{}",
        mail_core::store::now(),
        state.pending_oauth.lock().unwrap_or_else(|e| e.into_inner()).len()
    );
    state.pending_oauth.lock().unwrap_or_else(|e| e.into_inner()).insert(session.clone(), tokens);
    Ok(MicrosoftSignIn { session: Some(session), email: Some(address), check: Some(check), problem: None, error: None })
}

/// Stores the account of a finished Microsoft sign-in with its refresh token.
#[tauri::command]
pub fn add_microsoft_account(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    name: String,
) -> Res<Account> {
    let tokens = state.pending_oauth.lock().unwrap_or_else(|e| e.into_inner()).remove(&session).ok_or_else(|| {
        Error::Invalid(mail_core::tr!("Bitte melde dich noch einmal an.", "Please sign in again.").into())
    })?;
    let email = tokens.email.clone().unwrap_or_default();
    let refresh = tokens.refresh_token.clone().ok_or_else(|| {
        Error::Invalid(
            mail_core::tr!("Microsoft hat keinen dauerhaften Zugang erteilt.", "Microsoft granted no lasting access.")
                .into(),
        )
    })?;
    let draft = AccountDraft {
        provider: Provider::Microsoft,
        name: String::new(),
        email: email.clone(),
        sender_name: name.trim().to_string(),
        transport: microsoft_transport(&email)?,
    };
    let account = {
        let store = state.store();
        let id = store.add_account(&draft)?;
        if let Err(e) = secrets::set(&secret_key(id, SecretKind::RefreshToken), &refresh) {
            store.remove_account(id)?;
            return Err(e);
        }
        store.account(id)?
    };
    start_worker(&app, &account);
    // A phone asks whether it may tell about new mail once there is an account.
    #[cfg(mobile)]
    crate::mobile::ask_for_notifications(&app);
    Ok(account)
}

/// „Anmelden“ for a Microsoft account whose access ended (revoked, password changed).
#[tauri::command]
pub async fn microsoft_reauth(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<MicrosoftSignIn> {
    let email = state.store().account(id)?.email;
    let tokens = match browser_sign_in(&app, &email).await {
        Ok(t) => t,
        Err(e) => return Ok(sign_in_failed(e)),
    };
    let Some(refresh) = tokens.refresh_token.clone() else {
        return Ok(sign_in_failed(Error::Invalid("no refresh token".into())));
    };
    secrets::set(&secret_key(id, SecretKind::RefreshToken), &refresh)?;
    match state.worker(id) {
        Some(w) => w.send(Command::SignedIn(refresh)),
        None => {
            let account = state.store().account(id)?;
            start_worker(&app, &account);
        }
    }
    Ok(MicrosoftSignIn { session: None, email: tokens.email, check: None, problem: None, error: None })
}

/// Signs in to a JMAP server (password with Basic, or an API token with Bearer) and counts
/// the folders, as `check_account` does for IMAP.
#[tauri::command]
pub async fn check_jmap(session_url: String, username: String, password: String, token: bool) -> Res<CheckResult> {
    let auth = if token {
        mail_core::jmap::JmapAuth::Bearer(password)
    } else {
        mail_core::jmap::JmapAuth::Basic { user: username, password }
    };
    Ok(match mail_core::jmap::check(&session_url, &auth).await {
        Ok(folders) => CheckResult { imap: None, smtp: None, error: None, folders },
        Err(Error::Connection(p)) => CheckResult { imap: Some(p), smtp: None, error: None, folders: 0 },
        Err(Error::Network(t)) => CheckResult {
            imap: Some(mail_core::network::Problem::Protocol { detail: t }),
            smtp: None,
            error: None,
            folders: 0,
        },
        Err(e) => CheckResult { imap: None, smtp: None, error: Some(e.to_string()), folders: 0 },
    })
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
    let mut items =
        if threads { store.threads(mailbox_id, limit, offset)? } else { store.messages(mailbox_id, limit, offset)? };
    crate::tools::annotate(&store, &mut items);
    Ok(Page { total: store.list_len(mailbox_id, threads)?, offset, items })
}

#[tauri::command]
pub fn list_thread(state: State<'_, AppState>, thread_id: i64) -> Res<Vec<MessageSummary>> {
    let store = state.store();
    let mut items = store.thread(thread_id)?;
    crate::tools::annotate(&store, &mut items);
    Ok(items)
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
    let store = state.store();
    let mut items = mail_core::search::search(&store, &query, account_id, 200)?;
    crate::tools::annotate(&store, &mut items);
    Ok(items)
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
async fn forwarded(state: &AppState, ids: &[i64]) -> Res<Vec<FileAttachment>> {
    let mut out = vec![];
    for &id in ids {
        let (a, data) = attachment_bytes(state, id).await?;
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

/// Sends a message (after the undo window); `followup_secs`: „Erinnern, wenn keine Antwort“
/// that long after it went out.
#[tauri::command]
pub async fn send_message(state: State<'_, AppState>, draft: Draft, followup_secs: Option<i64>) -> Res<Queued> {
    let mut files = read_files(&draft.attachments)?;
    files.extend(forwarded(&state, &draft.forwarded).await?);
    let store = state.store();
    let delay = mail_core::settings::load(&store)?.undo_send_secs.min(30);
    let (outbox_id, message_id) = actions::queue_send(&store, &draft, &files, delay)?;
    if let Some(secs) = followup_secs.filter(|s| *s > 0) {
        let due = mail_core::store::now() + delay as i64 + secs;
        later::add_followup(&store, draft.account_id, &message_id, due, draft.subject.trim())?;
    }
    drop(store);
    state.kick(&[draft.account_id]);
    Ok(Queued { outbox_id, message_id, delay })
}

#[tauri::command]
pub fn cancel_send(state: State<'_, AppState>, outbox_id: i64, message_id: Option<String>) -> Res<bool> {
    let store = state.store();
    let ok = actions::cancel_send(&store, outbox_id)?;
    // A reminder asked for with the message goes with it.
    if ok && let Some(mid) = message_id {
        later::forget_followup(&store, &mid)?;
    }
    Ok(ok)
}

/// „Später senden“: the message waits here until `at` (Unix seconds).
#[tauri::command]
pub async fn send_later(state: State<'_, AppState>, draft: Draft, at: i64, followup_secs: Option<i64>) -> Res<i64> {
    let mut files = read_files(&draft.attachments)?;
    files.extend(forwarded(&state, &draft.forwarded).await?);
    let (id, _) = later::schedule_send(&state.store(), &draft, &files, at, followup_secs.filter(|s| *s > 0))?;
    Ok(id)
}

#[tauri::command]
pub fn scheduled_messages(state: State<'_, AppState>) -> Res<Vec<later::Scheduled>> {
    later::scheduled(&state.store())
}

/// Takes a scheduled message back; returns its draft for the compose sheet.
#[tauri::command]
pub fn unschedule(state: State<'_, AppState>, id: i64) -> Res<Draft> {
    later::unschedule(&state.store(), id)
}

#[tauri::command]
pub fn send_scheduled_now(state: State<'_, AppState>, id: i64) -> Res<()> {
    let account = later::send_now(&state.store(), id)?;
    state.kick(&[account]);
    Ok(())
}

/// Saves the draft in Drafts; returns its Message-ID for the next save.
#[tauri::command]
pub async fn save_draft(state: State<'_, AppState>, draft: Draft) -> Res<String> {
    let mut files = read_files(&draft.attachments)?;
    files.extend(forwarded(&state, &draft.forwarded).await?);
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

/// What „Öffnen“ did: opened, or held back because of the file's type (`risk`).
#[derive(Serialize)]
pub struct OpenedFile {
    opened: bool,
    risk: mail_core::files::Risk,
}

/// Opens an attachment in its program (a copy in the app's cache folder, marked as coming from
/// the internet). Programs, scripts and shortcuts are never opened, only saved; an Office file
/// with macros opens only once the user `confirmed` the warning.
#[tauri::command]
pub async fn open_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    confirmed: Option<bool>,
) -> Res<OpenedFile> {
    use mail_core::files::{self, Risk};
    use tauri_plugin_opener::OpenerExt;
    let a = state.store().attachment(id)?;
    let name = files::safe_name(a.filename.as_deref().unwrap_or("attachment"));
    let risk = files::risk(&name);
    if risk == Risk::Blocked || (risk == Risk::Macro && confirmed != Some(true)) {
        return Ok(OpenedFile { opened: false, risk });
    }
    let (_, data) = attachment_bytes(&state, id).await?;
    let dir = state.data_dir.join("attachments").join(id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| Error::Invalid(e.to_string()))?;
    let path = dir.join(name);
    files::write_marked(&path, &data).map_err(|e| Error::Invalid(e.to_string()))?;
    let p = path.to_string_lossy().into_owned();
    app.opener().open_path(&p, None::<&str>).map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(OpenedFile { opened: true, risk })
}

/// Saves an attachment where the user chose (the save dialog of the UI), marked as coming from
/// the internet.
#[tauri::command]
pub async fn save_attachment(state: State<'_, AppState>, id: i64, path: String) -> Res<()> {
    let (_, data) = attachment_bytes(&state, id).await?;
    mail_core::files::write_marked(std::path::Path::new(&path), &data).map_err(|e| Error::Invalid(e.to_string()))?;
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
    let text = a.mime_type.starts_with("text/plain") || a.mime_type == "text/csv";
    if (!a.mime_type.starts_with("image/") && !text) || a.mime_type.contains("svg") || data.len() > 25 * 1024 * 1024 {
        return Err(Error::Invalid(
            mail_core::tr!("Keine Vorschau für diesen Anhang.", "No preview for this attachment.").into(),
        ));
    }
    // Text: the start is enough for a look.
    let data = if text { data.into_iter().take(64 * 1024).collect() } else { data };
    Ok(Preview { mime_type: a.mime_type, data_b64: base64::engine::general_purpose::STANDARD.encode(data) })
}

// ---------------------------------------------------------------------------- the sorted inbox

/// A page of the sorted inbox (one account, or all of them).
#[tauri::command]
pub fn inbox_list(state: State<'_, AppState>, query: InboxQuery, offset: u32, limit: u32) -> Res<Page> {
    let store = state.store();
    let (total, mut items) = triage::inbox(&store, &query, limit.min(500), offset)?;
    crate::tools::annotate(&store, &mut items);
    Ok(Page { total, offset, items })
}

#[tauri::command]
pub fn inbox_counts(state: State<'_, AppState>, account_id: Option<i64>) -> Res<triage::InboxCounts> {
    triage::counts(&state.store(), account_id)
}

/// A bundle of the newsletters tab: the sender's messages, newest first.
#[tauri::command]
pub fn bundle_messages(
    state: State<'_, AppState>,
    account_id: Option<i64>,
    sender: String,
) -> Res<Vec<MessageSummary>> {
    triage::bundle(&state.store(), account_id, &sender, 30)
}

/// „Absender verschieben“: every message of the sender shows in this tab (`None`: as sorted).
#[tauri::command]
pub fn set_sender_category(state: State<'_, AppState>, email: String, category: Option<Category>) -> Res<()> {
    triage::set_sender_category(&state.store(), &email, category)
}

#[tauri::command]
pub fn screener_senders(state: State<'_, AppState>, account_id: Option<i64>) -> Res<Vec<triage::WaitingSender>> {
    triage::waiting_senders(&state.store(), account_id)
}

#[tauri::command]
pub fn screener_allow(state: State<'_, AppState>, email: String) -> Res<()> {
    triage::allow_sender(&state.store(), &email)
}

/// „Abweisen“: the sender's mail moves to the „Screened Out“ folder, now and later.
#[tauri::command]
pub fn screener_block(app: AppHandle, state: State<'_, AppState>, email: String) -> Res<Undo> {
    let u = triage::block_sender(&state.store(), &email)?;
    after(&app, &state, u)
}

/// Takes the Screener's answer for a sender back.
#[tauri::command]
pub fn screener_forget(state: State<'_, AppState>, email: String) -> Res<()> {
    triage::forget_sender(&state.store(), &email)
}

#[tauri::command]
pub fn blocked_senders(state: State<'_, AppState>) -> Res<Vec<String>> {
    triage::blocked_senders(&state.store())
}

#[tauri::command]
pub fn subscriptions(state: State<'_, AppState>) -> Res<Vec<triage::Subscription>> {
    triage::subscriptions(&state.store())
}

/// What „Abbestellen“ did: unsubscribed with one click, sent the e-mail the list asks for, or
/// found only a page the user opens after confirming it.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Unsubscribed {
    Done,
    Sent { to: String },
    Link { url: String },
}

/// „Abbestellen“ for a sender: one click (RFC 8058) through `mail_core::network`, else an
/// e-mail through the account, else the page to confirm.
#[tauri::command]
pub async fn unsubscribe(state: State<'_, AppState>, sender: String) -> Res<Unsubscribed> {
    let (account, header, one_click) = mail_core::unsubscribe::latest(&state.store(), &sender)?;
    let method = mail_core::unsubscribe::best(&header, one_click).ok_or_else(|| {
        Error::Invalid(
            mail_core::tr!(
                "Für diesen Absender gibt es keine Abmeldeadresse.",
                "This sender gives no way to unsubscribe."
            )
            .into(),
        )
    })?;
    match method {
        Method::OneClick { url } => {
            mail_core::unsubscribe::post_one_click(&url).await?;
            mail_core::unsubscribe::record(&state.store(), &sender, "one_click")?;
            Ok(Unsubscribed::Done)
        }
        Method::Mailto { to, subject, body } => {
            let draft = mail_core::unsubscribe::mailto_draft(account, &to, &subject, &body);
            {
                let store = state.store();
                actions::queue_send(&store, &draft, &[], 0)?;
                mail_core::unsubscribe::record(&store, &sender, "mailto")?;
            }
            state.kick(&[account]);
            Ok(Unsubscribed::Sent { to })
        }
        Method::Link { url } => Ok(Unsubscribed::Link { url }),
    }
}

/// The user opened the unsubscribe page of a sender (after confirming it).
#[tauri::command]
pub fn unsubscribed_by_link(state: State<'_, AppState>, sender: String) -> Res<()> {
    mail_core::unsubscribe::record(&state.store(), &sender, "link")
}

// ---------------------------------------------------------------------------- time tools

#[tauri::command]
pub fn set_pinned(state: State<'_, AppState>, ids: Vec<i64>, on: bool) -> Res<()> {
    later::set_pinned(&state.store(), &ids, on)
}

#[tauri::command]
pub fn set_read_later(state: State<'_, AppState>, ids: Vec<i64>, on: bool) -> Res<()> {
    later::set_later(&state.store(), &ids, on)
}

#[tauri::command]
pub fn read_later(state: State<'_, AppState>) -> Res<Vec<MessageSummary>> {
    later::later_list(&state.store())
}

#[tauri::command]
pub fn snooze(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>, until: i64) -> Res<()> {
    let accounts = later::snooze(&state.store(), &ids, until)?;
    state.kick(&accounts);
    update_badge(&app);
    Ok(())
}

#[tauri::command]
pub fn unsnooze(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>) -> Res<()> {
    let accounts = later::unsnooze(&state.store(), &ids)?;
    state.kick(&accounts);
    update_badge(&app);
    Ok(())
}

#[tauri::command]
pub fn snoozed(state: State<'_, AppState>) -> Res<Vec<MessageSummary>> {
    later::snoozed(&state.store())
}

/// „Erinnern, wenn keine Antwort“ on messages, due at `due` (Unix seconds).
#[tauri::command]
pub fn set_followup(state: State<'_, AppState>, ids: Vec<i64>, due: i64) -> Res<()> {
    later::followup_messages(&state.store(), &ids, due)
}

#[tauri::command]
pub fn dismiss_followups(state: State<'_, AppState>, ids: Vec<i64>) -> Res<()> {
    later::dismiss_followups(&state.store(), &ids)
}

#[tauri::command]
pub fn followups(state: State<'_, AppState>) -> Res<Vec<MessageSummary>> {
    later::followups_due(&state.store())
}

#[tauri::command]
pub fn time_counts(state: State<'_, AppState>) -> Res<later::Counts> {
    later::counts(&state.store())
}

/// Lets the clock look at the time tools now (after the UI changed something due soon).
#[tauri::command]
pub fn tick(app: AppHandle) {
    crate::tick_now(&app);
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

// ---------------------------------------------------------------------------- opened from outside

/// A message to open that a notification asked for before the UI was ready (a phone's
/// notification starts the app): `(id, reply)`.
static PENDING_OPEN: std::sync::Mutex<Option<(i64, bool)>> = std::sync::Mutex::new(None);

/// A notification asked to open a message (to read or to reply): the running UI hears it at
/// once (`open-message`), a UI still starting takes it with [`take_pending_open`].
pub fn open_message(app: Option<&AppHandle>, id: i64, reply: bool) {
    *PENDING_OPEN.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, reply));
    if let Some(app) = app {
        use tauri::Emitter;
        let _ = app.emit("open-message", serde_json::json!({ "id": id, "reply": reply }));
    }
}

#[derive(Serialize)]
pub struct PendingOpen {
    id: i64,
    reply: bool,
}

/// The message a notification asked to open, once.
#[tauri::command]
pub fn take_pending_open() -> Option<PendingOpen> {
    PENDING_OPEN.lock().unwrap_or_else(|e| e.into_inner()).take().map(|(id, reply)| PendingOpen { id, reply })
}
