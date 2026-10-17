//! The calendar's commands and its sync loop. The logic is in the core (`mail_core::calendar`,
//! `mail_core::itip`); this module fetches the secrets of each source, runs a round of every
//! source every five minutes and right after a change here (`kick`), tells the UI
//! (`calendar-changed`) and sends what the rounds ask for by mail: the answers to invitations and
//! the updates of meetings the user organizes, for servers that do not send them themselves.

use std::sync::Arc;
use std::time::Duration;

use mail_core::Error;
use mail_core::account::{Auth, OAuthProvider, SecretKind, Transport, secret_key};
use mail_core::calendar::{
    self, Calendar, CalendarEvent, CalendarPatch, Fields, Source, SourceKind, Undo, sync::Access,
};
use mail_core::itip::{self, Answer};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::{AppState, secrets};

type Res<T> = Result<T, Error>;

/// How often every calendar is read while the app runs.
const ROUND: Duration = Duration::from_secs(300);

/// Wakes the loop for a round now.
static WAKE: std::sync::OnceLock<Arc<Notify>> = std::sync::OnceLock::new();

fn wake() -> Arc<Notify> {
    WAKE.get_or_init(|| Arc::new(Notify::new())).clone()
}

/// A round soon (a change made here, new mail with an invitation).
pub fn kick() {
    wake().notify_one();
}

fn me(state: &AppState) -> Vec<String> {
    calendar::my_addresses(&state.store()).unwrap_or_default()
}

fn parse_time(s: &str) -> Res<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|_| Error::Invalid(mail_core::trf!("„{s}“ ist kein Zeitpunkt.", "“{s}” is no point in time.")))
}

// ---------------------------------------------------------------------------- reading

#[derive(Serialize)]
pub struct Overview {
    sources: Vec<Source>,
    calendars: Vec<Calendar>,
    /// Where new appointments go.
    default: i64,
    /// CalDAV servers people use (name, address; `{host}`: the user's own).
    presets: Vec<(&'static str, &'static str)>,
    /// Accounts whose calendars can be added (Microsoft over Graph, JMAP).
    accounts: Vec<AccountCalendars>,
}

#[derive(Serialize)]
pub struct AccountCalendars {
    account_id: i64,
    email: String,
    /// `graph` or `jmap`.
    kind: String,
    on: bool,
}

#[tauri::command]
pub fn calendar_overview(state: State<'_, AppState>) -> Res<Overview> {
    let store = state.store();
    let chosen = mail_core::settings::load(&store)?.calendar.default_calendar;
    let default = calendar::default_calendar(&store, chosen)?;
    let mut accounts = vec![];
    for a in store.accounts()? {
        let kind = match &a.transport {
            Transport::Imap { auth: Auth::OAuth2(OAuthProvider::Microsoft), .. } => "graph",
            Transport::Jmap { .. } => "jmap",
            _ => continue,
        };
        accounts.push(AccountCalendars {
            account_id: a.id,
            email: a.email.clone(),
            kind: kind.into(),
            on: calendar::account_source(&store, a.id)?.is_some(),
        });
    }
    Ok(Overview {
        sources: calendar::sources(&store)?,
        calendars: calendar::calendars(&store)?,
        default,
        presets: calendar::caldav::PRESETS.to_vec(),
        accounts,
    })
}

/// The appointments of the shown calendars between two instants (RFC 3339).
#[tauri::command]
pub fn calendar_events(state: State<'_, AppState>, from: String, to: String) -> Res<Vec<CalendarEvent>> {
    let me = me(&state);
    calendar::events(&state.store(), parse_time(&from)?, parse_time(&to)?, &me, false)
}

// ---------------------------------------------------------------------------- changes

#[derive(Serialize)]
pub struct Changed {
    event: Option<CalendarEvent>,
    undo: Undo,
}

/// What the UI sends for a change: RFC 3339 times.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FieldsIn {
    title: Option<String>,
    start: Option<String>,
    end: Option<String>,
    location: Option<String>,
    all_day: Option<bool>,
    description: Option<String>,
}

impl FieldsIn {
    fn fields(self) -> Res<Fields> {
        Ok(Fields {
            title: self.title,
            start: self.start.as_deref().map(parse_time).transpose()?,
            end: self.end.as_deref().map(parse_time).transpose()?,
            location: self.location,
            all_day: self.all_day,
            description: self.description,
        })
    }
}

