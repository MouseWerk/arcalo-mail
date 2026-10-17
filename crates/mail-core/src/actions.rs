//! What the user does to messages and folders, the local-first way: the store shows the change
//! at once, the offline queue ([`crate::outbox`]) carries it to the server. Moves, deletions
//! and sending wait a few seconds first, so [`undo`] can take them back. The shell's commands
//! call these and kick the account's worker.
//!
//! Also here: the message as the reading pane shows it ([`message_view`]), with the HTML
//! sanitized, its pictures inlined, the quote folded away and the remote images decided.

use std::collections::BTreeMap;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::compose::{self, Draft, FileAttachment};
use crate::error::{Error, Result};
use crate::imap::Role;
use crate::outbox::{self, Op};
use crate::sanitize::{
    Sanitized, quote_start_html, quote_start_text, sanitize_html_inline, sanitize_html_with, text_to_html_counted,
};
use crate::settings::RemoteContent;
use crate::store::{Attachment, Mailbox, MessageDetail, Store, now};
use crate::{tr, trf};

/// Seconds a move or deletion waits for „Rückgängig“.
pub const UNDO_SECS: i64 = 5;

/// What [`undo`] needs to take an action back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Undo {
    pub outbox: Vec<i64>,
    pub hidden: Vec<i64>,
    /// The accounts whose workers should look at their queue.
    pub accounts: Vec<i64>,
}

fn b64(raw: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(raw)
}

/// The messages by folder.
fn by_mailbox(store: &Store, ids: &[i64]) -> Result<BTreeMap<i64, Vec<(i64, u32)>>> {
    let mut out: BTreeMap<i64, Vec<(i64, u32)>> = BTreeMap::new();
    for s in store.summaries(ids)? {
        out.entry(s.mailbox_id).or_default().push((s.id, s.uid));
    }
    Ok(out)
}

/// Marks messages read or unread, flagged or not; returns the accounts touched.
pub fn set_flags(store: &Store, ids: &[i64], seen: Option<bool>, flagged: Option<bool>) -> Result<Vec<i64>> {
    let mut accounts = vec![];
    for (mailbox_id, msgs) in by_mailbox(store, ids)? {
        let mb = store.mailbox(mailbox_id)?;
        for (id, _) in &msgs {
            if let Some(s) = seen {
                store.mark_seen(*id, s)?;
            }
            if let Some(f) = flagged {
                store.mark_flagged(*id, f)?;
            }
        }
        let uids = msgs.iter().map(|(_, u)| *u).collect();
        outbox::enqueue(store, mb.account_id, &Op::Flags { mailbox: mb.name, uids, seen, flagged })?;
        accounts.push(mb.account_id);
    }
    accounts.dedup();
    Ok(accounts)
}

/// Sets or takes off an IMAP keyword (a label, `$Rechnung`); here at once, on the server
/// through the queue. Returns the accounts touched.
pub fn set_keyword(store: &Store, ids: &[i64], keyword: &str, add: bool) -> Result<Vec<i64>> {
    let keyword = keyword.trim();
    let valid = !keyword.is_empty()
        && !keyword.starts_with('\\')
        && keyword.chars().all(|c| c.is_ascii_graphic() && !"(){%*\"\\]".contains(c));
    if !valid {
        return Err(Error::Invalid(trf!(
            "„{keyword}“ taugt nicht als Schlagwort: nur Buchstaben, Ziffern und $-_. ohne Leerzeichen.",
            "“{keyword}” does not work as a label: letters, digits and $-_. only, no spaces."
        )));
    }
    let mut accounts = vec![];
    for (mailbox_id, msgs) in by_mailbox(store, ids)? {
        let mb = store.mailbox(mailbox_id)?;
        for (id, _) in &msgs {
            let raw: String =
                store.conn().query_row("SELECT keywords FROM messages WHERE id = ?1", [id], |r| r.get(0))?;
            let mut list: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
            list.retain(|k| !k.eq_ignore_ascii_case(keyword));
            if add {
                list.push(keyword.to_string());
            }
            store.conn().execute(
                "UPDATE messages SET keywords = ?2 WHERE id = ?1",
                rusqlite::params![id, serde_json::to_string(&list)?],
            )?;
        }
        let uids = msgs.iter().map(|(_, u)| *u).collect();
        outbox::enqueue(store, mb.account_id, &Op::Keyword { mailbox: mb.name, uids, keyword: keyword.into(), add })?;
        accounts.push(mb.account_id);
    }
    accounts.dedup();
    Ok(accounts)
}

