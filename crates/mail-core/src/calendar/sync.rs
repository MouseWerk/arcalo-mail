//! One round of a calendar source: the changes made here go to the server first (each with the
//! ETag it was based on; a refusal of that ETag becomes a conflict), then what changed there
//! comes in. The store is locked only between the server's answers, so the app stays usable
//! while a round waits.

use std::sync::Mutex;

use chrono::Utc;

use super::{SourceKind, Write, caldav, graph, jmap};
use crate::error::{Error, Result};
use crate::store::Store;

/// How a round reaches the source's server.
pub enum Access {
    /// This device: nothing to reach.
    None,
    /// CalDAV with the user name and password (or app password).
    Basic { user: String, password: String },
    /// A Microsoft account's Graph with an access token.
    Graph(crate::graph::Graph),
    /// A JMAP account's session.
    Jmap(Box<crate::jmap::JmapClient>),
    /// An ICS subscription's address (from the credential store).
    Url(String),
}

/// What a round did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Something in the calendars changed (the view reloads).
    pub changed: bool,
    /// Meetings the user organizes whose update was confirmed and that the server does not send
    /// itself: the shell sends their invitations (iTIP `REQUEST`) by mail.
    pub notify: Vec<String>,
}

fn lock(store: &Mutex<Store>) -> std::sync::MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|e| e.into_inner())
}

/// One round of a source. A write that fails stays queued and the round goes on: the other
/// writes and the server's changes are never held up by it. When the server answered in that
/// round, the failure counts towards parking the write ([`super::write_struck`]).
pub async fn sync_source(store: &Mutex<Store>, source_id: i64, access: &Access) -> Result<Outcome> {
    let source = super::source(&lock(store), source_id)?;
    let result = async {
        let mut out = Outcome::default();
        let writes = super::pending_writes(&lock(store), source_id)?;
        let auto_schedule = source.auto_schedule;
        let mut failed = vec![];
        let mut first_error = None;
        for w in writes {
            match replay(store, &source.kind, access, &w).await {
                Ok(Replayed::Done) => {
                    out.changed = true;
                    if w.send && w.op != "delete" && source.kind == SourceKind::Caldav && !auto_schedule {
                        out.notify.push(w.object.data.clone());
                    }
                }
                // Refused because of a change made elsewhere: nobody hears of it before the
                // user settled the conflict.
                Ok(Replayed::Conflict) => out.changed = true,
                Err(Error::Invalid(e)) => {
                    super::write_refused(&lock(store), &w, &e)?;
                    out.changed = true;
                }
                Err(e) => {
                    super::write_failed(&lock(store), &w, &super::describe_error(&e))?;
                    out.changed = true;
                    failed.push(w);
                    // Signed out: the rest would fail the same way.
                    let stop = matches!(e, Error::Connection(_));
                    first_error.get_or_insert(e);
                    if stop {
                        break;
                    }
                }
            }
        }
        match pull(store, source_id, &source.kind, access).await {
            Ok(changed) => out.changed |= changed,
            Err(e) => return Err(first_error.unwrap_or(e)),
        }
        let s = lock(store);
        for w in &failed {
            super::write_struck(&s, w)?;
        }
        Ok(out)
    }
    .await;
    match &result {
        Ok(_) => {}
        Err(e) => super::synced(&lock(store), source_id, Some(&super::describe_error(e)), None)?,
    }
    result
}

/// The calendar's remote id of a write's object.
fn remote_of(store: &Mutex<Store>, w: &Write) -> Result<String> {
    Ok(super::calendar(&lock(store), w.object.calendar_id)?.remote_id)
}

/// What became of a write.
enum Replayed {
    Done,
    /// Changed elsewhere since: the user decides.
    Conflict,
}