#[tauri::command]
pub fn calendar_create(state: State<'_, AppState>, calendar_id: Option<i64>, fields: FieldsIn) -> Res<Changed> {
    let me = me(&state);
    let store = state.store();
    let target = match calendar_id {
        Some(c) => c,
        None => calendar::default_calendar(&store, mail_core::settings::load(&store)?.calendar.default_calendar)?,
    };
    let (event, undo) = calendar::create(&store, target, &fields.fields()?, &me)?;
    drop(store);
    kick();
    Ok(Changed { event: Some(event), undo })
}

/// Changes an appointment; `send`: the user confirmed that a meeting's attendees get the update.
#[tauri::command]
pub fn calendar_update(state: State<'_, AppState>, key: String, fields: FieldsIn, send: bool) -> Res<Changed> {
    let me = me(&state);
    let (event, undo) = calendar::update(&state.store(), &key, &fields.fields()?, send, &me)?;
    kick();
    Ok(Changed { event: Some(event), undo })
}

#[tauri::command]
pub fn calendar_delete(state: State<'_, AppState>, key: String) -> Res<Undo> {
    let me = me(&state);
    let u = calendar::delete(&state.store(), &key, &me)?;
    kick();
    Ok(u)
}

#[tauri::command]
pub fn calendar_undo(state: State<'_, AppState>, undo: Undo) -> Res<Option<CalendarEvent>> {
    let me = me(&state);
    let back = calendar::undo(&state.store(), &undo, &me)?;
    kick();
    Ok(back)
}

/// Settles a conflict: `keep_mine` writes the change made here over the server's version.
#[tauri::command]
pub fn calendar_resolve(state: State<'_, AppState>, key: String, keep_mine: bool) -> Res<()> {
    calendar::resolve(&state.store(), &key, keep_mine)?;
    kick();
    Ok(())
}

#[tauri::command]
pub fn calendar_discard(state: State<'_, AppState>, key: String) -> Res<()> {
    calendar::discard(&state.store(), &key)?;
    kick();
    Ok(())
}

#[tauri::command]
pub fn calendar_set(state: State<'_, AppState>, id: i64, patch: CalendarPatch) -> Res<Calendar> {
    calendar::set_calendar(&state.store(), id, &patch)
}

/// Every source now; returns the sources with their state.
#[tauri::command]
pub async fn calendar_sync(app: AppHandle, state: State<'_, AppState>) -> Res<Vec<Source>> {
    round(&app).await;
    calendar::sources(&state.store())
}

// ---------------------------------------------------------------------------- sources

/// Adds a CalDAV server (checked first: its calendars are found) and reads it.
#[tauri::command]
pub async fn caldav_add(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    url: String,
    username: String,
    password: String,
) -> Res<i64> {
    let url = url.trim().to_string();
    mail_core::network::check_https_url(&url)?;
    let dav = calendar::caldav::CalDav::new(&username, &password)?;
    let found = dav.discover(&url).await?;
    let name =
        if name.trim().is_empty() { found.calendars.first().map(|c| c.name.clone()).unwrap_or_default() } else { name };
    let id = calendar::add_source(&state.store(), SourceKind::Caldav, &name, &url, &username, None)?;
    if let Err(e) = secrets::set(&calendar::secret_key(id), &password) {
        calendar::remove_source(&state.store(), id)?;
        return Err(e);
    }
    sync_one(&app, id).await?;
    Ok(id)
}

/// Subscribes to an ICS address (read-only); the address goes to the credential store, the
/// settings show its scheme and host.
#[tauri::command]
pub async fn ics_subscribe(app: AppHandle, state: State<'_, AppState>, name: String, url: String) -> Res<i64> {
    let target = calendar::ics::fetch_url(&url)?;
    // Checked once before it is kept.
    calendar::ics::fetch(&target, None).await?;
    let shown = calendar::ics::display_url(&target);
    let name = if name.trim().is_empty() { mail_core::tr!("Abo", "Subscription").to_string() } else { name };
    let id = calendar::add_source(&state.store(), SourceKind::Ics, &name, &shown, "", None)?;
    if let Err(e) = secrets::set(&calendar::secret_key(id), &target) {
        calendar::remove_source(&state.store(), id)?;
        return Err(e);
    }
    sync_one(&app, id).await?;
    Ok(id)
}

