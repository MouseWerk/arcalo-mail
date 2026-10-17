//! The commands of the power tools: rules (local, Sieve, Microsoft), templates and snippets,
//! contacts with their address books and pictures, and the attachment hub. The logic is in the
//! core; this module fetches secrets, talks to the servers through the core's clients and
//! keeps the store free while it waits.

use mail_core::Error;
use mail_core::account::{Account, Auth, OAuthProvider, SecretKind, Server, Transport, secret_key};
use mail_core::contacts::{self, AddressBook, ContactCard};
use mail_core::hub;
use mail_core::imap::Role;
use mail_core::rules::{self, Place, Report, Rule, ServerRules};
use mail_core::store::{MessageSummary, Store, now};
use mail_core::templates::{self, Template, Vars};
use mail_core::{carddav, graph, sieve, tr, trf};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, secrets};

type Res<T> = Result<T, Error>;

// ---------------------------------------------------------------------------- Microsoft Graph

/// A Graph access token of a Microsoft account (its refresh token, Graph's scopes).
pub(crate) async fn graph_token(account_id: i64) -> Res<String> {
    let key = secret_key(account_id, SecretKind::RefreshToken);
    let refresh = secrets::get(&key)?.ok_or_else(|| {
        Error::Invalid(tr!("Das Konto ist nicht angemeldet.", "The account is not signed in.").into())
    })?;
    let cfg = mail_core::oauth::graph_config(&mail_core::oauth::config_for(OAuthProvider::Microsoft)?);
    let save: mail_core::oauth::OnRefreshToken = std::sync::Arc::new(move |t: &str| {
        if let Err(e) = secrets::set(&key, t) {
            mail_core::diag::log("secrets", &format!("refresh token not stored: {e}"));
        }
    });
    mail_core::oauth::TokenSource::new(cfg, refresh, Some(save)).access_token(false).await.map_err(|e| match e {
        Error::Connection(mail_core::network::Problem::AdminConsent { .. }) => Error::Invalid(
            tr!(
                "Microsoft hat den Zugriff auf Regeln und Kontakte noch nicht erlaubt. Melde das Konto in den Einstellungen neu an.",
                "Microsoft has not allowed access to rules and contacts yet. Sign the account in again in Settings."
            )
            .into(),
        ),
        other => other,
    })
}

fn is_microsoft(a: &Account) -> bool {
    matches!(a.transport, Transport::Imap { auth: Auth::OAuth2(OAuthProvider::Microsoft), .. })
}

fn password_of(account_id: i64) -> Res<String> {
    secrets::get(&secret_key(account_id, SecretKind::Password))?.ok_or_else(|| {
        Error::Invalid(tr!("Das Passwort des Kontos fehlt.", "The account's password is missing.").into())
    })
}

// ---------------------------------------------------------------------------- rules

#[tauri::command]
pub fn rules_list(state: State<'_, AppState>) -> Res<Vec<Rule>> {
    rules::list(&state.store())
}

/// What the server can do with an account's rules.
#[derive(Serialize)]
pub struct ServerInfo {
    /// `sieve`, `graph` or `none`.
    kind: String,
    server: Option<Server>,
    error: Option<String>,
    checked: bool,
}

/// Where an account's server rules would go, checked on the server (ManageSieve: signs in).
#[tauri::command]
pub async fn rules_server(state: State<'_, AppState>, account_id: i64, server: Option<Server>) -> Res<ServerInfo> {
    let (account, known) = {
        let store = state.store();
        (store.account(account_id)?, rules::server_rules(&store, account_id)?)
    };
    if is_microsoft(&account) {
        return Ok(ServerInfo { kind: "graph".into(), server: None, error: None, checked: true });
    }
    let Some(default) = sieve::default_server(&account.transport) else {
        return Ok(ServerInfo { kind: "none".into(), server: None, error: None, checked: true });
    };
    if !matches!(account.transport, Transport::Imap { auth: Auth::Password, .. }) {
        return Ok(ServerInfo { kind: "none".into(), server: None, error: None, checked: true });
    }
    let server = server.or(known.as_ref().and_then(|k| k.server.clone())).unwrap_or(default);
    mail_core::network::check_server(&server)?;
    let user = match &account.transport {
        Transport::Imap { username, .. } => username.clone(),
        _ => account.email.clone(),
    };
    let result = sieve::probe(&server, &user, &password_of(account_id)?).await;
    let (kind, error) = match result {
        Ok(_) => ("sieve", None),
        Err(e) => ("none", Some(e.to_string())),
    };
    rules::set_server_rules(
        &state.store(),
        &ServerRules {
            account_id,
            kind: kind.into(),
            server: Some(server.clone()),
            script: known.and_then(|k| k.script),
            checked_at: now(),
            error: error.clone(),
        },
    )?;
    Ok(ServerInfo { kind: kind.into(), server: Some(server), error, checked: true })
}

