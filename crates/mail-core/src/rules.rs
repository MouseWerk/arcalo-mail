//! Rules: one model for the rules the app runs itself on new mail (local) and the rules that
//! live on the server (Sieve over ManageSieve, [`crate::sieve`]; Microsoft inbox rules through
//! Graph, [`crate::graph`]). A rule has conditions (all or any must hold) and actions, an order
//! and a switch. Local rules run in the account's worker right after new mail arrived in the
//! inbox, before anything is shown or told; every action is an ordinary local-first action
//! (`actions`), so it reaches the server through the queue.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::actions;
use crate::compose::{Draft, FileAttachment};
use crate::error::{Error, Result};
use crate::later;
use crate::parse::Address;
use crate::store::{MessageSummary, SUMMARY_COLS, Store, now, summary};
use crate::triage::{self, Category};
use crate::{tr, trf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// The app runs it on new mail.
    #[default]
    Local,
    /// The server runs it (also while the app is closed).
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// Sender: name or address.
    From,
    To,
    Cc,
    Subject,
    /// The mailing list (`List-Id`).
    List,
    HasAttachment,
    /// Size in KB.
    Size,
    /// Age in days (for „run now“ and the preview; new mail is never old).
    Age,
    /// The inbox tab (`people`, `newsletters`, …).
    Category,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Contains,
    NotContains,
    Is,
    StartsWith,
    /// Size, age: more than.
    Over,
    /// Size, age: less than.
    Under,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub field: Field,
    pub op: Op,
    /// Text, a number (KB, days), `true`/`false` (attachment) or a category.
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Action {
    /// Into the folder of this name (created when missing).
    Move {
        folder: String,
    },
    /// An IMAP keyword (Microsoft: a category).
    Label {
        keyword: String,
    },
    MarkRead,
    Flag,
    Pin,
    /// A copy to this address.
    Forward {
        to: String,
    },
    /// To the trash.
    Delete,
    /// Out of the inbox into the archive.
    SkipInbox,
    /// Into this inbox tab.
    Category {
        category: String,
    },
}

/// What a rule does, as stored (`rules.body`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleBody {
    /// All conditions must hold; else any one.
    pub match_all: bool,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// No later rule runs on a message this one caught.
    pub stop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub id: i64,
    /// `None`: every account (local rules only).
    pub account_id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub place: Place,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub position: i64,
    pub body: RuleBody,
    /// The id of the rule on the server (Microsoft), if it has one.
    #[serde(default)]
    pub server_ref: Option<String>,
}

fn yes() -> bool {
    true
}