/// Switches a Microsoft or JMAP account's calendars on (and reads them) or off.
#[tauri::command]
pub async fn calendar_account(app: AppHandle, state: State<'_, AppState>, account_id: i64, on: bool) -> Res<()> {
    let existing = calendar::account_source(&state.store(), account_id)?;
    match (on, existing) {
        (true, None) => {
            let (kind, name) = {
                let store = state.store();
                let a = store.account(account_id)?;
                let kind =
                    if matches!(a.transport, Transport::Jmap { .. }) { SourceKind::Jmap } else { SourceKind::Graph };
                (kind, a.email.clone())
            };
            let id = calendar::add_source(&state.store(), kind, &name, "", "", Some(account_id))?;
            if let Err(e) = sync_one(&app, id).await {
                calendar::remove_source(&state.store(), id)?;
                return Err(e);
            }
            Ok(())
        }
        (true, Some(id)) => sync_one(&app, id).await,
        (false, Some(id)) => calendar::remove_source(&state.store(), id),
        (false, None) => Ok(()),
    }
}

#[tauri::command]
pub fn calendar_remove_source(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<()> {
    calendar::remove_source(&state.store(), id)?;
    let _ = secrets::delete(&calendar::secret_key(id));
    let _ = app.emit("calendar-changed", ());
    Ok(())
}

// ---------------------------------------------------------------------------- rounds

/// How a round reaches a source.
async fn access(state: &AppState, s: &Source) -> Res<Access> {
    Ok(match s.kind {
        SourceKind::Local => Access::None,
        SourceKind::Caldav => Access::Basic {
            user: s.username.clone(),
            password: secrets::get(&calendar::secret_key(s.id))?.unwrap_or_default(),
        },
        SourceKind::Ics => Access::Url(secrets::get(&calendar::secret_key(s.id))?.ok_or_else(|| {
            Error::Invalid(
                mail_core::tr!("Die Adresse des Abos fehlt.", "The subscription's address is missing.").into(),
            )
        })?),
        SourceKind::Graph => {
            let account = s.account_id.ok_or_else(|| Error::Invalid("graph source without account".into()))?;
            let token = crate::tools::graph_token(account).await?;
            Access::Graph(mail_core::graph::Graph::new(&token, mail_core::network::Service::Graph)?)
        }
        SourceKind::Jmap => {
            let account = s.account_id.ok_or_else(|| Error::Invalid("jmap source without account".into()))?;
            let a = state.store().account(account)?;
            let Transport::Jmap { session_url, username, auth } = &a.transport else {
                return Err(Error::Invalid("not a JMAP account".into()));
            };
            let secret = secrets::get(&secret_key(account, SecretKind::Password))?.unwrap_or_default();
            let client = mail_core::jmap::JmapClient::connect(
                session_url,
                &mail_core::jmap::JmapAuth::of(auth, username, &secret),
            )
            .await?;
            if calendar::jmap::account_of(&client).is_none() {
                return Err(Error::Invalid(
                    mail_core::tr!(
                        "Dieser Server bietet keine Kalender über JMAP an.",
                        "This server offers no calendars over JMAP."
                    )
                    .into(),
                ));
            }
            Access::Jmap(Box::new(client))
        }
    })
}

/// One round of one source, with its outcome sent on.
async fn sync_one(app: &AppHandle, id: i64) -> Res<()> {
    let Some(state) = app.try_state::<AppState>() else { return Ok(()) };
    let source = calendar::source(&state.store(), id)?;
    let access = match access(&state, &source).await {
        Ok(a) => a,
        Err(e) => {
            calendar::synced(&state.store(), id, Some(&calendar::describe_error(&e)), None)?;
            let _ = app.emit("calendar-changed", ());
            return Err(e);
        }
    };
    let result = calendar::sync::sync_source(&state.store, id, &access).await;
    let _ = app.emit("calendar-changed", ());
    let outcome = result?;
    // Meetings whose update the server does not send: the organizer's account sends it.
    for data in outcome.notify {
        let store = state.store();
        let accounts = store.accounts()?;
        let organizer = itip::parse(&data, &[]).and_then(|i| calendar::attendee_email(&i.organizer));
        let Some(account) = accounts.iter().find(|a| Some(a.email.to_ascii_lowercase()) == organizer) else { continue };
        if let Some((raw, to)) = itip::update_mail(account, &data)? {
            mail_core::actions::queue_raw(&store, account.id, &raw, &to)?;
            drop(store);
            state.kick(&[account.id]);
        }
    }
    Ok(())
}

/// Every source once; invitations in new mail applied first.
pub async fn round(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let (ids, scanned) = {
        let store = state.store();
        let scanned = itip::scan(&store).unwrap_or_else(|e| {
            mail_core::diag::log("calendar", &format!("invitations: {e}"));
            false
        });
        (
            calendar::sources(&store)
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.kind != SourceKind::Local)
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            scanned,
        )
    };
    if scanned {
        let _ = app.emit("calendar-changed", ());
    }
    for id in ids {
        if let Err(e) = sync_one(app, id).await {
            mail_core::diag::log("calendar", &format!("source {id}: {e}"));
        }
    }
}