/// Why a rule cannot run on its account's server (`None`: it can, or it is local).
#[tauri::command]
pub fn rule_unsupported(state: State<'_, AppState>, rule: Rule) -> Res<Option<String>> {
    if rule.place != Place::Server {
        return Ok(None);
    }
    let Some(account_id) = rule.account_id else { return Ok(None) };
    let account = state.store().account(account_id)?;
    Ok(if is_microsoft(&account) { graph::unsupported(&rule) } else { sieve::unsupported(&rule) })
}

/// Writes an account's server rules to its server; the error, if any.
async fn sync_server(state: &AppState, account_id: i64, removed: Option<String>) -> Option<String> {
    match push_server(state, account_id, removed).await {
        Ok(()) => None,
        Err(e) => {
            let text = e.to_string();
            let store = state.store();
            if let Ok(Some(mut s)) = rules::server_rules(&store, account_id) {
                s.error = Some(text.clone());
                let _ = rules::set_server_rules(&store, &s);
            }
            Some(text)
        }
    }
}

async fn push_server(state: &AppState, account_id: i64, removed: Option<String>) -> Res<()> {
    let (account, list, known, trash, archive) = {
        let store = state.store();
        let name = |role: Role, fallback: &str| -> Res<String> {
            Ok(store.mailbox_by_role(account_id, role)?.map(|m| m.name).unwrap_or_else(|| fallback.to_string()))
        };
        (
            store.account(account_id)?,
            rules::server_rules_of(&store, account_id)?,
            rules::server_rules(&store, account_id)?,
            name(Role::Trash, "Trash")?,
            name(Role::Archive, "Archive")?,
        )
    };
    if is_microsoft(&account) {
        let g = graph::Graph::new(&graph_token(account_id).await?, mail_core::network::Service::Graph)?;
        if let Some(r) = removed {
            g.delete_rule(&r).await?;
        }
        for (i, r) in list.iter().enumerate() {
            let id = g.put_rule(r, i as i64 + 1, r.server_ref.as_deref()).await?;
            if r.server_ref.as_deref() != Some(id.as_str()) {
                rules::set_server_ref(&state.store(), r.id, Some(&id))?;
            }
        }
        return Ok(());
    }
    let Transport::Imap { username, .. } = &account.transport else {
        return Err(Error::Invalid(tr!("Dieser Server kennt keine Regeln.", "This server has no rules.").into()));
    };
    let server = known
        .as_ref()
        .and_then(|k| k.server.clone())
        .or_else(|| sieve::default_server(&account.transport))
        .ok_or_else(|| Error::Invalid(tr!("Dieser Server kennt keine Regeln.", "This server has no rules.").into()))?;
    let (requires, block) = sieve::script(&list, &trash, &archive);
    let requires: Vec<&str> = requires.to_vec();
    let script = sieve::upload(&server, username, &password_of(account_id)?, &requires, &block).await?;
    rules::set_server_rules(
        &state.store(),
        &ServerRules {
            account_id,
            kind: "sieve".into(),
            server: Some(server),
            script: Some(script),
            checked_at: now(),
            error: None,
        },
    )
}

#[derive(Serialize)]
pub struct Saved {
    id: i64,
    /// The server did not take the rule (it is kept here).
    server_error: Option<String>,
}

#[tauri::command]
pub async fn rule_save(state: State<'_, AppState>, rule: Rule) -> Res<Saved> {
    let before = if rule.id > 0 { Some(rules::get(&state.store(), rule.id)?) } else { None };
    let id = rules::save(&state.store(), &rule)?;
    let mut server_error = None;
    // A rule that left the server, or one that went there.
    if let Some(b) =
        before.filter(|b| b.place == Place::Server && (rule.place != Place::Server || b.account_id != rule.account_id))
        && let Some(a) = b.account_id
    {
        rules::set_server_ref(&state.store(), id, None)?;
        server_error = sync_server(&state, a, b.server_ref.clone()).await;
    }
    if rule.place == Place::Server
        && let Some(a) = rule.account_id
    {
        server_error = server_error.or(sync_server(&state, a, None).await);
    }
    Ok(Saved { id, server_error })
}