/// The account's folder of a role; created (here and, through the queue, on the server) under
/// `name` when the server has none.
fn folder_for(store: &Store, account_id: i64, role: Role, name: &str, undo: &mut Undo) -> Result<Mailbox> {
    if let Some(m) = store.mailbox_by_role(account_id, role)? {
        return Ok(m);
    }
    if let Some(m) = store.mailbox_by_name(account_id, name)? {
        return Ok(m);
    }
    let id = store.upsert_mailbox(
        account_id,
        &crate::imap::MailboxEntry { name: name.into(), delimiter: Some('/'), attributes: vec![] },
    )?;
    undo.outbox.push(outbox::enqueue(store, account_id, &Op::CreateFolder { name: name.into() })?);
    store.mailbox(id)
}

/// Moves messages to a folder of the same account (after the undo window).
pub fn move_messages(store: &Store, ids: &[i64], target: i64) -> Result<Undo> {
    let to = store.mailbox(target)?;
    let mut undo = Undo::default();
    move_into(store, ids, &to, &mut undo, UNDO_SECS)?;
    Ok(undo)
}

fn move_into(store: &Store, ids: &[i64], to: &Mailbox, undo: &mut Undo, delay: i64) -> Result<()> {
    for (mailbox_id, msgs) in by_mailbox(store, ids)? {
        let from = store.mailbox(mailbox_id)?;
        if from.account_id != to.account_id {
            return Err(Error::Invalid(
                tr!(
                    "Nachrichten lassen sich nur innerhalb eines Kontos verschieben.",
                    "Messages can only be moved within one account."
                )
                .into(),
            ));
        }
        if from.id == to.id {
            continue;
        }
        let local: Vec<i64> = msgs.iter().map(|(id, _)| *id).collect();
        store.set_hidden(&local, true)?;
        let op = Op::Move {
            from: from.name,
            to: to.name.clone(),
            uids: msgs.iter().map(|(_, u)| *u).collect(),
            ids: local.clone(),
        };
        undo.outbox.push(outbox::enqueue_at(store, to.account_id, &op, now() + delay)?);
        undo.hidden.extend(local);
        undo.accounts.push(to.account_id);
    }
    undo.accounts.dedup();
    Ok(())
}

/// Moves messages to a folder of the same account at once, without the undo window: what the
/// app does by itself (a snoozed message coming back).
pub fn move_now(store: &Store, ids: &[i64], target: i64) -> Result<Undo> {
    let to = store.mailbox(target)?;
    let mut undo = Undo::default();
    move_into(store, ids, &to, &mut undo, 0)?;
    Ok(undo)
}

/// Moves messages into the folder `name` of their accounts at once, creating it (here and,
/// through the queue, on the server) where it is missing: the Screener's „Screened Out“, the
/// snoozed messages' „Snoozed“.
pub fn move_to_named(store: &Store, ids: &[i64], name: &str) -> Result<Undo> {
    let mut undo = Undo::default();
    let mut per_account: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for (mailbox_id, msgs) in by_mailbox(store, ids)? {
        let account = store.mailbox(mailbox_id)?.account_id;
        per_account.entry(account).or_default().extend(msgs.iter().map(|(id, _)| *id));
    }
    for (account, list) in per_account {
        let target = named_folder(store, account, name, &mut undo)?;
        move_into(store, &list, &target, &mut undo, 0)?;
    }
    Ok(undo)
}

/// The account's folder `name`, created here and queued for the server when it is missing.
pub(crate) fn named_folder(store: &Store, account_id: i64, name: &str, undo: &mut Undo) -> Result<Mailbox> {
    if let Some(m) = store.mailbox_by_name(account_id, name)? {
        return Ok(m);
    }
    let id = store.upsert_mailbox(
        account_id,
        &crate::imap::MailboxEntry { name: name.into(), delimiter: Some('/'), attributes: vec![] },
    )?;
    undo.outbox.push(outbox::enqueue(store, account_id, &Op::CreateFolder { name: name.into() })?);
    store.mailbox(id)
}

/// Moves messages to the archive of their account.
pub fn archive(store: &Store, ids: &[i64]) -> Result<Undo> {
    to_role(store, ids, Role::Archive, "Archive")
}

/// Moves messages to spam.
pub fn mark_spam(store: &Store, ids: &[i64]) -> Result<Undo> {
    to_role(store, ids, Role::Junk, "Junk")
}

fn to_role(store: &Store, ids: &[i64], role: Role, name: &str) -> Result<Undo> {
    let mut undo = Undo::default();
    let mut per_account: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for (mailbox_id, msgs) in by_mailbox(store, ids)? {
        let account = store.mailbox(mailbox_id)?.account_id;
        per_account.entry(account).or_default().extend(msgs.iter().map(|(id, _)| *id));
    }
    for (account, list) in per_account {
        // Gmail has no archive folder: archiving takes the message out of the inbox, into
        // All Mail.
        let all = match role {
            Role::Archive if store.mailbox_by_role(account, Role::Archive)?.is_none() => {
                store.mailbox_by_role(account, Role::All)?
            }
            _ => None,
        };
        let target = match all {
            Some(all) => all,
            None => folder_for(store, account, role, name, &mut undo)?,
        };
        move_into(store, &list, &target, &mut undo, UNDO_SECS)?;
    }
    Ok(undo)
}

