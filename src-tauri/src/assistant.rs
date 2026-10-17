//! The AI assistant's commands (`ui/src/lib/api.ts`, `ai*`): the core prepares each request
//! and applies the privacy rule (`mail_core::ai::assistant`), this module fetches the
//! provider's key from the credential store, streams the answer to the UI (`ai-delta` events)
//! without holding the store, and lets the user cancel it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mail_core::Error;
use mail_core::ai::assistant::{self, Outcome, Request};
use mail_core::ai::extract::Item;
use mail_core::ai::priority::{self, By};
use mail_core::ai::tone::{self, Tone};
use mail_core::ai::{AiClient, AiProvider, Answer, ChatMessage, Destination, provider};
use mail_core::i18n::lang;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{AppState, secrets};

type Res<T> = Result<T, Error>;

/// A piece of a streamed answer.
#[derive(Clone, Serialize)]
struct Delta<'a> {
    id: &'a str,
    text: &'a str,
}

fn key_of(p: &AiProvider) -> Option<String> {
    secrets::get(&provider::key_name(&p.id)).ok().flatten()
}

/// Runs a request: prepared by the core, answered by the account's provider, kept by the core.
/// `offset`: the user's offset from UTC in seconds (the dates of the prompts).
pub async fn run(app: &AppHandle, state: &AppState, id: &str, request: Request, offset: i64) -> Res<Outcome> {
    let prepared = {
        let store = state.store();
        let settings = mail_core::settings::load(&store)?;
        assistant::prepare(&store, &settings.ai, &request, lang(), mail_core::store::now(), offset)?
    };
    if let Some(text) = prepared.cached.clone() {
        return assistant::finish(&state.store(), &prepared, Answer { text, cancelled: false }, true);
    }
    let client = AiClient::new(prepared.provider.clone(), key_of(&prepared.provider))?;
    let cancel = Arc::new(AtomicBool::new(false));
    state.ai_jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), cancel.clone());
    let answer = client
        .chat(&prepared.messages, prepared.temperature, &cancel, |text| {
            let _ = app.emit("ai-delta", Delta { id, text });
        })
        .await;
    state.ai_jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    assistant::finish(&state.store(), &prepared, answer?, false)
}

#[tauri::command]
pub async fn ai_run(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    request: Request,
    offset: i64,
) -> Res<Outcome> {
    run(&app, &state, &id, request, offset).await
}