#[tauri::command]
pub async fn rule_delete(state: State<'_, AppState>, id: i64) -> Res<Option<String>> {
    let rule = rules::get(&state.store(), id)?;
    rules::delete(&state.store(), id)?;
    if rule.place == Place::Server
        && let Some(a) = rule.account_id
    {
        return Ok(sync_server(&state, a, rule.server_ref).await);
    }
    Ok(None)
}

#[tauri::command]
pub async fn rule_enable(state: State<'_, AppState>, id: i64, enabled: bool) -> Res<Option<String>> {
    rules::set_enabled(&state.store(), id, enabled)?;
    let rule = rules::get(&state.store(), id)?;
    if rule.place == Place::Server
        && let Some(a) = rule.account_id
    {
        return Ok(sync_server(&state, a, None).await);
    }
    Ok(None)
}

#[tauri::command]
pub async fn rules_reorder(state: State<'_, AppState>, ids: Vec<i64>) -> Res<Option<String>> {
    rules::reorder(&state.store(), &ids)?;
    let accounts: Vec<i64> = {
        let mut a: Vec<i64> = rules::list(&state.store())?
            .into_iter()
            .filter(|r| r.place == Place::Server)
            .filter_map(|r| r.account_id)
            .collect();
        a.sort_unstable();
        a.dedup();
        a
    };
    let mut err = None;
    for a in accounts {
        err = err.or(sync_server(&state, a, None).await);
    }
    Ok(err)
}

#[derive(Serialize)]
pub struct Preview {
    total: u32,
    items: Vec<MessageSummary>,
}

/// The stored messages a rule would catch.
#[tauri::command]
pub fn rule_preview(state: State<'_, AppState>, rule: Rule) -> Res<Preview> {
    let (total, items) = rules::preview(&state.store(), &rule, 30)?;
    Ok(Preview { total, items })
}

/// „Jetzt anwenden“ on a folder.
#[tauri::command]
pub fn rule_run(app: AppHandle, state: State<'_, AppState>, rule: Rule, mailbox_id: i64) -> Res<Report> {
    let report = rules::run_on_folder(&state.store(), &rule, mailbox_id)?;
    state.kick(&report.accounts);
    for a in &report.accounts {
        let _ = app.emit("mail-notice", mail_core::engine::Notice::Changed { account_id: *a, mailboxes: vec![] });
    }
    crate::update_badge(&app);
    Ok(report)
}

// ---------------------------------------------------------------------------- templates

#[tauri::command]
pub fn templates_list(state: State<'_, AppState>) -> Res<Vec<Template>> {
    templates::list(&state.store())
}

#[tauri::command]
pub fn template_save(state: State<'_, AppState>, template: Template) -> Res<i64> {
    templates::save(&state.store(), &template)
}

#[tauri::command]
pub fn template_delete(state: State<'_, AppState>, id: i64) -> Res<()> {
    templates::delete(&state.store(), id)
}

/// Fills a template's or snippet's variables for the compose sheet: the first recipient
/// (`Name <a@b>` or an address the contacts know), the account's sender name and signature,
/// today in the user's words (`date`, `time` from the UI).
#[tauri::command]
pub fn template_render(
    state: State<'_, AppState>,
    texts: Vec<String>,
    account_id: Option<i64>,
    to: Vec<String>,
    subject: String,
    date: String,
    time: String,
) -> Res<Vec<String>> {
    let vars = template_vars(&state, account_id, &to, subject, date, time)?;
    Ok(texts.iter().map(|t| templates::render(t, &vars)).collect())
}