/// Deletes messages: to the trash, or for good when they are in the trash or spam already
/// (or the account has no trash).
pub fn delete(store: &Store, ids: &[i64]) -> Result<Undo> {
    let mut undo = Undo::default();
    for (mailbox_id, msgs) in by_mailbox(store, ids)? {
        let from = store.mailbox(mailbox_id)?;
        let trash = store.mailbox_by_role(from.account_id, Role::Trash)?;
        let local: Vec<i64> = msgs.iter().map(|(id, _)| *id).collect();
        match trash {
            Some(t) if !matches!(from.role, Some(Role::Trash | Role::Junk)) => {
                move_into(store, &local, &t, &mut undo, UNDO_SECS)?
            }
            _ => {
                store.set_hidden(&local, true)?;
                let op =
                    Op::Delete { mailbox: from.name, uids: msgs.iter().map(|(_, u)| *u).collect(), ids: local.clone() };
                undo.outbox.push(outbox::enqueue_at(store, from.account_id, &op, now() + UNDO_SECS)?);
                undo.hidden.extend(local);
                undo.accounts.push(from.account_id);
            }
        }
    }
    undo.accounts.dedup();
    Ok(undo)
}

/// Takes an action back while its steps have not started; `false` when it is too late (the
/// server has it already).
pub fn undo(store: &Store, u: &Undo) -> Result<bool> {
    let mut all = true;
    for id in u.outbox.iter().rev() {
        all &= outbox::cancel(store, *id)?.is_some();
    }
    if all {
        store.set_hidden(&u.hidden, false)?;
    }
    Ok(all)
}

// ---------------------------------------------------------------------------- folders

/// A new folder (below `parent`, if given).
pub fn create_folder(store: &Store, account_id: i64, parent: Option<i64>, name: &str) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err(Error::Invalid(trf!(
            "„{name}“ ist kein gültiger Ordnername.",
            "“{name}” is not a valid folder name."
        )));
    }
    let (full, delimiter) = match parent {
        Some(p) => {
            let p = store.mailbox(p)?;
            let d = p.delimiter.clone().unwrap_or_else(|| "/".into());
            (format!("{}{d}{name}", p.name), d)
        }
        None => (name.to_string(), "/".into()),
    };
    if store.mailbox_by_name(account_id, &full)?.is_some() {
        return Err(Error::Invalid(trf!("Den Ordner „{name}“ gibt es schon.", "The folder “{name}” exists already.")));
    }
    let id = store.upsert_mailbox(
        account_id,
        &crate::imap::MailboxEntry { name: full.clone(), delimiter: delimiter.chars().next(), attributes: vec![] },
    )?;
    outbox::enqueue(store, account_id, &Op::CreateFolder { name: full })?;
    Ok(id)
}

/// Renames a folder (its last part); special folders keep their names.
pub fn rename_folder(store: &Store, mailbox_id: i64, new_name: &str) -> Result<()> {
    let m = store.mailbox(mailbox_id)?;
    let new_name = new_name.trim();
    if m.role.is_some() {
        return Err(Error::Invalid(
            tr!("Besondere Ordner lassen sich nicht umbenennen.", "Special folders cannot be renamed.").into(),
        ));
    }
    if new_name.is_empty() || new_name.contains(['/', '\\']) {
        return Err(Error::Invalid(trf!(
            "„{new_name}“ ist kein gültiger Ordnername.",
            "“{new_name}” is not a valid folder name."
        )));
    }
    let to = match m.delimiter.as_deref().and_then(|d| m.name.rsplit_once(d).map(|(p, _)| format!("{p}{d}{new_name}")))
    {
        Some(t) => t,
        None => new_name.to_string(),
    };
    if store.mailbox_by_name(m.account_id, &to)?.is_some() {
        return Err(Error::Invalid(trf!(
            "Den Ordner „{new_name}“ gibt es schon.",
            "The folder “{new_name}” exists already."
        )));
    }
    store.rename_mailbox(m.account_id, &m.name, &to)?;
    outbox::enqueue(store, m.account_id, &Op::RenameFolder { from: m.name, to })?;
    Ok(())
}

/// Deletes a folder with its messages (special folders stay).
pub fn delete_folder(store: &Store, mailbox_id: i64) -> Result<()> {
    let m = store.mailbox(mailbox_id)?;
    if m.role.is_some() {
        return Err(Error::Invalid(
            tr!("Besondere Ordner lassen sich nicht löschen.", "Special folders cannot be deleted.").into(),
        ));
    }
    store.remove_mailbox(m.id)?;
    outbox::enqueue(store, m.account_id, &Op::DeleteFolder { name: m.name })?;
    Ok(())
}

