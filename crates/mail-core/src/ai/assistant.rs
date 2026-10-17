//! What the UI asks the assistant for, in two steps around the network: [`prepare`] reads the
//! store, applies the privacy rule (which provider may see which account's mail) and builds the
//! prompt; the shell then streams the answer from the provider without holding the store
//! ([`super::AiClient::chat`]); [`finish`] keeps what is worth keeping (summaries,
//! translations, tasks, the priority) and reads the structured answers.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::ask::{self, Source};
use super::extract::{self, Item};
use super::priority::{self, By};
use super::prompts::{self, MailText, Style, Task};
use super::{AiProvider, AiSettings, Answer, ChatMessage, Destination, tone};
use crate::error::{Error, Result};
use crate::i18n::Lang;
use crate::store::Store;
use crate::{tr, trf};

/// Characters of one message's text in a prompt; a conversation keeps its newest messages
/// within [`THREAD_CHARS`].
const MESSAGE_CHARS: usize = 4000;
const THREAD_CHARS: usize = 16000;

/// A conversation is long (its summary comes by itself when that is on) from this many messages
/// or this much text.
pub const LONG_THREAD_MESSAGES: usize = 4;
pub const LONG_THREAD_CHARS: usize = 3000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "task", rename_all = "snake_case")]
pub enum Request {
    /// The conversation of the message.
    Summary {
        message_id: i64,
    },
    /// A reply to the message's conversation.
    Reply {
        message_id: i64,
        #[serde(default)]
        instructions: String,
    },
    /// Writing help on a draft of `account_id`.
    Rewrite {
        account_id: i64,
        text: String,
        style: Style,
    },
    Translate {
        message_id: i64,
        target: String,
    },
    Extract {
        message_id: i64,
    },
    Priority {
        message_id: i64,
    },
    /// A question to the mailbox; `provider`: which one (else the default), it sees only the
    /// accounts that allow it.
    Ask {
        question: String,
        #[serde(default)]
        provider: Option<String>,
    },
}

/// A request ready to go out.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub task: Task,
    pub provider: AiProvider,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    /// An answer kept from before, or one that needs no model (nothing found); nothing is sent.
    pub cached: Option<String>,
    pub sources: Vec<Source>,
    pub destination: Destination,
    cache: Option<(String, String, String)>,
    message_id: Option<i64>,
    offset: i64,
}

/// The answer as the UI shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    pub task: Task,
    pub text: String,
    pub cancelled: bool,
    /// Came from the store, nothing was sent.
    pub cached: bool,
    pub destination: Destination,
    /// Ask: the messages `[n]` refers to.
    pub sources: Vec<Source>,
    /// Extract: tasks and appointments.
    pub items: Vec<Item>,
    /// Priority: the model's decision and reason.
    pub important: Option<bool>,
    pub reason: Option<String>,
}

fn lang_code(lang: Lang) -> &'static str {
    match lang {
        Lang::De => "de",
        Lang::En => "en",
    }
}

/// A date for a prompt, in the user's local time (`offset` seconds east of UTC).
pub fn date_text(secs: i64, offset: i64, lang: Lang) -> String {
    let local = secs + offset;
    let (y, m, d, _) = super::civil(local);
    let t = local.rem_euclid(86400);
    match lang {
        Lang::De => format!("{d:02}.{m:02}.{y} {:02}:{:02}", t / 3600, t % 3600 / 60),
        Lang::En => format!("{y}-{m:02}-{d:02} {:02}:{:02}", t / 3600, t % 3600 / 60),
    }
}

/// Today with its weekday, for the prompts that work with dates.
pub fn today_text(now: i64, offset: i64, lang: Lang) -> String {
    let (y, m, d, wd) = super::civil(now + offset);
    let de = ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"];
    let en = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    match lang {
        Lang::De => format!("{}, {y}-{m:02}-{d:02}", de[wd as usize]),
        Lang::En => format!("{}, {y}-{m:02}-{d:02}", en[wd as usize]),
    }
}

fn account_label(store: &Store, account_id: i64) -> String {
    store.account(account_id).map(|a| if a.name.trim().is_empty() { a.email } else { a.name }).unwrap_or_default()
}