fn template_vars(
    state: &AppState,
    account_id: Option<i64>,
    to: &[String],
    subject: String,
    date: String,
    time: String,
) -> Res<Vars> {
    let store = state.store();
    let account = account_id.and_then(|a| store.account(a).ok());
    let name = to
        .first()
        .map(|t| match mail_core::compose::parse_address(t) {
            Ok(a) => a
                .name
                .filter(|n| !n.trim().is_empty())
                .or_else(|| contacts::card(&store, &a.email).ok().and_then(|c| c.name))
                .unwrap_or_default(),
            Err(_) => String::new(),
        })
        .unwrap_or_default();
    Ok(Vars {
        name,
        my_name: account
            .as_ref()
            .map(|a| if a.sender_name.trim().is_empty() { a.name.clone() } else { a.sender_name.clone() })
            .unwrap_or_default(),
        date,
        time,
        subject,
        signature: account.map(|a| a.signature).unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------- contacts

#[tauri::command]
pub fn contact_card(state: State<'_, AppState>, email: String) -> Res<ContactCard> {
    contacts::card(&state.store(), &email)
}

#[tauri::command]
pub fn address_books(state: State<'_, AppState>) -> Res<Vec<AddressBook>> {
    contacts::books(&state.store())
}

/// Syncs one address book (CardDAV or Microsoft's contacts).
pub async fn sync_book(state: &AppState, id: i64) -> Res<()> {
    let (book, known) = {
        let store = state.store();
        (contacts::book(&store, id)?, contacts::known(&store, id)?)
    };
    let result = async {
        if book.kind == "graph" {
            let account = book.account_id.ok_or_else(|| Error::Invalid("graph book without account".into()))?;
            let g = graph::Graph::new(&graph_token(account).await?, mail_core::network::Service::Contacts)?;
            let mut cards = vec![];
            for c in g.contacts().await? {
                let photo = g.contact_photo(&c.id).await.unwrap_or(None);
                cards.push(carddav::Card {
                    href: c.id,
                    etag: None,
                    name: c.name,
                    emails: c.emails,
                    phones: c.phones,
                    org: c.org,
                    photo,
                });
            }
            let hrefs: Vec<String> = cards.iter().map(|c| c.href.clone()).collect();
            let removed: Vec<String> = known.iter().map(|(h, _)| h.clone()).filter(|h| !hrefs.contains(h)).collect();
            contacts::apply(&state.store(), id, &cards, &removed)
        } else {
            let password = secrets::get(&contacts::secret_key(id))?.unwrap_or_default();
            let dav = carddav::Dav::new(&book.username, &password)?;
            let books = dav.discover(&book.url).await?;
            let changes = carddav::sync(&dav, &books, &known).await?;
            contacts::apply(&state.store(), id, &changes.changed, &changes.removed)
        }
    }
    .await;
    if let Err(e) = &result {
        contacts::failed(&state.store(), id, &e.to_string())?;
    }
    result
}

/// Adds a CardDAV address book (checked first) and syncs it.
#[tauri::command]
pub async fn carddav_add(
    state: State<'_, AppState>,
    name: String,
    url: String,
    username: String,
    password: String,
) -> Res<i64> {
    let url = url.trim().to_string();
    mail_core::network::check_https_url(&url)?;
    let dav = carddav::Dav::new(&username, &password)?;
    let found = dav.discover(&url).await?;
    let name = if name.trim().is_empty() { found.first().map(|b| b.name.clone()).unwrap_or_default() } else { name };
    let id = contacts::add_book(&state.store(), "carddav", &name, &url, &username, None)?;
    if let Err(e) = secrets::set(&contacts::secret_key(id), &password) {
        contacts::remove_book(&state.store(), id)?;
        return Err(e);
    }
    sync_book(&state, id).await?;
    Ok(id)
}

#[tauri::command]
pub async fn address_book_sync(state: State<'_, AppState>, id: i64) -> Res<()> {
    sync_book(&state, id).await
}

#[tauri::command]
pub fn address_book_remove(state: State<'_, AppState>, id: i64) -> Res<()> {
    contacts::remove_book(&state.store(), id)?;
    secrets::delete(&contacts::secret_key(id))
}

/// Switches the Microsoft contacts of an account on (and syncs them) or off.
#[tauri::command]
pub async fn microsoft_contacts(state: State<'_, AppState>, account_id: i64, on: bool) -> Res<()> {
    let existing = contacts::graph_book(&state.store(), account_id)?;
    match (on, existing) {
        (true, None) => {
            let name = {
                let store = state.store();
                let a = store.account(account_id)?;
                trf!("Kontakte von {}", "Contacts of {}", a.email)
            };
            let id = contacts::add_book(&state.store(), "graph", &name, "", "", Some(account_id))?;
            sync_book(&state, id).await
        }
        (true, Some(id)) => sync_book(&state, id).await,
        (false, Some(id)) => contacts::remove_book(&state.store(), id),
        (false, None) => Ok(()),
    }
}

/// Syncs every address book (at start, in the background).
pub fn sync_all_books(app: &AppHandle) {
    use tauri::Manager;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else { return };
        let ids: Vec<i64> =
            contacts::books(&state.store()).map(|b| b.into_iter().map(|b| b.id).collect()).unwrap_or_default();
        for id in ids {
            if let Err(e) = sync_book(&state, id).await {
                mail_core::diag::log("contacts", &format!("address book {id}: {e}"));
            }
        }
    });
}

/// Suggestions for the address fields: the address books and the people of the mail.
#[tauri::command]
pub fn suggest_contacts(state: State<'_, AppState>, prefix: String) -> Res<Vec<mail_core::store::Contact>> {
    contacts::suggest(&state.store(), &prefix, 8)
}

// ---------------------------------------------------------------------------- pictures

/// The picture of an address as a `data:` address, or `None` (initials). BIMI and Gravatar are
/// fetched once and kept; never anything from inside a mail.
#[tauri::command]
pub async fn picture(state: State<'_, AppState>, email: String) -> Res<Option<String>> {
    use base64::Engine;
    use mail_core::avatars::{self, Lookup, Source};
    let encode = |mime: &str, data: &[u8]| {
        format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(data))
    };
    let lookup = {
        let store = state.store();
        let s = mail_core::settings::load(&store)?;
        avatars::lookup(&store, &email, s.bimi, s.gravatar)?
    };
    let sources = match lookup {
        Lookup::Found(p) => return Ok(Some(encode(&p.mime, &p.data))),
        Lookup::None => return Ok(None),
        Lookup::Fetch(s) => s,
    };
    for src in sources {
        // One fetch per domain or address at a time.
        let key = src.key();
        if !state.pictures.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone()) {
            return Ok(None);
        }
        let found = match &src {
            Source::Bimi(domain) => avatars::fetch_bimi_dns(domain).await,
            Source::Gravatar(address) => avatars::fetch_gravatar(address).await,
        };
        state.pictures.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        match found {
            Ok(f) => {
                avatars::keep(&state.store(), &src, f.as_ref())?;
                if let Some((mime, data)) = f {
                    return Ok(Some(encode(&mime, &data)));
                }
            }
            // Offline or a broken server: asked again another time, nothing kept.
            Err(e) => mail_core::diag::log("pictures", &format!("{key}: {e}")),
        }
    }
    Ok(None)
}