// ---------------------------------------------------------------------------- sending

/// The draft with its HTML cleaned like a received message (pasted content can carry
/// anything).
fn cleaned(d: &Draft) -> Draft {
    Draft { html: d.html.as_deref().map(|h| sanitize_html_with(h, true, false).html), ..d.clone() }
}

/// Queues a message to send after `delay_secs` (the undo window); returns the queue entry and
/// the message's Message-ID.
pub fn queue_send(store: &Store, d: &Draft, files: &[FileAttachment], delay_secs: u32) -> Result<(i64, String)> {
    let date = now();
    let (op, mid) = prepare_send(store, d, files, date)?;
    let id = outbox::enqueue_at(store, d.account_id, &op, now() + delay_secs as i64)?;
    if let Some(r) = d.reply_to_id {
        store.mark_answered(r)?;
    }
    Ok((id, mid))
}

/// The finished message of a draft as the outbox step that sends it, dated `date`, and its
/// Message-ID; its recipients go to the address book. A draft that asks for it is signed and/or
/// encrypted ([`crate::crypto::protect`], with the secrets the shell registered), the copy for
/// Sent too; mail from an address with an own OpenPGP key carries its `Autocrypt` header.
pub fn prepare_send(store: &Store, d: &Draft, files: &[FileAttachment], date: i64) -> Result<(Op, String)> {
    prepare_send_with(store, d, files, date, crate::crypto::registered().as_ref())
}