fn account_of(store: &Store, message_id: i64) -> Result<i64> {
    store
        .conn()
        .query_row("SELECT account_id FROM messages WHERE id = ?1", [message_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| Error::NotFound(tr!("Die Nachricht ist nicht mehr da.", "The message is gone.").into()))
}

fn mail_text(store: &Store, id: i64, offset: i64, lang: Lang, max: usize) -> Result<MailText> {
    let d = store.message(id)?;
    let from =
        d.summary.from.as_ref().map(|a| a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or(a.email.clone()));
    let mut text = super::plain_text(d.text.as_deref(), d.html.as_deref(), max);
    if text.is_empty() {
        text = d.summary.snippet.clone();
    }
    Ok(MailText {
        from: from.unwrap_or_default(),
        date: date_text(d.summary.date, offset, lang),
        subject: d.summary.subject,
        text,
    })
}

/// The conversation of a message as prompt texts, oldest first, the newest kept whole.
fn thread_texts(store: &Store, message_id: i64, offset: i64, lang: Lang) -> Result<(Vec<MailText>, String)> {
    let thread_id: i64 =
        store.conn().query_row("SELECT thread_id FROM messages WHERE id = ?1", [message_id], |r| r.get(0))?;
    let list = store.thread(thread_id)?;
    let list = if list.is_empty() { store.summaries(&[message_id])? } else { list };
    let mut out = vec![];
    let mut used = 0;
    for m in list.iter().rev() {
        let t = mail_text(store, m.id, offset, lang, MESSAGE_CHARS)?;
        used += t.text.chars().count();
        out.push(t);
        if used > THREAD_CHARS {
            break;
        }
    }
    out.reverse();
    let latest = list.last().map_or(message_id, |m| m.id);
    let key =
        format!("thread:{}:{}:{}", message_key_id(store, latest)?, list.len(), digest(&out.iter().collect::<Vec<_>>()));
    Ok((out, key))
}

/// The Message-ID of a message (`local:<row>` without one): answers kept about it follow the
/// message, not its row, whose id a later message may take.
fn message_key_id(store: &Store, id: i64) -> Result<String> {
    let mid: Option<String> = store
        .conn()
        .query_row("SELECT message_id FROM messages WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .flatten();
    Ok(mid.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| format!("local:{id}")))
}

/// A short hash of the texts a request sends.
fn digest(texts: &[&MailText]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for t in texts {
        for part in [&t.from, &t.subject, &t.text] {
            h.update(part.as_bytes());
            h.update([0]);
        }
    }
    h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// The key of answers kept about one message: its Message-ID and the hash of its text.
fn message_key(store: &Store, id: i64, m: &MailText) -> Result<String> {
    Ok(format!("msg:{}:{}", message_key_id(store, id)?, digest(&[m])))
}

/// Whether a conversation is long enough for its summary to come by itself.
pub fn long_thread(store: &Store, message_id: i64) -> Result<bool> {
    let thread_id: Option<i64> = store
        .conn()
        .query_row("SELECT thread_id FROM messages WHERE id = ?1", [message_id], |r| r.get(0))
        .optional()?;
    let Some(thread_id) = thread_id else { return Ok(false) };
    let list = store.thread(thread_id)?;
    if list.len() >= LONG_THREAD_MESSAGES {
        return Ok(true);
    }
    let mut chars = 0;
    for m in &list {
        let t: Option<String> = store
            .conn()
            .query_row("SELECT text FROM message_bodies WHERE message_id = ?1", [m.id], |r| r.get(0))
            .optional()?
            .flatten();
        chars += super::plain_text(t.as_deref(), None, 100_000).chars().count();
    }
    Ok(chars >= LONG_THREAD_CHARS)
}

fn cached(store: &Store, key: &str, kind: &str, lang: &str) -> Result<Option<String>> {
    Ok(store
        .conn()
        .query_row(
            "SELECT text FROM ai_results WHERE key = ?1 AND kind = ?2 AND lang = ?3",
            params![key, kind, lang],
            |r| r.get(0),
        )
        .optional()?)
}

/// A summary kept from before (the reading pane shows it without asking again).
pub fn kept_summary(store: &Store, message_id: i64, lang: Lang) -> Result<Option<String>> {
    let (_, key) = thread_texts(store, message_id, 0, lang)?;
    cached(store, &key, "summary", lang_code(lang))
}

/// Which provider answers questions to the mailbox, and which accounts it may search.
pub fn ask_scope(settings: &AiSettings, store: &Store, provider: Option<&str>) -> Result<(AiProvider, Vec<i64>)> {
    let accounts = store.accounts()?;
    let allowed = |p: &AiProvider| -> Vec<i64> {
        accounts.iter().filter(|a| settings.for_account(a.id).is_some_and(|x| x.id == p.id)).map(|a| a.id).collect()
    };
    let chosen = match provider {
        Some(id) => settings.provider(id).cloned(),
        None => accounts.iter().find_map(|a| settings.for_account(a.id).cloned()),
    };
    let Some(p) = chosen else {
        return Err(Error::Invalid(
            tr!(
                "Kein KI-Anbieter darf deine Mails sehen. Richte in den Einstellungen unter KI einen ein, zum Beispiel Ollama auf diesem Gerät.",
                "No AI provider may see your mail. Set one up in Settings under AI, for example Ollama on this device."
            )
            .into(),
        ));
    };
    let ids = allowed(&p);
    if ids.is_empty() {
        return Err(Error::Invalid(trf!(
            "Kein Konto erlaubt „{}“, seine Mails zu sehen.",
            "No account lets “{}” see its mail.",
            p.display_name()
        )));
    }
    Ok((p, ids))
}

/// The providers that may see at least one account, for the question box's choice.
pub fn ask_providers(settings: &AiSettings, store: &Store) -> Result<Vec<Destination>> {
    let mut out: Vec<Destination> = vec![];
    for a in store.accounts()? {
        let d = settings.destination(a.id);
        if d.provider.is_some() && !out.iter().any(|x| x.provider == d.provider) {
            out.push(d);
        }
    }
    Ok(out)
}

/// Reads what the request needs and builds its prompt. `now` and `offset` (the user's offset
/// from UTC in seconds) place the dates.
pub fn prepare(
    store: &Store,
    settings: &AiSettings,
    req: &Request,
    lang: Lang,
    now: i64,
    offset: i64,
) -> Result<Prepared> {
    let code = lang_code(lang);
    let route = |account_id: i64| -> Result<(AiProvider, Destination)> {
        let p = settings.require(account_id, &account_label(store, account_id))?;
        Ok((p, settings.destination(account_id)))
    };
    let base = |task: Task, provider: AiProvider, destination: Destination, messages: Vec<ChatMessage>| Prepared {
        task,
        provider,
        messages,
        temperature: 0.3,
        cached: None,
        sources: vec![],
        destination,
        cache: None,
        message_id: None,
        offset,
    };
    Ok(match req {
        Request::Summary { message_id } => {
            let (provider, dest) = route(account_of(store, *message_id)?)?;
            let (thread, key) = thread_texts(store, *message_id, offset, lang)?;
            let mut p = base(Task::Summary, provider, dest, prompts::summary(&thread, lang));
            p.cached = cached(store, &key, "summary", code)?;
            p.cache = Some((key, "summary".into(), code.into()));
            p.temperature = 0.2;
            p
        }
        Request::Reply { message_id, instructions } => {
            let account_id = account_of(store, *message_id)?;
            let (provider, dest) = route(account_id)?;
            let account = store.account(account_id)?;
            let me = [&account.sender_name, &account.name, &account.email]
                .into_iter()
                .find(|s| !s.trim().is_empty())
                .cloned()
                .unwrap_or_default();
            let tone = if settings.tone {
                match tone::load(store, account_id)? {
                    Some(t) => Some(t),
                    None => tone::learn(store, account_id)?,
                }
            } else {
                None
            };
            let (thread, _) = thread_texts(store, *message_id, offset, lang)?;
            let mut p =
                base(Task::Reply, provider, dest, prompts::reply(&thread, &me, tone.as_ref(), instructions, lang));
            p.temperature = 0.5;
            p
        }
        Request::Rewrite { account_id, text, style } => {
            if text.trim().is_empty() {
                return Err(Error::Invalid(
                    tr!("Es gibt noch keinen Text zum Überarbeiten.", "There is no text to revise yet.").into(),
                ));
            }
            let (provider, dest) = route(*account_id)?;
            base(Task::Rewrite, provider, dest, prompts::rewrite(text, *style, lang))
        }
        Request::Translate { message_id, target } => {
            let (provider, dest) = route(account_of(store, *message_id)?)?;
            let m = mail_text(store, *message_id, offset, lang, THREAD_CHARS)?;
            let kind = format!("translation:{}", target.trim().to_ascii_lowercase());
            let key = message_key(store, *message_id, &m)?;
            let mut p = base(Task::Translate, provider, dest, prompts::translate(&m, target, lang));
            p.cached = cached(store, &key, &kind, code)?;
            p.cache = Some((key, kind, code.into()));
            p.temperature = 0.1;
            p
        }
        Request::Extract { message_id } => {
            let (provider, dest) = route(account_of(store, *message_id)?)?;
            let m = mail_text(store, *message_id, offset, lang, MESSAGE_CHARS)?;
            let key = message_key(store, *message_id, &m)?;
            let mut p = base(Task::Extract, provider, dest, prompts::extract(&m, &today_text(now, offset, lang), lang));
            p.cached = cached(store, &key, "extract", code)?;
            p.cache = Some((key, "extract".into(), code.into()));
            p.message_id = Some(*message_id);
            p.temperature = 0.0;
            p
        }
        Request::Priority { message_id } => {
            let account_id = account_of(store, *message_id)?;
            let (provider, dest) = route(account_id)?;
            let account = store.account(account_id)?;
            let m = mail_text(store, *message_id, offset, lang, 2000)?;
            let me =
                if account.sender_name.trim().is_empty() { account.email.clone() } else { account.sender_name.clone() };
            let mut p = base(Task::Priority, provider, dest, prompts::priority(&m, &me, lang));
            p.message_id = Some(*message_id);
            p.temperature = 0.0;
            p
        }
        Request::Ask { question, provider } => {
            if question.trim().is_empty() {
                return Err(Error::Invalid(tr!("Schreib eine Frage.", "Write a question.").into()));
            }
            let (p, accounts) = ask_scope(settings, store, provider.as_deref())?;
            let dest = Destination {
                provider: Some(p.id.clone()),
                name: p.display_name().to_string(),
                host: p.host(),
                local: p.local(),
            };
            let found = ask::retrieve(store, question, &accounts, |at| date_text(at, offset, lang))?;
            let (sources, texts): (Vec<Source>, Vec<MailText>) = found.into_iter().unzip();
            let mut out =
                base(Task::Ask, p, dest, prompts::ask(question, &texts, &today_text(now, offset, lang), lang));
            if sources.is_empty() {
                // Nothing to ask the model about: nothing is sent.
                out.cached = Some(
                    tr!(
                        "Dazu habe ich in deinen Mails auf diesem Gerät nichts gefunden. Versuche andere Wörter, zum Beispiel einen Namen oder einen Betreff.",
                        "I found nothing about that in your mail on this device. Try other words, for example a name or a subject."
                    )
                    .into(),
                );
            }
            out.sources = sources;
            out.temperature = 0.2;
            out
        }
    })
}

/// Keeps and reads the answer.
pub fn finish(store: &Store, p: &Prepared, answer: Answer, from_cache: bool) -> Result<Outcome> {
    let text = answer.text.trim().to_string();
    let mut out = Outcome {
        task: p.task,
        text: text.clone(),
        cancelled: answer.cancelled,
        cached: from_cache,
        destination: p.destination.clone(),
        sources: vec![],
        items: vec![],
        important: None,
        reason: None,
    };
    if answer.cancelled {
        return Ok(out);
    }
    if let (Some((key, kind, lang)), false) = (&p.cache, from_cache) {
        store.conn().execute(
            "INSERT INTO ai_results (key, kind, lang, provider, text, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (key, kind, lang) DO UPDATE SET provider = excluded.provider, text = excluded.text, created_at = excluded.created_at",
            params![key, kind, lang, p.provider.id, text, crate::store::now()],
        )?;
    }
    match p.task {
        Task::Ask => {
            let cited = ask::cited(&text);
            out.sources = p.sources.iter().filter(|s| cited.contains(&s.n)).cloned().collect();
            if out.sources.is_empty() && !p.sources.is_empty() && !from_cache {
                out.sources = p.sources.clone();
            }
        }
        Task::Extract => out.items = extract::parse(&text, p.offset),
        Task::Priority => {
            let v = prompts::json_in(&text);
            out.important = v.as_ref().and_then(|v| v["important"].as_bool());
            out.reason = v.as_ref().and_then(|v| v["reason"].as_str().map(str::to_owned));
            if let (Some(important), Some(id)) = (out.important, p.message_id)
                && priority::decided_by(store, id)? != Some(By::User)
            {
                priority::set(store, id, important, By::Model)?;
            }
        }
        _ => {}
    }
    Ok(out)
}

/// Forgets every answer kept on this device (Settings → KI).
pub fn forget_results(store: &Store) -> Result<()> {
    store.conn().execute("DELETE FROM ai_results", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{OFF, ProviderKind};
    use crate::parse::parse;
    use crate::store::tests::{header, store_with_inbox};

    pub(crate) fn put(
        s: &Store,
        inbox: i64,
        uid: u32,
        from: &str,
        subject: &str,
        reply: Option<&str>,
        body: &str,
    ) -> i64 {
        let refs = reply.map(|r| format!("In-Reply-To: <{r}>\r\nReferences: <{r}>\r\n")).unwrap_or_default();
        let raw = format!(
            "From: {from}\r\nTo: Maria Muster <maria@example.org>\r\nSubject: {subject}\r\nMessage-ID: <t{uid}@example.org>\r\n{refs}Date: Fri, 16 Oct 2026 09:{uid:02}:00 +0000\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        );
        let p = parse(raw.as_bytes()).unwrap();
        let id = s.insert_message(inbox, &header(uid, raw.as_bytes()), &p).unwrap();
        s.set_body(id, &p).unwrap();
        id
    }

    fn settings() -> AiSettings {
        let mut s = AiSettings {
            providers: vec![AiProvider { model: "llama3.2".into(), ..AiProvider::new(ProviderKind::Ollama) }],
            ..Default::default()
        };
        s.normalize().unwrap();
        s
    }

    #[test]
    fn kept_answers_belong_to_their_message_not_to_its_row() {
        let (s, _, inbox) = store_with_inbox();
        let a = put(&s, inbox, 1, "Jonas <jonas@example.org>", "Rechnung", None, "Bitte bis Freitag zahlen.");
        let cfg = settings();
        let req = |id| Request::Translate { message_id: id, target: "English".into() };
        let p = prepare(&s, &cfg, &req(a), Lang::De, 0, 0).unwrap();
        finish(&s, &p, Answer { text: "Please pay by Friday.".into(), cancelled: false }, false).unwrap();
        let ex = prepare(&s, &cfg, &Request::Extract { message_id: a }, Lang::De, 0, 0).unwrap();
        finish(&s, &ex, Answer { text: "[]".into(), cancelled: false }, false).unwrap();
        let sum = prepare(&s, &cfg, &Request::Summary { message_id: a }, Lang::De, 0, 0).unwrap();
        finish(&s, &sum, Answer { text: "- Zahlen bis Freitag".into(), cancelled: false }, false).unwrap();
        assert_eq!(
            prepare(&s, &cfg, &req(a), Lang::De, 0, 0).unwrap().cached.as_deref(),
            Some("Please pay by Friday.")
        );
        // The message goes; the next one takes its row id.
        s.remove_messages(&[a]).unwrap();
        let b = put(&s, inbox, 2, "Lea <lea@example.org>", "Urlaub", None, "Ich bin ab Montag weg.");
        assert_eq!(a, b, "the row id is used again");
        assert!(prepare(&s, &cfg, &req(b), Lang::De, 0, 0).unwrap().cached.is_none(), "not the old translation");
        assert!(prepare(&s, &cfg, &Request::Extract { message_id: b }, Lang::De, 0, 0).unwrap().cached.is_none());
        assert!(kept_summary(&s, b, Lang::De).unwrap().is_none());
        // The same message stored again (another folder, synced anew) keeps its answer.
        let c = put(&s, inbox, 1, "Jonas <jonas@example.org>", "Rechnung", None, "Bitte bis Freitag zahlen.");
        assert_eq!(
            prepare(&s, &cfg, &req(c), Lang::De, 0, 0).unwrap().cached.as_deref(),
            Some("Please pay by Friday.")
        );
    }

    #[test]
    fn summaries_are_kept_and_asked_once() {
        let (s, account, inbox) = store_with_inbox();
        let a = put(&s, inbox, 1, "Jonas <jonas@example.org>", "Montag", None, "Passt dir Montag um neun?");
        let b = put(
            &s,
            inbox,
            2,
            "Lea <lea@example.org>",
            "Re: Montag",
            Some("t1@example.org"),
            "Ich kann erst um zehn.\n\n> Passt dir Montag um neun?",
        );
        let cfg = settings();
        let p = prepare(&s, &cfg, &Request::Summary { message_id: a }, Lang::De, 0, 7200).unwrap();
        assert_eq!(p.task, Task::Summary);
        assert!(p.cached.is_none());
        assert!(p.destination.local);
        let user = &p.messages[1].content;
        assert!(user.contains("Von: Jonas\nDatum: 16.10.2026 11:01"), "local time: {user}");
        assert!(user.contains("Ich kann erst um zehn.") && !user.contains("> Passt"), "quotes stay out: {user}");
        let o = finish(&s, &p, Answer { text: "- Termin Montag\n- Lea erst um zehn".into(), cancelled: false }, false)
            .unwrap();
        assert!(!o.cached);
        let again = prepare(&s, &cfg, &Request::Summary { message_id: b }, Lang::De, 0, 0).unwrap();
        assert_eq!(again.cached.as_deref(), Some("- Termin Montag\n- Lea erst um zehn"));
        assert_eq!(kept_summary(&s, a, Lang::De).unwrap().as_deref(), Some("- Termin Montag\n- Lea erst um zehn"));
        assert!(kept_summary(&s, a, Lang::En).unwrap().is_none(), "per language");
        // A new message in the conversation asks again.
        put(&s, inbox, 3, "Jonas <jonas@example.org>", "Re: Montag", Some("t2@example.org"), "Dann zehn.");
        assert!(prepare(&s, &cfg, &Request::Summary { message_id: a }, Lang::De, 0, 0).unwrap().cached.is_none());
        assert!(!long_thread(&s, a).unwrap());
        put(&s, inbox, 4, "Lea <lea@example.org>", "Re: Montag", Some("t3@example.org"), "Gut.");
        assert!(long_thread(&s, a).unwrap());
        // A cancelled answer is not kept.
        let p = prepare(&s, &cfg, &Request::Translate { message_id: a, target: "en".into() }, Lang::De, 0, 0).unwrap();
        finish(&s, &p, Answer { text: "Does Monday".into(), cancelled: true }, false).unwrap();
        assert!(
            prepare(&s, &cfg, &Request::Translate { message_id: a, target: "en".into() }, Lang::De, 0, 0)
                .unwrap()
                .cached
                .is_none()
        );
        let _ = account;
    }

    #[test]
    fn nothing_is_prepared_for_an_account_that_allows_no_provider() {
        let (s, account, inbox) = store_with_inbox();
        let a = put(&s, inbox, 1, "Jonas <jonas@example.org>", "Montag", None, "Passt Montag?");
        let mut cfg = settings();
        cfg.accounts.insert(account.to_string(), OFF.into());
        let e = prepare(&s, &cfg, &Request::Summary { message_id: a }, Lang::En, 0, 0).unwrap_err().to_string();
        assert!(e.contains("No AI provider may see the mail of the account “Arbeit”"), "{e}");
        assert!(
            prepare(&s, &cfg, &Request::Ask { question: "Montag".into(), provider: None }, Lang::En, 0, 0).is_err()
        );
        assert!(prepare(&s, &AiSettings::default(), &Request::Priority { message_id: a }, Lang::En, 0, 0).is_err());
    }

    #[test]
    fn questions_cite_their_messages_and_need_no_model_when_nothing_is_found() {
        let (s, _, inbox) = store_with_inbox();
        let offer =
            put(&s, inbox, 1, "Lea <lea@example.org>", "Angebot Website", None, "Das Angebot kommt am Dienstag.");
        let cfg = settings();
        let p = prepare(
            &s,
            &cfg,
            &Request::Ask { question: "Wann kommt das Angebot?".into(), provider: None },
            Lang::De,
            0,
            0,
        )
        .unwrap();
        assert_eq!(p.sources.len(), 1);
        assert!(p.messages[0].content.contains("Heute ist der Donnerstag, 1970-01-01"));
        let o = finish(&s, &p, Answer { text: "Am Dienstag [1].".into(), cancelled: false }, false).unwrap();
        assert_eq!(o.sources[0].message_id, offer);
        let none =
            prepare(&s, &cfg, &Request::Ask { question: "Urlaub Spanien".into(), provider: None }, Lang::De, 0, 0)
                .unwrap();
        assert!(none.cached.as_deref().unwrap().contains("found nothing"));
        assert_eq!(ask_providers(&cfg, &s).unwrap().len(), 1);
    }

    #[test]
    fn the_models_priority_counts_unless_the_user_decided() {
        let (s, _, inbox) = store_with_inbox();
        let a = put(&s, inbox, 1, "Jonas <jonas@example.org>", "Vertrag", None, "Bitte unterschreiben.");
        let cfg = settings();
        let p = prepare(&s, &cfg, &Request::Priority { message_id: a }, Lang::De, 0, 0).unwrap();
        let o = finish(
            &s,
            &p,
            Answer { text: r#"{"important": true, "reason": "Unterschrift nötig"}"#.into(), cancelled: false },
            false,
        )
        .unwrap();
        assert_eq!((o.important, o.reason.as_deref()), (Some(true), Some("Unterschrift nötig")));
        assert_eq!(priority::decided_by(&s, a).unwrap(), Some(By::Model));
        priority::set(&s, a, false, By::User).unwrap();
        finish(&s, &p, Answer { text: r#"{"important": true}"#.into(), cancelled: false }, false).unwrap();
        assert_eq!(priority::decided_by(&s, a).unwrap(), Some(By::User));
    }

    #[test]
    fn extract_reads_items_and_dates() {
        let (s, _, inbox) = store_with_inbox();
        let a = put(&s, inbox, 1, "Lea <lea@example.org>", "Termin", None, "Treffen am Dienstag um 14 Uhr.");
        let cfg = settings();
        let p = prepare(&s, &cfg, &Request::Extract { message_id: a }, Lang::De, 1_792_195_200, 7200).unwrap();
        assert!(p.messages[0].content.contains("Heute ist der Samstag, 2026-10-17"));
        let o = finish(
            &s,
            &p,
            Answer {
                text: r#"{"items":[{"kind":"event","title":"Treffen","date":"2026-10-20","time":"14:00"}]}"#.into(),
                cancelled: false,
            },
            false,
        )
        .unwrap();
        assert_eq!(o.items.len(), 1);
        assert_eq!(o.items[0].at, Some(crate::ai::days_from_civil(2026, 10, 20) * 86400 + 12 * 3600));
    }

    #[test]
    fn reply_drafts_learn_the_tone_first() {
        let (s, account, inbox) = store_with_inbox();
        let a = put(&s, inbox, 1, "Jonas <jonas@example.org>", "Montag", None, "Passt Montag?");
        let sent = s
            .upsert_mailbox(
                account,
                &crate::imap::MailboxEntry { name: "Sent".into(), delimiter: None, attributes: vec!["\\Sent".into()] },
            )
            .unwrap();
        put(
            &s,
            sent,
            1,
            "Maria Muster <maria@example.org>",
            "Re: Zahlen",
            None,
            "Hallo Lea,\n\nklar, mache ich gern bis Freitag.\n\nViele Grüße\nMaria",
        );
        let p =
            prepare(&s, &settings(), &Request::Reply { message_id: a, instructions: "zusagen".into() }, Lang::De, 0, 0)
                .unwrap();
        assert!(p.messages[0].content.contains("im Namen von Maria Muster"));
        assert!(p.messages[0].content.contains("Gruß wie sonst: „Viele Grüße“"), "{}", p.messages[0].content);
        assert!(tone::load(&s, account).unwrap().is_some(), "learned on this device");
        let mut no_tone = settings();
        no_tone.tone = false;
        let p = prepare(&s, &no_tone, &Request::Reply { message_id: a, instructions: String::new() }, Lang::De, 0, 0)
            .unwrap();
        assert!(!p.messages[0].content.contains("So schreibt"));
    }
}