/// The loop: a round at start, every [`ROUND`] and after each [`kick`].
pub fn start_loop(app: &AppHandle) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let notify = wake();
        loop {
            round(&app).await;
            let _ = tokio::time::timeout(ROUND, notify.notified()).await;
            // Changes made one after another (dragging, typing) go together.
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    });
}

// ---------------------------------------------------------------------------- invitations

#[derive(Serialize)]
pub struct Answered {
    /// The appointment in the calendar, when it stays there.
    event_key: Option<String>,
    /// How the organizer heard: `mail` (an iTIP reply), `microsoft` (Graph), `server`
    /// (the CalDAV server schedules), `none` (no organizer to tell).
    sent: String,
}

/// Answers an invitation: tells the organizer and files the appointment (or takes it out).
#[tauri::command]
pub async fn invitation_answer(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    answer: Answer,
    comment: Option<String>,
) -> Res<Answered> {
    let comment = comment.unwrap_or_default();
    let (account, data, inv) = {
        let store = state.store();
        let detail = store.message(message_id)?;
        let account = store.account(detail.account_id)?;
        let data = itip::calendar_data(&store, message_id)?.ok_or_else(|| {
            Error::Invalid(
                mail_core::tr!("Die Einladung ist noch nicht geladen.", "The invitation is not loaded yet.").into(),
            )
        })?;
        let me = calendar::my_addresses(&store)?;
        let inv = itip::parse(&data, &me).ok_or_else(|| {
            Error::Invalid(mail_core::tr!("Die Einladung ist nicht lesbar.", "The invitation cannot be read.").into())
        })?;
        (account, data, inv)
    };
    // A Microsoft account with its calendars on: Exchange has the meeting, Graph answers.
    let graph_source = {
        let store = state.store();
        match calendar::account_source(&store, account.id)? {
            Some(source) if calendar::source(&store, source)?.kind == SourceKind::Graph => Some(source),
            _ => None,
        }
    };
    if let Some(source) = graph_source {
        let token = crate::tools::graph_token(account.id).await?;
        let g = mail_core::graph::Graph::new(&token, mail_core::network::Service::Graph)?;
        let event = calendar::graph::find_by_uid(&g, &inv.uid).await?.ok_or_else(|| {
            Error::Invalid(
                mail_core::tr!(
                    "Microsoft hat den Termin noch nicht im Kalender. Versuch es gleich noch einmal.",
                    "Microsoft does not have the appointment in the calendar yet. Try again in a moment."
                )
                .into(),
            )
        })?;
        let id = event["id"].as_str().unwrap_or_default().to_string();
        calendar::graph::respond(&g, &id, answer.graph_action(), &comment).await?;
        itip::answered(&state.store(), message_id, &inv, answer)?;
        sync_one(&app, source).await.ok();
        let key = calendar::objects_by_uid(&state.store(), &inv.uid)?
            .into_iter()
            .next()
            .map(|o| format!("{}|{}|", o.calendar_id, o.id));
        return Ok(Answered { event_key: key, sent: "microsoft".into() });
    }
    let (key, server_schedules) = {
        let store = state.store();
        let chosen = mail_core::settings::load(&store)?.calendar.default_calendar;
        let target = calendar::default_calendar(&store, chosen)?;
        let existing = calendar::objects_by_uid(&store, &inv.uid)?;
        // A CalDAV server that schedules put the invitation into the calendar and answers itself.
        let server_schedules = existing.iter().any(|o| {
            calendar::calendar(&store, o.calendar_id)
                .ok()
                .and_then(|c| calendar::source(&store, c.source_id).ok())
                .is_some_and(|s| s.kind == SourceKind::Caldav && s.auto_schedule)
        });
        let key = itip::file(&store, &data, &inv, answer, target)?;
        itip::answered(&store, message_id, &inv, answer)?;
        (key, server_schedules)
    };
    kick();
    if server_schedules {
        return Ok(Answered { event_key: key, sent: "server".into() });
    }
    if calendar::attendee_email(&inv.organizer).is_none() {
        return Ok(Answered { event_key: key, sent: "none".into() });
    }
    let (raw, to) = itip::reply_mail(&account, &inv, &data, answer, &comment)?;
    mail_core::actions::queue_raw(&state.store(), account.id, &raw, &[to])?;
    state.kick(&[account.id]);
    Ok(Answered { event_key: key, sent: "mail".into() })
}