/// [`prepare_send`] with the secrets in `keys`. An encrypted message goes to To and Cc encrypted
/// for them (and the sender) only; every Bcc recipient gets a copy of its own, encrypted for
/// that recipient (and the sender), so no one learns of a Bcc recipient from the keys a message
/// is encrypted to.
pub fn prepare_send_with(
    store: &Store,
    d: &Draft,
    files: &[FileAttachment],
    date: i64,
    keys: &dyn crate::crypto::Keys,
) -> Result<(Op, String)> {
    let d = &cleaned(d);
    let account = store.account(d.account_id)?;
    let rcpt = compose::recipients(d)?;
    let blind: Vec<String> = d
        .bcc
        .iter()
        .filter(|b| !b.trim().is_empty())
        .map(|b| compose::parse_address(b).map(|a| a.email))
        .collect::<Result<_>>()?;
    let encrypt = d.protect.is_some_and(|p| p.encrypt);
    let visible: Vec<String> = rcpt
        .iter()
        .map(|a| a.email.clone())
        .filter(|e| !(encrypt && blind.iter().any(|b| b.eq_ignore_ascii_case(e))))
        .collect();
    let mid = d.message_id.clone().unwrap_or_else(|| compose::new_message_id(&account.email));
    let autocrypt = if crate::settings::load(store)?.autocrypt {
        crate::crypto::autocrypt_for(store, &account.email)?
    } else {
        None
    };
    let finish = |raw: Vec<u8>, to: &[String]| -> Result<Vec<u8>> {
        let raw = match &autocrypt {
            Some(h) => crate::crypto::add_header(&raw, "Autocrypt", h),
            None => raw,
        };
        match &d.protect {
            Some(how) => crate::crypto::protect(store, &account.email, &raw, to, how, keys),
            None => Ok(raw),
        }
    };
    let built = compose::build(&account, d, files, &mid, date, false)?;
    let all: Vec<String> = rcpt.iter().map(|a| a.email.clone()).collect();
    let (raw, copies) = if encrypt && !blind.is_empty() {
        let copies = blind
            .iter()
            .map(|b| {
                Ok(crate::outbox::Copy {
                    raw_b64: b64(&finish(built.clone(), std::slice::from_ref(b))?),
                    recipients: vec![b.clone()],
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // Nobody in To or Cc: the copies are all there is to send.
        let raw = if visible.is_empty() { None } else { Some(finish(built, &visible)?) };
        (raw, copies)
    } else {
        (Some(finish(built, &all)?), vec![])
    };
    let with_bcc = !blind.is_empty();
    let sent =
        if with_bcc { Some(b64(&finish(compose::build(&account, d, files, &mid, date, true)?, &all)?)) } else { None };
    let sent_mailbox = store.mailbox_by_role(account.id, Role::Sent)?.map(|m| m.name);
    let answered = match d.reply_to_id {
        Some(id) => store.summaries(&[id])?.first().map(|s| (s.mailbox_id, s.uid)),
        None => None,
    };
    let answered = match answered {
        Some((mb, uid)) => Some((store.mailbox(mb)?.name, uid)),
        None => None,
    };
    let (raw_b64, recipients, copies) = match raw {
        Some(r) => (b64(&r), if encrypt { visible } else { all }, copies),
        None => {
            let mut copies = copies;
            let first = copies.remove(0);
            (first.raw_b64, first.recipients, copies)
        }
    };
    let op = Op::Send {
        raw_b64,
        from: account.email.clone(),
        recipients,
        sent_b64: sent,
        sent_mailbox,
        draft: d.message_id.clone(),
        answered,
        copies,
    };
    for a in &rcpt {
        store.touch_contact(a, true, now())?;
    }
    Ok((op, mid))
}

/// What a button of a new-mail notification does to the message: `archive` or `read` (marked
/// read). Returns the accounts whose worker should look at its queue.
pub fn quick_action(store: &Store, id: i64, action: &str) -> Result<Vec<i64>> {
    match action {
        "archive" => Ok(archive(store, &[id])?.accounts),
        "read" => set_flags(store, &[id], Some(true), None),
        other => Err(Error::Invalid(trf!("Unbekannte Aktion „{other}“.", "Unknown action “{other}”."))),
    }
}

/// Queues a finished message (an answer to an invitation, a meeting's update): sent at once,
/// filed in Sent as written.
pub fn queue_raw(store: &Store, account_id: i64, raw: &[u8], recipients: &[String]) -> Result<i64> {
    let account = store.account(account_id)?;
    let sent_mailbox = store.mailbox_by_role(account.id, Role::Sent)?.map(|m| m.name);
    let op = Op::Send {
        raw_b64: b64(raw),
        from: account.email.clone(),
        recipients: recipients.to_vec(),
        sent_b64: None,
        sent_mailbox,
        draft: None,
        answered: None,
        copies: vec![],
    };
    outbox::enqueue(store, account_id, &op)
}

/// Takes a queued message back (undo send); `false` when it went out already.
pub fn cancel_send(store: &Store, outbox_id: i64) -> Result<bool> {
    Ok(outbox::cancel(store, outbox_id)?.is_some())
}

/// Saves a draft in the account's Drafts folder (replacing its earlier copy); returns its
/// Message-ID, which the next save keeps.
pub fn queue_draft(store: &Store, d: &Draft, files: &[FileAttachment]) -> Result<String> {
    let d = &cleaned(d);
    let account = store.account(d.account_id)?;
    let mid = d.message_id.clone().unwrap_or_else(|| compose::new_message_id(&account.email));
    let raw = compose::build(&account, d, files, &mid, now(), true)?;
    let mut undo = Undo::default();
    let drafts = folder_for(store, account.id, Role::Drafts, "Drafts", &mut undo)?;
    outbox::enqueue(
        store,
        account.id,
        &Op::SaveDraft { mailbox: drafts.name, raw_b64: b64(&raw), message_id: mid.clone() },
    )?;
    Ok(mid)
}

/// Removes a draft from Drafts (the user discarded it).
pub fn discard_draft(store: &Store, account_id: i64, message_id: &str) -> Result<()> {
    if let Some(drafts) = store.mailbox_by_role(account_id, Role::Drafts)? {
        outbox::enqueue(store, account_id, &Op::DeleteDraft { mailbox: drafts.name, message_id: message_id.into() })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------- reading

/// The body as the reading pane shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Body {
    /// Sanitized, the quote folded away when there is one.
    pub html: String,
    /// The whole message, when [`Body::html`] left a quote out.
    pub full_html: Option<String>,
    /// The text part as HTML, for „Als Text anzeigen“ when the message has HTML.
    pub text_html: Option<String>,
    pub blocked_remote: usize,
    /// Tracking pixels and link trackers removed („Tracker entfernt“).
    pub trackers: usize,
    /// Made from plain text: drawn in the app's colors, not on the light paper of HTML mail.
    pub plain: bool,
}

/// A file inside a protected message (opened again from the message when saved).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProtectedFile {
    pub index: u32,
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: u64,
}

/// An online meeting's link in a message („Beitreten“).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeetingLink {
    pub url: String,
    /// „Teams“, „Zoom“, „Webex“, „Google Meet“, „Jitsi“, „Online“.
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessageView {
    #[serde(flatten)]
    pub detail: MessageDetail,
    pub body: Body,
    /// Remote images load for this sender.
    pub sender_trusted: bool,
    /// Attachments the user sees (not the pictures the HTML shows).
    pub files: Vec<Attachment>,
    /// Pinned, read later, snoozed, a reminder.
    pub marks: crate::later::Marks,
    /// When the user unsubscribed from the sender (the button gives way to a note).
    pub unsubscribed: Option<i64>,
    /// OpenPGP or S/MIME: encrypted, signed, by whom, whether it could be opened.
    pub protection: Option<crate::crypto::Protection>,
    /// The files inside an encrypted message.
    pub protected_files: Vec<ProtectedFile>,
    /// A meeting invitation (or its update, cancellation, answer) in the message.
    pub invitation: Option<crate::itip::Card>,
    /// A Teams, Zoom, Webex, Google Meet or Jitsi link in the message.
    pub meeting: Option<MeetingLink>,
}

/// The wrappers of signed and encrypted mail, never shown as files.
fn is_wrapper(a: &Attachment) -> bool {
    matches!(
        a.mime_type.as_str(),
        "application/pgp-signature"
            | "application/pgp-encrypted"
            | "application/pkcs7-signature"
            | "application/x-pkcs7-signature"
            | "application/pkcs7-mime"
            | "application/x-pkcs7-mime"
    ) || a
        .filename
        .as_deref()
        .is_some_and(|f| matches!(f, "encrypted.asc" | "smime.p7m" | "smime.p7s" | "signature.asc"))
}

/// The message with its body ready for the sandboxed frame, opened with the secrets the shell
/// registered ([`crate::crypto::register`]).
pub fn message_view(store: &Store, id: i64, remote: RemoteContent) -> Result<MessageView> {
    message_view_with(store, id, remote, crate::crypto::registered().as_ref())
}

/// The message with its body ready for the sandboxed frame. A signed or encrypted message is
/// opened with `keys`; what it shows then comes from inside it, and its remote content never
/// loads.
pub fn message_view_with(
    store: &Store,
    id: i64,
    remote: RemoteContent,
    keys: &dyn crate::crypto::Keys,
) -> Result<MessageView> {
    let mut detail = store.message(id)?;
    let sender = detail.summary.from.as_ref().map(|a| a.email.clone()).unwrap_or_default();
    let sender_trusted = !sender.is_empty() && store.sender_allowed(&sender)?;
    let mut allow = detail.remote_allowed || sender_trusted || remote == RemoteContent::Always;
    let raw = store.raw_message(id)?;
    let mut protection = None;
    let mut protected_files = vec![];
    let mut invitation = None;
    if let Some(raw) = &raw {
        if !sender.is_empty() {
            let _ = crate::crypto::ingest_autocrypt(store, raw, &sender);
        }
        let from = (!sender.is_empty()).then_some(sender.as_str());
        if let Some(opened) = crate::crypto::open(store, raw, from, keys) {
            let inner = opened.entity.as_deref().and_then(|e| crate::parse::parse(e).ok());
            if opened.protection.encrypted {
                // Opened here only: nothing of it leaves the backend but what is shown.
                allow = false;
                detail.text = inner.as_ref().and_then(|p| p.text.clone());
                detail.html = inner.as_ref().and_then(|p| p.html.clone());
                protected_files = inner
                    .as_ref()
                    .map(|p| {
                        p.attachments
                            .iter()
                            .filter(|a| !(a.inline && a.content_id.is_some()))
                            .map(|a| ProtectedFile {
                                index: a.index,
                                filename: a.filename.as_deref().map(crate::files::display_name),
                                mime_type: a.mime_type.clone(),
                                size: a.size,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            protection = Some(opened.protection);
        } else if let Some((text, p)) =
            detail.text.as_deref().and_then(|t| crate::crypto::open_inline(store, t, from, keys))
        {
            if p.encrypted {
                allow = false;
                detail.html = None;
            }
            if p.decrypted || p.signed {
                // What the protection speaks for is the text: an HTML part next to it (which
                // nothing signed) is not shown.
                detail.html = None;
                detail.text = Some(text);
            }
            protection = Some(p);
        }
        if let Some(data) = crate::itip::find_in_message(raw) {
            invitation = crate::itip::card(store, id, &data)?;
        }
    } else if let Some(data) = crate::itip::calendar_data(store, id)? {
        // A large message: its calendar part came with the text.
        invitation = crate::itip::card(store, id, &data)?;
    }
    let images =
        if detail.html.as_deref().is_some_and(|h| h.contains("cid:")) { store.inline_images(id)? } else { vec![] };
    let clean = |html: &str| -> Sanitized { sanitize_html_inline(html, allow, &images) };
    let body = match (&detail.html, &detail.text) {
        (Some(html), text) => {
            let full = clean(html);
            let (main, full_html) = match quote_start_html(html) {
                Some(at) => (clean(&html[..at]).html, Some(full.html.clone())),
                None => (full.html.clone(), None),
            };
            Body {
                html: main,
                full_html,
                text_html: text.as_deref().filter(|t| !t.trim().is_empty()).map(|t| text_to_html_counted(t).0),
                blocked_remote: full.blocked_remote,
                trackers: full.trackers,
                plain: false,
            }
        }
        (None, Some(text)) => {
            let (full, trackers) = text_to_html_counted(text);
            match quote_start_text(text) {
                Some(at) => Body {
                    html: text_to_html_counted(text[..at].trim_end()).0,
                    full_html: Some(full),
                    text_html: None,
                    blocked_remote: 0,
                    trackers,
                    plain: true,
                },
                None => Body { html: full, full_html: None, text_html: None, blocked_remote: 0, trackers, plain: true },
            }
        }
        (None, None) => {
            let html = text_to_html_counted(&detail.summary.snippet).0;
            Body { html, full_html: None, text_html: None, blocked_remote: 0, trackers: 0, plain: true }
        }
    };
    let encrypted = protection.as_ref().is_some_and(|p| p.encrypted);
    let files = detail
        .attachments
        .iter()
        .filter(|a| !(a.inline && a.content_id.is_some() && a.mime_type.starts_with("image/") && a.filename.is_none()))
        .filter(|a| !(a.content_id.is_some() && a.part_index >= 10_000))
        .filter(|a| !encrypted && !is_wrapper(a))
        // The calendar part of an invitation shows as its card.
        .filter(|a| invitation.is_none() || !(a.mime_type == "text/calendar" || a.mime_type == "application/ics"))
        // Shown without bidirectional controls, so the real extension is what the user reads.
        .map(|a| Attachment { filename: a.filename.as_deref().map(crate::files::display_name), ..a.clone() })
        .collect();
    let meeting = invitation
        .as_ref()
        .and_then(|c| c.invitation.link.clone())
        .or_else(|| {
            crate::calendar::meeting_link([detail.text.as_deref().unwrap_or(""), detail.html.as_deref().unwrap_or("")])
        })
        .map(|url| MeetingLink { kind: crate::calendar::meeting_kind(&url).to_string(), url });
    // The raw HTML stays in the backend.
    detail.html = None;
    detail.remote_allowed = allow;
    let marks = crate::later::marks_of(store, id)?;
    let unsubscribed = crate::unsubscribe::unsubscribed_at(store, &sender)?;
    Ok(MessageView {
        detail,
        body,
        sender_trusted,
        files,
        marks,
        unsubscribed,
        protection,
        protected_files,
        invitation,
        meeting,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{AccountDraft, Provider, preset};
    use crate::imap::MailboxEntry;
    use crate::parse::{parse, tests::sample};
    use crate::store::tests::header;

    fn setup() -> (Store, i64, i64, Vec<i64>) {
        let s = Store::open_in_memory().unwrap();
        let account = s
            .add_account(&AccountDraft {
                provider: Provider::Imap,
                name: String::new(),
                email: "maria@example.org".into(),
                sender_name: "Maria".into(),
                transport: preset(Provider::Imap, "maria@example.org").unwrap(),
            })
            .unwrap();
        let mut boxes = vec![];
        for (name, attr) in [("INBOX", ""), ("Trash", "\\Trash"), ("Sent", "\\Sent")] {
            let attributes = if attr.is_empty() { vec![] } else { vec![attr.to_string()] };
            boxes.push(
                s.upsert_mailbox(account, &MailboxEntry { name: name.into(), delimiter: Some('/'), attributes })
                    .unwrap(),
            );
        }
        let ids = (1..=2)
            .map(|uid| {
                let raw = sample(&format!("m{uid}@x"), &format!("Nachricht {uid}"), None, "Text");
                s.insert_message(boxes[0], &header(uid, &raw), &parse(&raw).unwrap()).unwrap()
            })
            .collect();
        (s, account, boxes[0], ids)
    }

    #[test]
    fn archive_creates_the_folder_and_undo_takes_it_back() {
        let (s, account, inbox, ids) = setup();
        let u = archive(&s, &ids[..1]).unwrap();
        assert_eq!(s.messages(inbox, 10, 0).unwrap().len(), 1, "hidden at once");
        assert!(s.mailbox_by_role(account, Role::Archive).unwrap().is_some(), "created here");
        let kinds: Vec<String> = outbox::entries(&s).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["create_folder", "move"]);
        assert!(undo(&s, &u).unwrap());
        assert_eq!(s.messages(inbox, 10, 0).unwrap().len(), 2);
        assert!(outbox::entries(&s).unwrap().is_empty());
    }

    #[test]
    fn notification_buttons_archive_and_mark_read() {
        let (s, account, inbox, ids) = setup();
        assert_eq!(quick_action(&s, ids[0], "read").unwrap(), [account]);
        assert_eq!(s.mailbox(inbox).unwrap().unread, 1);
        assert_eq!(quick_action(&s, ids[1], "archive").unwrap(), [account]);
        assert_eq!(s.messages(inbox, 10, 0).unwrap().len(), 1, "archived at once");
        assert!(quick_action(&s, ids[0], "explode").is_err());
    }

    #[test]
    fn delete_goes_to_the_trash_and_from_there_for_good() {
        let (s, account, _, ids) = setup();
        delete(&s, &ids[..1]).unwrap();
        let e = outbox::entries(&s).unwrap();
        assert_eq!(e[0].kind, "move");
        let trash = s.mailbox_by_role(account, Role::Trash).unwrap().unwrap();
        let raw = sample("t@x", "Weg", None, "x");
        let t = s.insert_message(trash.id, &header(9, &raw), &parse(&raw).unwrap()).unwrap();
        delete(&s, &[t]).unwrap();
        assert_eq!(outbox::entries(&s).unwrap()[1].kind, "delete");
        assert!(s.messages(trash.id, 10, 0).unwrap().is_empty());
    }

    #[test]
    fn flags_and_folders() {
        let (s, account, inbox, ids) = setup();
        set_flags(&s, &ids, Some(true), Some(true)).unwrap();
        assert_eq!(s.mailbox(inbox).unwrap().unread, 0);
        assert!(s.messages(inbox, 10, 0).unwrap().iter().all(|m| m.flagged));
        let p = create_folder(&s, account, None, "Projekte").unwrap();
        let w = create_folder(&s, account, Some(p), "Website").unwrap();
        assert_eq!(s.mailbox(w).unwrap().name, "Projekte/Website");
        assert!(create_folder(&s, account, None, "Projekte").is_err());
        rename_folder(&s, p, "Kunden").unwrap();
        assert_eq!(s.mailbox(w).unwrap().name, "Kunden/Website");
        assert!(rename_folder(&s, inbox, "Eingang").is_err(), "special folders keep their names");
        delete_folder(&s, w).unwrap();
        let kinds: Vec<String> = outbox::entries(&s).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["flags", "create_folder", "create_folder", "rename_folder", "delete_folder"]);
    }

    #[test]
    fn sending_waits_for_undo_and_drafts_get_their_folder() {
        let (s, account, inbox, ids) = setup();
        let d = Draft {
            account_id: account,
            to: vec!["Jonas <jonas@example.org>".into()],
            bcc: vec!["chef@example.org".into()],
            subject: "Re: Nachricht 1".into(),
            text: "Passt.".into(),
            reply_to_id: Some(ids[0]),
            ..Draft::default()
        };
        let (id, mid) = queue_send(&s, &d, &[], 5).unwrap();
        assert!(mid.ends_with("@example.org"));
        assert_eq!(outbox::next_due(&s, account, now()).unwrap(), None, "the undo window");
        let q = outbox::next_due(&s, account, now() + 5).unwrap().unwrap();
        let Op::Send { recipients, sent_b64, sent_mailbox, answered, .. } = q.op else { panic!() };
        assert_eq!(recipients, ["jonas@example.org", "chef@example.org"]);
        assert!(sent_b64.is_some(), "a copy with Bcc for Sent");
        assert_eq!(sent_mailbox.as_deref(), Some("Sent"));
        assert_eq!(answered, Some(("INBOX".into(), 1)));
        assert!(s.messages(inbox, 10, 0).unwrap().iter().any(|m| m.answered));
        assert!(cancel_send(&s, id).unwrap());
        assert!(!cancel_send(&s, id).unwrap());
        assert!(queue_send(&s, &Draft { account_id: account, ..Draft::default() }, &[], 0).is_err(), "no recipient");
        let mid = queue_draft(&s, &Draft { account_id: account, subject: "Entwurf".into(), ..Draft::default() }, &[])
            .unwrap();
        let kinds: Vec<String> = outbox::entries(&s).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["create_folder", "save_draft"]);
        discard_draft(&s, account, &mid).unwrap();
    }

    #[test]
    fn the_reading_view_folds_quotes_and_decides_remote_images() {
        let (s, _, inbox, _) = setup();
        let raw = "From: News <news@example.org>\r\nTo: maria@example.org\r\nSubject: Neu\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"A\"\r\n\r\n--A\r\nContent-Type: text/plain\r\n\r\nNeu da.\r\n\
--A\r\nContent-Type: text/html\r\n\r\n<p>Neu <img src=\"https://t.example.org/p.gif\"></p><div class=\"gmail_quote\">alt</div>\r\n--A--\r\n";
        let id = s.insert_message(inbox, &header(5, raw.as_bytes()), &parse(raw.as_bytes()).unwrap()).unwrap();
        s.set_body(id, &parse(raw.as_bytes()).unwrap()).unwrap();
        let v = message_view(&s, id, RemoteContent::Ask).unwrap();
        assert_eq!(v.body.blocked_remote, 1);
        assert!(!v.body.html.contains("alt") && v.body.full_html.as_deref().unwrap().contains("alt"));
        assert!(v.body.text_html.as_deref().unwrap().contains("Neu da."));
        assert!(v.detail.html.is_none(), "the raw HTML stays in the backend");
        s.allow_sender("news@example.org").unwrap();
        let v = message_view(&s, id, RemoteContent::Ask).unwrap();
        assert!(v.sender_trusted && v.body.html.contains("t.example.org"));
        s.forbid_sender("news@example.org").unwrap();
        assert!(message_view(&s, id, RemoteContent::Always).unwrap().body.html.contains("t.example.org"));
    }
}