async fn replay(store: &Mutex<Store>, kind: &SourceKind, access: &Access, w: &Write) -> Result<Replayed> {
    let done = |r: Result<()>| r.map(|_| Replayed::Done);
    let conflict = |r: Result<()>| r.map(|_| Replayed::Conflict);
    match (kind, access) {
        (SourceKind::Caldav, Access::Basic { user, password }) => {
            let dav = caldav::CalDav::new(user, password)?;
            let href = &w.object.href;
            let etag = w.object.etag.as_deref();
            let written = if w.op == "delete" {
                dav.delete(href, etag).await?
            } else {
                dav.put(href, &w.object.data, if w.op == "create" { None } else { etag }).await?
            };
            match written {
                caldav::Written::Done(new_etag) => {
                    // Without an ETag the server changed the object: read it as it is now.
                    let (etag, data) = match (&new_etag, w.op.as_str()) {
                        (None, op) if op != "delete" => match dav.get(href).await? {
                            Some((e, d)) => (e, Some(d)),
                            None => (None, None),
                        },
                        _ => (new_etag, None),
                    };
                    done(super::write_done(&lock(store), w, None, etag.as_deref(), data.as_deref()))
                }
                caldav::Written::Conflict => {
                    let server = dav.get(href).await?;
                    let s = lock(store);
                    conflict(match server {
                        Some((e, d)) => super::write_conflict(&s, w, Some(&d), e.as_deref()),
                        None => super::write_conflict(&s, w, None, None),
                    })
                }
            }
        }
        (SourceKind::Graph, Access::Graph(g)) => {
            let calendar = remote_of(store, w)?;
            let written = match w.op.as_str() {
                "create" => graph::Written::Done(Some(graph::create(g, &calendar, &w.object.data).await?)),
                "delete" => graph::delete(g, &w.object.href, w.object.etag.as_deref()).await?,
                _ => graph::update(g, &w.object.href, &w.object.data, w.object.etag.as_deref()).await?,
            };
            match written {
                graph::Written::Done(Some(v)) => {
                    let id = v["id"].as_str().map(str::to_owned);
                    done(super::write_done(
                        &lock(store),
                        w,
                        id.as_deref(),
                        graph::etag_of(&v).as_deref(),
                        Some(&v.to_string()),
                    ))
                }
                graph::Written::Done(None) => done(super::write_done(&lock(store), w, None, None, None)),
                graph::Written::Conflict => {
                    let server = graph::get(g, &w.object.href).await?;
                    let s = lock(store);
                    conflict(match server {
                        Some(v) => super::write_conflict(&s, w, Some(&v.to_string()), graph::etag_of(&v).as_deref()),
                        None => super::write_conflict(&s, w, None, None),
                    })
                }
            }
        }
        (SourceKind::Jmap, Access::Jmap(client)) => {
            let calendar = remote_of(store, w)?;
            // The server sends a confirmed change to the attendees itself.
            let me = super::my_addresses(&lock(store))?;
            match w.op.as_str() {
                "create" => {
                    let id = jmap::create(client, &calendar, &w.object.data, w.send).await?;
                    done(super::write_done(&lock(store), w, Some(&id), None, None))
                }
                "delete" => {
                    jmap::destroy(client, &w.object.href).await?;
                    done(super::write_done(&lock(store), w, None, None, None))
                }
                _ => {
                    jmap::update(client, &w.object.href, &w.object.data, w.send, &me).await?;
                    done(super::write_done(&lock(store), w, None, None, None))
                }
            }
        }
        _ => Err(Error::Invalid(
            crate::tr!("Dieser Kalender nimmt keine Änderungen an.", "This calendar takes no changes.").into(),
        )),
    }
}