/// Stops a streamed answer; what arrived stays.
#[tauri::command]
pub fn ai_cancel(state: State<'_, AppState>, id: String) {
    if let Some(flag) = state.ai_jobs.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Where the text of an account goes (the privacy line next to every AI button).
#[tauri::command]
pub fn ai_destination(state: State<'_, AppState>, account_id: i64) -> Res<Destination> {
    Ok(mail_core::settings::load(&state.store())?.ai.destination(account_id))
}

/// The providers that answer questions to the mailbox, each with the accounts it may see.
#[tauri::command]
pub fn ai_ask_providers(state: State<'_, AppState>) -> Res<Vec<Destination>> {
    let store = state.store();
    let settings = mail_core::settings::load(&store)?;
    assistant::ask_providers(&settings.ai, &store)
}

/// A summary kept from before, and whether the conversation is long enough for one to come by
/// itself.
#[derive(Serialize)]
pub struct ThreadInfo {
    summary: Option<String>,
    long: bool,
}

#[tauri::command]
pub fn ai_thread(state: State<'_, AppState>, message_id: i64) -> Res<ThreadInfo> {
    let store = state.store();
    Ok(ThreadInfo {
        summary: assistant::kept_summary(&store, message_id, lang())?,
        long: assistant::long_thread(&store, message_id)?,
    })
}

/// The models of a provider (Settings → KI), with the key typed in or the stored one.
#[tauri::command]
pub async fn ai_models(provider: AiProvider, key: Option<String>) -> Res<Vec<String>> {
    let key = key.filter(|k| !k.trim().is_empty()).or_else(|| key_of(&provider));
    let client = AiClient::new(provider.clone(), key)?;
    if provider.kind == mail_core::ai::ProviderKind::Azure {
        // Deployments cannot be listed: one short request checks the key and the deployment.
        let msgs = [ChatMessage::user("OK")];
        client.chat(&msgs, 0.0, &AtomicBool::new(false), |_| {}).await?;
        return Ok(vec![provider.model.clone()]);
    }
    client.models().await
}

/// Keeps a provider's key in the credential store (an empty one removes it).
#[tauri::command]
pub fn ai_set_key(provider_id: String, key: String) -> Res<()> {
    let name = provider::key_name(&provider::slug(&provider_id));
    if key.trim().is_empty() { secrets::delete(&name) } else { secrets::set(&name, key.trim()) }
}

#[tauri::command]
pub fn ai_has_key(provider_id: String) -> bool {
    secrets::get(&provider::key_name(&provider_id)).ok().flatten().is_some()
}

#[tauri::command]
pub fn ai_tone(state: State<'_, AppState>, account_id: i64) -> Res<Option<Tone>> {
    tone::load(&state.store(), account_id)
}

/// Learns the tone from the account's sent mail, on this device.
#[tauri::command]
pub fn ai_learn_tone(state: State<'_, AppState>, account_id: i64) -> Res<Option<Tone>> {
    tone::learn(&state.store(), account_id)
}

#[tauri::command]
pub fn ai_forget_tone(state: State<'_, AppState>, account_id: i64) -> Res<()> {
    tone::forget(&state.store(), account_id)
}

/// Forgets every kept answer (summaries, translations, tasks).
#[tauri::command]
pub fn ai_forget_results(state: State<'_, AppState>) -> Res<()> {
    assistant::forget_results(&state.store())
}

/// „Wichtig“ set or taken off by the user (the assistant does not change it again).
#[tauri::command]
pub fn set_important(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>, important: bool) -> Res<()> {
    let store = state.store();
    for id in &ids {
        priority::set(&store, *id, important, By::User)?;
    }
    let accounts: Vec<i64> = store.summaries(&ids)?.into_iter().map(|m| m.account_id).collect();
    drop(store);
    for a in accounts {
        let _ = app.emit("mail-notice", mail_core::engine::Notice::Changed { account_id: a, mailboxes: vec![] });
    }
    Ok(())
}

/// A reminder of a task found in a message, at its time.
#[tauri::command]
pub fn ai_remind(state: State<'_, AppState>, message_id: i64, due: i64, note: String) -> Res<()> {
    mail_core::later::remind_task(&state.store(), message_id, due, &note)?;
    Ok(())
}

/// An appointment found in a message as a draft for the calendar: the calendar opens with it in
/// its editor; nothing is written before the user creates it.
#[tauri::command]
pub fn ai_calendar(state: State<'_, AppState>, message_id: i64, item: Item) -> Res<mail_core::itip::Draft> {
    let description = {
        let store = state.store();
        let d = store.message(message_id)?;
        let from = d.summary.from.map(|a| a.name.unwrap_or(a.email)).unwrap_or_default();
        mail_core::trf!("Aus der Mail „{}“ von {from}", "From the mail “{}” from {from}", d.summary.subject)
    };
    mail_core::itip::draft_from(&item, &description).ok_or_else(|| {
        Error::Invalid(mail_core::tr!("Der Termin hat kein Datum.", "The appointment has no date.").into())
    })
}

/// New mail of an account whose provider may see it: the model decides about „Wichtig“ (a few
/// messages at a time, in the background). The local heuristics ran in the worker already.
pub fn judge_new_mail(app: &AppHandle, account_id: i64, messages: &[i64]) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let on = mail_core::settings::load(&state.store())
        .map(|s| s.ai.on() && s.ai.priority && s.ai.for_account(account_id).is_some())
        .unwrap_or(false);
    if !on || messages.is_empty() {
        return;
    }
    let app = app.clone();
    let ids: Vec<i64> = messages.iter().copied().take(5).collect();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else { return };
        let mut changed = false;
        for id in ids {
            let job = format!("priority-{id}");
            match run(&app, &state, &job, Request::Priority { message_id: id }, 0).await {
                Ok(_) => changed = true,
                Err(e) => {
                    mail_core::diag::log("ai", &format!("priority: {e}"));
                    break;
                }
            }
        }
        if changed {
            let _ = app.emit("mail-notice", mail_core::engine::Notice::Changed { account_id, mailboxes: vec![] });
        }
    });
}