impl Rule {
    /// Checks a rule before it is saved.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::Invalid(tr!("Gib der Regel einen Namen.", "Give the rule a name.").into()));
        }
        if self.body.conditions.is_empty() {
            return Err(Error::Invalid(
                tr!("Eine Regel braucht mindestens eine Bedingung.", "A rule needs at least one condition.").into(),
            ));
        }
        if self.body.actions.is_empty() {
            return Err(Error::Invalid(
                tr!("Eine Regel braucht mindestens eine Aktion.", "A rule needs at least one action.").into(),
            ));
        }
        for c in &self.body.conditions {
            let numeric = matches!(c.field, Field::Size | Field::Age);
            if numeric && (c.value.trim().parse::<u64>().is_err() || !matches!(c.op, Op::Over | Op::Under)) {
                return Err(Error::Invalid(
                    tr!(
                        "Größe und Alter brauchen eine Zahl und „mehr als“ oder „weniger als“.",
                        "Size and age need a number and “more than” or “less than”."
                    )
                    .into(),
                ));
            }
            if !numeric && !matches!(c.field, Field::HasAttachment | Field::Category) && c.value.trim().is_empty() {
                return Err(Error::Invalid(
                    tr!("Eine Bedingung hat noch keinen Text.", "A condition has no text yet.").into(),
                ));
            }
        }
        for a in &self.body.actions {
            match a {
                Action::Move { folder } if folder.trim().is_empty() => {
                    return Err(Error::Invalid(tr!("Wähle den Ordner.", "Choose the folder.").into()));
                }
                Action::Forward { to } => {
                    crate::compose::parse_address(to)?;
                }
                Action::Label { keyword } if keyword.trim().is_empty() => {
                    return Err(Error::Invalid(tr!("Gib das Schlagwort ein.", "Enter the label.").into()));
                }
                Action::Category { category } if Category::parse(category).is_none() => {
                    return Err(Error::Invalid(tr!("Wähle den Bereich.", "Choose the tab.").into()));
                }
                _ => {}
            }
        }
        if self.place == Place::Server && self.account_id.is_none() {
            return Err(Error::Invalid(
                tr!("Eine Regel auf dem Server gehört zu einem Konto.", "A rule on the server belongs to one account.")
                    .into(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------- the store

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Rule> {
    let body: String = r.get(6)?;
    Ok(Rule {
        id: r.get(0)?,
        account_id: r.get(1)?,
        name: r.get(2)?,
        place: if r.get::<_, String>(3)? == "server" { Place::Server } else { Place::Local },
        enabled: r.get(4)?,
        position: r.get(5)?,
        body: serde_json::from_str(&body).unwrap_or_default(),
        server_ref: r.get(7)?,
    })
}

const COLS: &str = "id, account_id, name, place, enabled, position, body, server_ref";

/// Every rule in order.
pub fn list(store: &Store) -> Result<Vec<Rule>> {
    let mut st = store.conn().prepare(&format!("SELECT {COLS} FROM rules ORDER BY position, id"))?;
    let rows = st.query_map([], row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(store: &Store, id: i64) -> Result<Rule> {
    store
        .conn()
        .query_row(&format!("SELECT {COLS} FROM rules WHERE id = ?1"), [id], row)
        .optional()?
        .ok_or_else(|| Error::NotFound(tr!("Die Regel gibt es nicht mehr.", "The rule is gone.").into()))
}

/// Saves a rule (new without an id, at the end); returns its id.
pub fn save(store: &Store, rule: &Rule) -> Result<i64> {
    rule.validate()?;
    let place = if rule.place == Place::Server { "server" } else { "local" };
    let body = serde_json::to_string(&rule.body)?;
    if rule.id > 0 {
        store.conn().execute(
            "UPDATE rules SET account_id = ?2, name = ?3, place = ?4, enabled = ?5, body = ?6, server_ref = ?7, updated_at = ?8 WHERE id = ?1",
            params![rule.id, rule.account_id, rule.name.trim(), place, rule.enabled, body, rule.server_ref, now()],
        )?;
        return Ok(rule.id);
    }
    let pos: i64 = store.conn().query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM rules", [], |r| r.get(0))?;
    store.conn().execute(
        "INSERT INTO rules (account_id, name, place, enabled, position, body, server_ref, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![rule.account_id, rule.name.trim(), place, rule.enabled, pos, body, rule.server_ref, now()],
    )?;
    Ok(store.conn().last_insert_rowid())
}

pub fn delete(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("DELETE FROM rules WHERE id = ?1", [id])?;
    Ok(())
}

pub fn set_enabled(store: &Store, id: i64, enabled: bool) -> Result<()> {
    store
        .conn()
        .execute("UPDATE rules SET enabled = ?2, updated_at = ?3 WHERE id = ?1", params![id, enabled, now()])?;
    Ok(())
}

pub fn set_server_ref(store: &Store, id: i64, server_ref: Option<&str>) -> Result<()> {
    store.conn().execute("UPDATE rules SET server_ref = ?2 WHERE id = ?1", params![id, server_ref])?;
    Ok(())
}

/// The order the user set: the ids from first to last.
pub fn reorder(store: &Store, ids: &[i64]) -> Result<()> {
    store.batch(|| {
        for (i, id) in ids.iter().enumerate() {
            store.conn().execute("UPDATE rules SET position = ?2 WHERE id = ?1", params![id, i as i64 + 1])?;
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------- server rules

/// Where an account's server rules live, as the last check found it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRules {
    pub account_id: i64,
    /// `sieve`, `graph` or `none`.
    pub kind: String,
    /// The ManageSieve server (Sieve only).
    pub server: Option<crate::account::Server>,
    /// The script as the app last uploaded it.
    pub script: Option<String>,
    pub checked_at: i64,
    pub error: Option<String>,
}

pub fn server_rules(store: &Store, account_id: i64) -> Result<Option<ServerRules>> {
    Ok(store
        .conn()
        .query_row(
            "SELECT kind, server, script, checked_at, error FROM rule_servers WHERE account_id = ?1",
            [account_id],
            |r| {
                let server: Option<String> = r.get(1)?;
                Ok(ServerRules {
                    account_id,
                    kind: r.get(0)?,
                    server: server.and_then(|s| serde_json::from_str(&s).ok()),
                    script: r.get(2)?,
                    checked_at: r.get(3)?,
                    error: r.get(4)?,
                })
            },
        )
        .optional()?)
}

/// Notes what a check or an upload found.
pub fn set_server_rules(store: &Store, s: &ServerRules) -> Result<()> {
    let server = s.server.as_ref().map(serde_json::to_string).transpose()?;
    store.conn().execute(
        "INSERT INTO rule_servers (account_id, kind, server, script, checked_at, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (account_id) DO UPDATE SET kind = excluded.kind, server = excluded.server, script = excluded.script,
           checked_at = excluded.checked_at, error = excluded.error",
        params![s.account_id, s.kind, server, s.script, s.checked_at, s.error],
    )?;
    Ok(())
}

/// The enabled server rules of an account, in order.
pub fn server_rules_of(store: &Store, account_id: i64) -> Result<Vec<Rule>> {
    Ok(list(store)?.into_iter().filter(|r| r.place == Place::Server && r.account_id == Some(account_id)).collect())
}

// ---------------------------------------------------------------------------- matching

/// What the conditions look at.
#[derive(Debug, Clone)]
struct Facts {
    id: i64,
    account_id: i64,
    from: String,
    to: Vec<Address>,
    cc: Vec<Address>,
    subject: String,
    list_id: String,
    has_attachments: bool,
    size: u64,
    date: i64,
    category: Category,
}

fn facts(store: &Store, id: i64) -> Result<Option<Facts>> {
    let r = store
        .conn()
        .query_row(
            "SELECT account_id, COALESCE(from_name, ''), COALESCE(from_email, ''), to_json, cc_json, subject,
               COALESCE(list_id, ''), has_attachments, size, COALESCE(date, internal_date)
             FROM messages WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, bool>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, i64>(9)?,
                ))
            },
        )
        .optional()?;
    let Some((account_id, name, email, to, cc, subject, list_id, has_attachments, size, date)) = r else {
        return Ok(None);
    };
    Ok(Some(Facts {
        id,
        account_id,
        from: format!("{name} <{email}>"),
        to: serde_json::from_str(&to).unwrap_or_default(),
        cc: serde_json::from_str(&cc).unwrap_or_default(),
        subject,
        list_id,
        has_attachments,
        size: size.max(0) as u64,
        date,
        category: triage::category_of(store, id)?,
    }))
}

fn text_matches(hay: &str, op: Op, value: &str) -> bool {
    let (h, v) = (hay.to_lowercase(), value.trim().to_lowercase());
    match op {
        Op::Contains => h.contains(&v),
        Op::NotContains => !h.contains(&v),
        Op::Is => h.trim() == v,
        Op::StartsWith => h.trim_start().starts_with(&v),
        Op::Over | Op::Under => false,
    }
}

fn addresses_match(list: &[Address], op: Op, value: &str) -> bool {
    let each = |a: &Address| {
        let both = format!("{} <{}>", a.name.as_deref().unwrap_or_default(), a.email);
        match op {
            Op::Is => a.email.eq_ignore_ascii_case(value.trim()),
            _ => text_matches(&both, op, value),
        }
    };
    match op {
        Op::NotContains => list.iter().all(each),
        _ => list.iter().any(each),
    }
}

fn condition_holds(f: &Facts, c: &Condition, at: i64) -> bool {
    let number = c.value.trim().parse::<u64>().unwrap_or(0);
    let compare = |x: u64| match c.op {
        Op::Over => x > number,
        Op::Under => x < number,
        _ => false,
    };
    match c.field {
        Field::From => match c.op {
            Op::Is => {
                f.from.rsplit('<').next().unwrap_or_default().trim_end_matches('>').eq_ignore_ascii_case(c.value.trim())
            }
            op => text_matches(&f.from, op, &c.value),
        },
        Field::To => addresses_match(&f.to, c.op, &c.value),
        Field::Cc => addresses_match(&f.cc, c.op, &c.value),
        Field::Subject => text_matches(&f.subject, c.op, &c.value),
        Field::List => !f.list_id.is_empty() && text_matches(&f.list_id, c.op, &c.value),
        Field::HasAttachment => f.has_attachments == !matches!(c.value.trim(), "false" | "no" | "0"),
        Field::Size => compare(f.size / 1024),
        Field::Age => compare(((at - f.date).max(0) / 86400) as u64),
        Field::Category => {
            Category::parse(c.value.trim()).is_some_and(|x| (x == f.category) == (c.op != Op::NotContains))
        }
    }
}

fn applies(f: &Facts, r: &Rule, at: i64) -> bool {
    if r.account_id.is_some_and(|a| a != f.account_id) {
        return false;
    }
    let mut results = r.body.conditions.iter().map(|c| condition_holds(f, c, at));
    if r.body.match_all { results.all(|x| x) } else { results.any(|x| x) }
}

/// Whether a stored message matches the body's conditions (any account).
pub fn matches(store: &Store, id: i64, body: &RuleBody) -> Result<bool> {
    let rule = Rule {
        id: 0,
        account_id: None,
        name: String::new(),
        place: Place::Local,
        enabled: true,
        position: 0,
        body: body.clone(),
        server_ref: None,
    };
    Ok(facts(store, id)?.is_some_and(|f| applies(&f, &rule, now())))
}

// ---------------------------------------------------------------------------- running

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Messages a rule caught.
    pub matched: u32,
    /// Messages that left the folder (moved, archived, deleted).
    pub removed: Vec<i64>,
    /// Accounts whose queue got steps.
    pub accounts: Vec<i64>,
}

/// A forward of a message: its text below a short header, its attachments when the whole
/// message is kept.
fn forward(store: &Store, id: i64, to: &str) -> Result<()> {
    let d = store.message(id)?;
    let from = d.summary.from.as_ref().map(|a| match &a.name {
        Some(n) => format!("{n} <{}>", a.email),
        None => a.email.clone(),
    });
    let head = trf!(
        "---------- Weitergeleitete Nachricht ----------\nVon: {}\nBetreff: {}\n\n",
        "---------- Forwarded message ----------\nFrom: {}\nSubject: {}\n\n",
        from.unwrap_or_default(),
        d.summary.subject
    );
    let text =
        d.text.clone().unwrap_or_else(|| d.html.as_deref().map(crate::compose::html_to_text).unwrap_or_default());
    let mut files = vec![];
    if let Some(raw) = store.raw_message(id)? {
        for a in d.attachments.iter().filter(|a| !a.inline) {
            if let Some(data) = crate::parse::attachment_data(&raw, a.part_index) {
                files.push(FileAttachment {
                    name: a.filename.clone().unwrap_or_else(|| "attachment".into()),
                    mime_type: a.mime_type.clone(),
                    data,
                });
            }
        }
    }
    let draft = Draft {
        account_id: d.account_id,
        to: vec![to.trim().to_string()],
        subject: format!("Fwd: {}", d.summary.subject),
        text: format!("{head}{text}"),
        ..Default::default()
    };
    let (op, _) = actions::prepare_send(store, &draft, &files, now())?;
    crate::outbox::enqueue(store, d.account_id, &op)?;
    Ok(())
}

/// Carries out a rule's actions on one message; `true` when the message left its folder.
fn act(store: &Store, id: i64, actions_: &[Action], report: &mut Report) -> Result<bool> {
    let account = facts(store, id)?.map(|f| f.account_id).unwrap_or_default();
    // Moves go last: the other actions need the message where it is.
    let mut leave: Option<&Action> = None;
    for a in actions_ {
        match a {
            Action::MarkRead => {
                actions::set_flags(store, &[id], Some(true), None)?;
            }
            Action::Flag => {
                actions::set_flags(store, &[id], None, Some(true))?;
            }
            Action::Pin => later::set_pinned(store, &[id], true)?,
            Action::Label { keyword } => {
                actions::set_keyword(store, &[id], keyword, true)?;
            }
            Action::Forward { to } => forward(store, id, to)?,
            Action::Category { category } => {
                store.conn().execute("UPDATE messages SET category = ?2 WHERE id = ?1", params![id, category])?;
            }
            Action::Move { .. } | Action::Delete | Action::SkipInbox => leave = Some(a),
        }
    }
    report.accounts.push(account);
    let Some(a) = leave else { return Ok(false) };
    match a {
        Action::Move { folder } => {
            let cur: String = store.conn().query_row(
                "SELECT b.name FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id WHERE m.id = ?1",
                [id],
                |r| r.get(0),
            )?;
            if cur == folder.trim() {
                return Ok(false);
            }
            actions::move_to_named(store, &[id], folder.trim())?;
        }
        Action::Delete => {
            actions::delete(store, &[id])?;
        }
        _ => {
            actions::archive(store, &[id])?;
        }
    }
    Ok(true)
}

fn run_rules(store: &Store, ids: &[i64], rules: &[Rule], at: i64) -> Result<Report> {
    let mut report = Report::default();
    for &id in ids {
        let Some(f) = facts(store, id)? else { continue };
        let mut caught = false;
        for r in rules {
            if !applies(&f, r, at) {
                continue;
            }
            caught = true;
            if act(store, id, &r.body.actions, &mut report)? {
                report.removed.push(id);
                break;
            }
            if r.body.stop {
                break;
            }
        }
        if caught {
            report.matched += 1;
        }
    }
    report.accounts.sort_unstable();
    report.accounts.dedup();
    Ok(report)
}

/// Runs the enabled local rules of the account on its new messages (the worker, after the inbox
/// sync).
pub fn run_new(store: &Store, account_id: i64, ids: &[i64]) -> Result<Report> {
    let rules: Vec<Rule> = list(store)?
        .into_iter()
        .filter(|r| r.enabled && r.place == Place::Local && r.account_id.is_none_or(|a| a == account_id))
        .collect();
    if rules.is_empty() || ids.is_empty() {
        return Ok(Report::default());
    }
    run_rules(store, ids, &rules, now())
}

/// „Jetzt auf diesen Ordner anwenden“: one rule (enabled or not, local or server) on every
/// message of a folder.
pub fn run_on_folder(store: &Store, rule: &Rule, mailbox_id: i64) -> Result<Report> {
    let mb = store.mailbox(mailbox_id)?;
    if rule.account_id.is_some_and(|a| a != mb.account_id) {
        return Err(Error::Invalid(
            tr!("Die Regel gehört zu einem anderen Konto.", "The rule belongs to another account.").into(),
        ));
    }
    let ids: Vec<i64> = {
        let mut st = store
            .conn()
            .prepare("SELECT id FROM messages WHERE mailbox_id = ?1 AND hidden = 0 ORDER BY internal_date DESC")?;
        st.query_map([mailbox_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let mut r = rule.clone();
    r.enabled = true;
    run_rules(store, &ids, &[r], now())
}

/// „Vorschau“: the stored messages a rule would catch, newest first (not in Trash, Spam, Drafts
/// or Sent, one copy per message).
pub fn preview(store: &Store, rule: &Rule, limit: u32) -> Result<(u32, Vec<MessageSummary>)> {
    let mut st = store.conn().prepare(
        "SELECT m.id FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id
         WHERE m.hidden = 0 AND (?1 IS NULL OR m.account_id = ?1)
           AND COALESCE(b.role, '') NOT IN ('trash', 'junk', 'drafts', 'sent', 'all', 'flagged', 'important')
         ORDER BY COALESCE(m.date, m.internal_date) DESC, m.id DESC LIMIT 5000",
    )?;
    let ids: Vec<i64> = st.query_map([rule.account_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let at = now();
    let mut r = rule.clone();
    r.enabled = true;
    let mut hits = vec![];
    let mut total = 0;
    for id in ids {
        let Some(f) = facts(store, id)? else { continue };
        if applies(&f, &r, at) {
            total += 1;
            if hits.len() < limit as usize {
                hits.push(f.id);
            }
        }
    }
    let mut st = store.conn().prepare(&format!("SELECT {SUMMARY_COLS} FROM messages WHERE id = ?1"))?;
    let mut out = vec![];
    for id in hits {
        if let Some(s) = st.query_row([id], summary).optional()? {
            out.push(s);
        }
    }
    Ok((total, out))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::imap::MailboxEntry;
    use crate::outbox;
    use crate::parse::parse;
    use crate::store::tests::{header, store_with_inbox};

    pub(crate) fn put(s: &Store, inbox: i64, uid: u32, head: &str, subject: &str, body: &str) -> i64 {
        let raw = format!(
            "{head}\r\nSubject: {subject}\r\nMessage-ID: <r{uid}@example.org>\r\nDate: Fri, 16 Oct 2026 09:00:00 +0000\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        );
        let p = parse(raw.as_bytes()).unwrap();
        let id = s.insert_message(inbox, &header(uid, raw.as_bytes()), &p).unwrap();
        s.set_body_raw(id, &p, Some(raw.as_bytes())).unwrap();
        id
    }

    pub(crate) fn rule(name: &str, conditions: Vec<Condition>, actions: Vec<Action>) -> Rule {
        Rule {
            id: 0,
            account_id: None,
            name: name.into(),
            place: Place::Local,
            enabled: true,
            position: 0,
            body: RuleBody { match_all: true, conditions, actions, stop: false },
            server_ref: None,
        }
    }

    pub(crate) fn cond(field: Field, op: Op, value: &str) -> Condition {
        Condition { field, op, value: value.into() }
    }

    #[test]
    fn validation_says_what_is_missing() {
        let mut r = rule("", vec![], vec![]);
        assert!(r.validate().unwrap_err().to_string().contains("name"));
        r.name = "Rechnungen".into();
        assert!(r.validate().unwrap_err().to_string().contains("condition"));
        r.body.conditions.push(cond(Field::Size, Op::Contains, "x"));
        r.body.actions.push(Action::MarkRead);
        assert!(r.validate().unwrap_err().to_string().contains("number"));
        r.body.conditions[0] = cond(Field::Size, Op::Over, "500");
        assert!(r.validate().is_ok());
        r.place = Place::Server;
        assert!(r.validate().is_err(), "a server rule belongs to one account");
    }

    #[test]
    fn stored_in_order_and_switched() {
        let (s, account, _) = store_with_inbox();
        let a = save(&s, &rule("A", vec![cond(Field::Subject, Op::Contains, "x")], vec![Action::Flag])).unwrap();
        let b = save(
            &s,
            &Rule {
                account_id: Some(account),
                ..rule("B", vec![cond(Field::From, Op::Is, "a@b.c")], vec![Action::Pin])
            },
        )
        .unwrap();
        assert_eq!(list(&s).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), [a, b]);
        reorder(&s, &[b, a]).unwrap();
        assert_eq!(list(&s).unwrap().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["B", "A"]);
        set_enabled(&s, a, false).unwrap();
        assert!(!get(&s, a).unwrap().enabled);
        let mut edited = get(&s, b).unwrap();
        edited.name = "B2".into();
        save(&s, &edited).unwrap();
        assert_eq!(get(&s, b).unwrap().name, "B2");
        delete(&s, a).unwrap();
        assert_eq!(list(&s).unwrap().len(), 1);
    }

    #[test]
    fn conditions() {
        let (s, _, inbox) = store_with_inbox();
        let id = put(
            &s,
            inbox,
            1,
            "From: Rechnung <billing@shop.example>\r\nTo: Maria <maria@example.org>\r\nCc: Jonas <jonas@example.org>\r\nList-Id: <news.shop.example>",
            "Ihre Rechnung 2026-10",
            "Betrag 42 Euro",
        );
        let m = |c: Condition| {
            matches(&s, id, &RuleBody { match_all: true, conditions: vec![c], ..Default::default() }).unwrap()
        };
        assert!(m(cond(Field::From, Op::Contains, "shop.example")));
        assert!(m(cond(Field::From, Op::Is, "BILLING@shop.example")));
        assert!(!m(cond(Field::From, Op::Is, "shop.example")));
        assert!(m(cond(Field::To, Op::Is, "maria@example.org")));
        assert!(m(cond(Field::Cc, Op::Contains, "jonas")));
        assert!(m(cond(Field::To, Op::NotContains, "jonas")));
        assert!(m(cond(Field::Subject, Op::StartsWith, "ihre rechnung")));
        assert!(m(cond(Field::List, Op::Contains, "news.shop")));
        assert!(m(cond(Field::HasAttachment, Op::Is, "false")));
        assert!(m(cond(Field::Size, Op::Under, "10")));
        assert!(m(cond(Field::Age, Op::Under, "100000")));
        assert!(!m(cond(Field::Age, Op::Over, "100000")));
        assert!(m(cond(Field::Category, Op::Is, "receipts")));
        assert!(!m(cond(Field::Category, Op::Is, "people")));
        let any = RuleBody {
            match_all: false,
            conditions: vec![cond(Field::Subject, Op::Contains, "nope"), cond(Field::From, Op::Contains, "billing")],
            ..Default::default()
        };
        assert!(matches(&s, id, &any).unwrap());
        assert!(!matches(&s, id, &RuleBody { match_all: true, ..any }).unwrap());
    }

    #[test]
    fn new_mail_runs_through_the_rules_in_order() {
        let (s, account, inbox) = store_with_inbox();
        s.upsert_mailbox(
            account,
            &MailboxEntry { name: "Archive".into(), delimiter: Some('/'), attributes: vec!["\\Archive".into()] },
        )
        .unwrap();
        let bill =
            put(&s, inbox, 1, "From: Shop <billing@shop.example>\r\nTo: maria@example.org", "Rechnung 7", "42 Euro");
        let jonas = put(&s, inbox, 2, "From: Jonas <jonas@example.org>\r\nTo: maria@example.org", "Montag", "Passt?");
        let news =
            put(&s, inbox, 3, "From: News <news@paper.example>\r\nTo: maria@example.org", "Heute", "Nachrichten");
        let r1 = save(
            &s,
            &rule(
                "Rechnungen",
                vec![cond(Field::Subject, Op::Contains, "rechnung")],
                vec![
                    Action::Label { keyword: "$Rechnung".into() },
                    Action::MarkRead,
                    Action::Move { folder: "Rechnungen".into() },
                ],
            ),
        )
        .unwrap();
        save(&s, &rule("Jonas", vec![cond(Field::From, Op::Contains, "jonas")], vec![Action::Flag, Action::Pin]))
            .unwrap();
        save(
            &s,
            &Rule {
                enabled: false,
                ..rule("Aus", vec![cond(Field::From, Op::Contains, "news")], vec![Action::Delete])
            },
        )
        .unwrap();
        let r = run_new(&s, account, &[bill, jonas, news]).unwrap();
        assert_eq!(r.matched, 2);
        assert_eq!(r.removed, [bill]);
        let kw: String =
            s.conn().query_row("SELECT keywords FROM messages WHERE id = ?1", [bill], |r| r.get(0)).unwrap();
        assert!(kw.contains("$Rechnung"));
        assert!(s.mailbox_by_name(account, "Rechnungen").unwrap().is_some(), "the folder is created");
        let j = s.summaries(&[jonas]).unwrap();
        assert!(j[0].flagged);
        assert!(later::marks_of(&s, jonas).unwrap().pinned);
        assert_eq!(s.summaries(&[news]).unwrap().len(), 1, "a switched-off rule does nothing");
        let kinds: Vec<String> = outbox::entries(&s).unwrap().iter().map(|e| e.kind.clone()).collect();
        assert!(kinds.contains(&"keyword".to_string()) && kinds.contains(&"move".to_string()), "{kinds:?}");
        // A rule of another account leaves the mail alone.
        let second = s
            .add_account(&crate::account::AccountDraft {
                provider: crate::account::Provider::Imap,
                name: "Privat".into(),
                email: "m@example.net".into(),
                sender_name: String::new(),
                transport: crate::account::preset(crate::account::Provider::Imap, "m@example.net").unwrap(),
            })
            .unwrap();
        let mut other = get(&s, r1).unwrap();
        other.account_id = Some(second);
        save(&s, &other).unwrap();
        let again = put(&s, inbox, 4, "From: Shop <billing@shop.example>\r\nTo: maria@example.org", "Rechnung 8", "x");
        assert_eq!(run_new(&s, account, &[again]).unwrap().matched, 0);
    }

    #[test]
    fn preview_and_run_now_on_a_folder() {
        let (s, account, inbox) = store_with_inbox();
        for i in 1..=3 {
            put(
                &s,
                inbox,
                i,
                "From: Shop <billing@shop.example>\r\nTo: maria@example.org",
                &format!("Rechnung {i}"),
                "x",
            );
        }
        put(&s, inbox, 9, "From: Jonas <jonas@example.org>\r\nTo: maria@example.org", "Montag", "x");
        let r = rule("Shop", vec![cond(Field::From, Op::Contains, "shop.example")], vec![Action::SkipInbox]);
        let (total, rows) = preview(&s, &r, 2).unwrap();
        assert_eq!((total, rows.len()), (3, 2));
        assert!(rows.iter().all(|m| m.subject.starts_with("Rechnung")));
        let report = run_on_folder(&s, &r, inbox).unwrap();
        assert_eq!((report.matched, report.removed.len()), (3, 3));
        assert!(s.mailbox_by_name(account, "Archive").unwrap().is_some());
        assert_eq!(s.messages(inbox, 10, 0).unwrap().len(), 1, "only Jonas stays in the inbox");
    }

    #[test]
    fn forwards_with_the_attachments_of_kept_mail() {
        let (s, account, inbox) = store_with_inbox();
        let raw_head = "From: Shop <billing@shop.example>\r\nTo: maria@example.org\r\nSubject: Rechnung\r\nMessage-ID: <f1@x>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/plain\r\n\r\nAnbei.\r\n--B\r\nContent-Type: application/pdf; name=\"r.pdf\"\r\nContent-Disposition: attachment; filename=\"r.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0x\r\n--B--\r\n";
        let p = parse(raw_head.as_bytes()).unwrap();
        let id = s.insert_message(inbox, &header(1, raw_head.as_bytes()), &p).unwrap();
        s.set_body_raw(id, &p, Some(raw_head.as_bytes())).unwrap();
        let r = rule(
            "Weiter",
            vec![cond(Field::HasAttachment, Op::Is, "true")],
            vec![Action::Forward { to: "buchhaltung@example.org".into() }],
        );
        save(&s, &r).unwrap();
        assert_eq!(run_new(&s, account, &[id]).unwrap().matched, 1);
        let payload: String =
            s.conn().query_row("SELECT payload FROM outbox WHERE kind = 'send'", [], |r| r.get(0)).unwrap();
        let op: crate::outbox::Op = serde_json::from_str(&payload).unwrap();
        let crate::outbox::Op::Send { raw_b64, recipients, .. } = op else { panic!() };
        assert_eq!(recipients, ["buchhaltung@example.org"]);
        let raw =
            String::from_utf8(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, raw_b64).unwrap())
                .unwrap();
        assert!(raw.contains("Subject: Fwd: Rechnung"));
        assert!(raw.contains("r.pdf"));
    }
}