/// Forgets the kept pictures (Gravatar switched off, or Settings → Datenschutz).
#[tauri::command]
pub fn forget_pictures(state: State<'_, AppState>, prefix: String) -> Res<()> {
    mail_core::avatars::forget(&state.store(), &prefix)
}

// ---------------------------------------------------------------------------- POP3

/// The POP3 settings of an address: a preset where the provider still offers POP3, else a guess.
#[tauri::command]
pub fn pop3_preset(email: String) -> Option<Transport> {
    mail_core::pop3::preset(&email)
}

// ---------------------------------------------------------------------------- the attachment hub

#[derive(Serialize)]
pub struct HubPage {
    total: u32,
    offset: u32,
    items: Vec<hub::Item>,
}

#[tauri::command]
pub fn hub_list(state: State<'_, AppState>, filter: hub::Filter, offset: u32, limit: u32) -> Res<HubPage> {
    let (total, items) = hub::list(&state.store(), &filter, limit.min(500), offset)?;
    Ok(HubPage { total, offset, items })
}

#[tauri::command]
pub fn hub_senders(state: State<'_, AppState>, account_id: Option<i64>) -> Res<Vec<hub::Sender>> {
    hub::senders(&state.store(), account_id)
}

/// Marks the rows of a list „Wichtig“ where they are (the lists of folders, the inbox, search).
pub fn annotate(store: &Store, rows: &mut [MessageSummary]) {
    if let Err(e) = mail_core::ai::priority::annotate(store, rows) {
        mail_core::diag::log("priority", &e.to_string());
    }
}