/// Reads what changed on the server; `true` when something did.
async fn pull(store: &Mutex<Store>, source_id: i64, kind: &SourceKind, access: &Access) -> Result<bool> {
    let mut changed = false;
    match (kind, access) {
        (SourceKind::Local, _) => {}
        (SourceKind::Caldav, Access::Basic { user, password }) => {
            let start = super::source(&lock(store), source_id)?.url;
            let dav = caldav::CalDav::new(user, password)?;
            let found = dav.discover(&start).await?;
            let mut ids = vec![];
            for rc in &found.calendars {
                let cal_id = super::upsert_calendar(
                    &lock(store),
                    source_id,
                    &rc.url,
                    &rc.name,
                    rc.color.as_deref(),
                    rc.writable,
                )?;
                ids.push(rc.url.clone());
                let (state, ctag) = super::sync_state(&lock(store), cal_id)?;
                if state.is_some()
                    && (state == rc.sync_token || (rc.sync_token.is_none() && ctag.is_some() && ctag == rc.ctag))
                {
                    continue;
                }
                let delta = match state.as_deref() {
                    Some(t) if rc.sync_token.is_some() => dav.changes(&rc.url, t).await?,
                    _ => None,
                };
                let (wanted, removed, token) = match delta {
                    Some(c) => (c.changed, c.removed, c.token.or(rc.sync_token.clone())),
                    None => {
                        let tags = dav.etags(&rc.url).await?;
                        let known = super::known_etags(&lock(store), cal_id)?;
                        let wanted: Vec<String> = tags
                            .iter()
                            .filter(|(h, e)| !known.iter().any(|(kh, ke)| kh == h && ke.as_deref() == Some(e.as_str())))
                            .map(|(h, _)| h.clone())
                            .collect();
                        let removed: Vec<String> = known
                            .iter()
                            .map(|(h, _)| h.clone())
                            .filter(|h| !tags.iter().any(|(th, _)| th == h) && !h.starts_with("local:"))
                            .collect();
                        (wanted, removed, rc.sync_token.clone())
                    }
                };
                let objects = if wanted.is_empty() { vec![] } else { dav.multiget(&rc.url, &wanted).await? };
                let s = lock(store);
                s.batch(|| {
                    for (href, etag, data) in &objects {
                        super::put_object(&s, cal_id, href, etag.as_deref(), "ics", data)?;
                    }
                    for href in &removed {
                        super::remove_object(&s, cal_id, href)?;
                    }
                    super::set_sync_state(&s, cal_id, token.as_deref().or(Some("")), rc.ctag.as_deref())
                })?;
                changed |= !objects.is_empty() || !removed.is_empty();
            }
            let s = lock(store);
            super::keep_calendars(&s, source_id, &ids)?;
            super::set_user_addresses(&s, super::AddressOrigin::Source(source_id), &found.addresses)?;
            super::synced(&s, source_id, None, Some(found.auto_schedule))?;
        }
        (SourceKind::Graph, Access::Graph(g)) => {
            let list = graph::calendars(g).await?;
            let mut ids = vec![];
            for c in &list {
                let cal_id =
                    super::upsert_calendar(&lock(store), source_id, &c.id, &c.name, c.color.as_deref(), c.writable)?;
                ids.push(c.id.clone());
                let state: Option<graph::DeltaState> =
                    super::sync_state(&lock(store), cal_id)?.0.and_then(|s| serde_json::from_str(&s).ok());
                let d = graph::delta(g, &c.id, state.as_ref(), Utc::now()).await?;
                let s = lock(store);
                s.batch(|| {
                    if d.full {
                        let seen: Vec<String> =
                            d.changed.iter().filter_map(|e| e["id"].as_str().map(str::to_owned)).collect();
                        for (href, _) in super::known_etags(&s, cal_id)? {
                            if !seen.contains(&href) && !href.starts_with("local:") {
                                super::remove_object(&s, cal_id, &href)?;
                            }
                        }
                    }
                    for e in &d.changed {
                        let Some(id) = e["id"].as_str() else { continue };
                        super::put_object(&s, cal_id, id, graph::etag_of(e).as_deref(), "graph", &e.to_string())?;
                    }
                    for id in &d.removed {
                        super::remove_object(&s, cal_id, id)?;
                    }
                    let state = d.state.as_ref().map(serde_json::to_string).transpose()?;
                    super::set_sync_state(&s, cal_id, state.as_deref(), None)
                })?;
                changed |= !d.changed.is_empty() || !d.removed.is_empty();
            }
            let s = lock(store);
            super::keep_calendars(&s, source_id, &ids)?;
            super::synced(&s, source_id, None, None)?;
        }
        (SourceKind::Jmap, Access::Jmap(client)) => {
            let list = jmap::calendars(client).await?;
            let mut cal_ids = vec![];
            for c in &list {
                let id =
                    super::upsert_calendar(&lock(store), source_id, &c.id, &c.name, c.color.as_deref(), c.writable)?;
                cal_ids.push((c.id.clone(), id));
            }
            let state = match cal_ids.first() {
                Some((_, id)) => super::sync_state(&lock(store), *id)?.0,
                None => None,
            };
            let ch = jmap::changes(client, state.as_deref()).await?;
            let identities = jmap::identities(client).await.unwrap_or_default();
            let account = super::source(&lock(store), source_id)?.account_id;
            let s = lock(store);
            if let Some(account) = account {
                super::set_user_addresses(&s, super::AddressOrigin::Account(account), &identities)?;
            }
            s.batch(|| {
                let seen: Vec<String> = ch.changed.iter().filter_map(|e| e["id"].as_str().map(str::to_owned)).collect();
                for (_, cal) in &cal_ids {
                    for (href, _) in super::known_etags(&s, *cal)? {
                        if ch.removed.contains(&href) || (ch.full && !seen.contains(&href)) {
                            super::remove_object(&s, *cal, &href)?;
                        }
                    }
                }
                for e in &ch.changed {
                    let (Some(id), Some(cal)) = (e["id"].as_str(), jmap::calendar_of(e)) else { continue };
                    let Some((_, cal_id)) = cal_ids.iter().find(|(r, _)| *r == cal) else { continue };
                    if let Some(data) = jmap::to_ics(e) {
                        // An event moved to another calendar leaves the old one.
                        for (_, other) in cal_ids.iter().filter(|(_, c)| c != cal_id) {
                            super::remove_object(&s, *other, id)?;
                        }
                        super::put_object(&s, *cal_id, id, None, "ics", &data)?;
                    }
                }
                for (_, cal) in &cal_ids {
                    super::set_sync_state(&s, *cal, Some(&ch.state), None)?;
                }
                super::keep_calendars(&s, source_id, &list.iter().map(|c| c.id.clone()).collect::<Vec<_>>())?;
                super::synced(&s, source_id, None, None)
            })?;
            changed |= !ch.changed.is_empty() || !ch.removed.is_empty();
        }
        (SourceKind::Ics, Access::Url(url)) => {
            let name = super::source(&lock(store), source_id)?.name;
            let cal_id = super::upsert_calendar(&lock(store), source_id, "ics", &name, None, false)?;
            let (etag, _) = super::sync_state(&lock(store), cal_id)?;
            match super::ics::fetch(url, etag.as_deref()).await? {
                super::ics::Fetched::Unchanged => {}
                super::ics::Fetched::Data { bytes, etag } => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let s = lock(store);
                    s.batch(|| {
                        super::put_object(&s, cal_id, "ics", None, "ics", &text)?;
                        super::set_sync_state(&s, cal_id, etag.as_deref(), None)
                    })?;
                    changed = true;
                }
            }
            super::synced(&lock(store), source_id, None, None)?;
        }
        _ => {
            return Err(Error::Invalid(
                crate::tr!("Die Anmeldung für diesen Kalender fehlt.", "The sign-in for this calendar is missing.")
                    .into(),
            ));
        }
    }
    Ok(changed)
}
