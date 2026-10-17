//! The inbox that sorts itself, on this device and from the header alone: every message gets a
//! category when it is stored ([`classify`]), the lists ask for one tab at a time ([`inbox`]),
//! newsletters can come as one row per sender (bundles), and the Screener holds back mail from
//! people who never wrote before until the user lets them in or turns them away.
//!
//! What the lists show is decided when they are read, so a change applies to old mail too:
//!
//! 1. a category the user gave the sender (`senders.category`, „Absender verschieben“),
//! 2. receipts by their header (a shop the user once wrote to still sends receipts),
//! 3. people for anyone the user wrote to (`contacts.sent`, which includes every reply),
//! 4. the category of the header ([`classify`]).
//!
//! The Screener (setting `screener`, off by default) holds back mail that arrived after it was
//! switched on from senders without an answer yet; contacts, replies to the user's own messages
//! and the user's own addresses always pass. A sender turned away („Abweisen“) is never deleted:
//! the messages move to the folder [`BLOCKED_FOLDER`] on the server, now and whenever new ones
//! arrive ([`move_blocked`], after each sync of the inbox).

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::actions::{self, Undo};
use crate::error::{Error, Result};
use crate::parse::ParsedMessage;
use crate::store::{MessageSummary, Store, now, summary};
use crate::tr;

/// Where the messages of senders the user turned away go (on the server, so other clients see
/// them out of the inbox too).
pub const BLOCKED_FOLDER: &str = "Screened Out";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    People,
    Newsletters,
    Notifications,
    Receipts,
    Other,
}

impl Category {
    pub const ALL: [Category; 5] =
        [Category::People, Category::Newsletters, Category::Notifications, Category::Receipts, Category::Other];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::People => "people",
            Category::Newsletters => "newsletters",
            Category::Notifications => "notifications",
            Category::Receipts => "receipts",
            Category::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// Local parts of machine senders: nobody reads what is sent back.
const MACHINE_LOCALS: &[&str] = &[
    "noreply",
    "no-reply",
    "no_reply",
    "donotreply",
    "do-not-reply",
    "do_not_reply",
    "notifications",
    "notification",
    "notify",
    "notifier",
    "alerts",
    "alert",
    "mailer-daemon",
    "postmaster",
    "bounce",
    "bounces",
    "automated",
    "automailer",
    "system",
    "robot",
    "daemon",
    "ci",
    "builds",
    "jenkins",
    "monitoring",
    "security",
    "account-security",
    "accountprotection",
];

/// Local parts of shops, billing and bookings (receipts when the subject says so).
const TRANSACTION_LOCALS: &[&str] = &[
    "billing",
    "invoice",
    "invoices",
    "rechnung",
    "rechnungen",
    "orders",
    "order",
    "bestellung",
    "bestellungen",
    "receipts",
    "receipt",
    "payments",
    "payment",
    "accounts",
    "accounting",
    "buchhaltung",
    "shop",
    "store",
    "sales",
    "versand",
    "shipping",
    "tickets",
    "booking",
    "bookings",
    "buchung",
    "service",
    "kundenservice",
    "auto-confirm",
    "updates",
    "transaktion",
];

/// Senders whose mail is receipts and confirmations (a domain and its subdomains).
const TRANSACTION_DOMAINS: &[&str] = &[
    "paypal.com",
    "paypal.de",
    "amazon.com",
    "amazon.de",
    "amazon.co.uk",
    "ebay.com",
    "ebay.de",
    "apple.com",
    "stripe.com",
    "uber.com",
    "booking.com",
    "etsy.com",
    "zalando.de",
    "otto.de",
    "bahn.de",
    "lieferando.de",
    "klarna.com",
    "steampowered.com",
    "airbnb.com",
    "amazon.fr",
    "amazon.it",
    "amazon.es",
    "amazon.nl",
    "amazon.ca",
    "playstation.com",
    "sonyentertainmentnetwork.com",
    "nintendo.com",
    "nintendo.de",
    "nintendo.net",
    "epicgames.com",
    "xbox.com",
    "microsoft.com",
    "itunes.com",
    "humblebundle.com",
    "gog.com",
    "ea.com",
    "ubisoft.com",
    "battle.net",
    "blizzard.com",
    "spotify.com",
    "netflix.com",
    "aliexpress.com",
    "temu.com",
    "mediamarkt.de",
    "saturn.de",
    "ikea.com",
    "ikea.de",
];

/// Senders whose mail is notifications even with list headers (code hosting, chat, accounts).
const NOTIFICATION_DOMAINS: &[&str] = &[
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "atlassian.net",
    "slack.com",
    "facebookmail.com",
    "accounts.google.com",
    "accountprotection.microsoft.com",
    "discord.com",
    "trello.com",
    "asana.com",
    "notion.so",
    "figma.com",
];

/// Words of receipts, invoices and orders in subjects (German and English, lower case).
const RECEIPT_WORDS: &[&str] = &[
    "rechnung",
    "quittung",
    "kaufbeleg",
    "zahlungsbeleg",
    "beleg",
    "bestellbestätigung",
    "bestellung",
    "zahlungsbestätigung",
    "zahlungseingang",
    "auftragsbestätigung",
    "buchungsbestätigung",
    "lieferschein",
    "gutschrift",
    "versandbestätigung",
    "receipt",
    "invoice",
    "order confirmation",
    "your order",
    "order #",
    "order no",
    "order number",
    "payment received",
    "payment confirmation",
    "purchase",
    "booking confirmation",
    "billing statement",
    "refund",
    "has shipped",
    "einkauf",
    "kaufbestätigung",
    "kassenbon",
    "order confirmed",
];

/// Subjects that are a receipt whoever sent them: a shop's thanks for a purchase or an order,
/// a confirmation of one (German and English, lower case).
const RECEIPT_PHRASES: &[&str] = &[
    "danke für ihren einkauf",
    "danke für deinen einkauf",
    "danke für ihre bestellung",
    "danke für deine bestellung",
    "dank für ihren einkauf",
    "dank für deinen einkauf",
    "dank für ihre bestellung",
    "dank für deine bestellung",
    "bestellbestätigung",
    "kaufbestätigung",
    "zahlungsbestätigung",
    "buchungsbestätigung",
    "thank you for your purchase",
    "thanks for your purchase",
    "thank you for your order",
    "thanks for your order",
    "order confirmation",
    "your receipt from",
    "ihre quittung",
    "deine quittung",
];

fn split_address(email: &str) -> (String, String) {
    let e = email.trim().to_ascii_lowercase();
    match e.rsplit_once('@') {
        Some((l, d)) => (l.to_string(), d.to_string()),
        None => (e, String::new()),
    }
}

fn domain_in(domain: &str, list: &[&str]) -> bool {
    list.iter().any(|d| domain == *d || domain.ends_with(&format!(".{d}")))
}

/// Whether a local part belongs to a machine: one of the known names, or one of them with a
/// suffix (`noreply-billing`, `notifications+abc`).
fn machine_local(local: &str) -> bool {
    MACHINE_LOCALS
        .iter()
        .any(|m| local == *m || local.starts_with(&format!("{m}-")) || local.starts_with(&format!("{m}+")))
        || local.starts_with("noreply")
        || local.starts_with("no-reply")
}

/// Whether a subject names a receipt, an invoice or an order (not in a reply: that is a
/// conversation about one).
fn receipt_subject(subject: &str) -> bool {
    let s = subject.trim().to_lowercase();
    if crate::thread::base_subject(subject).1 {
        return false;
    }
    RECEIPT_WORDS.iter().any(|w| s.contains(w))
}

/// Whether a subject is a receipt on its own ([`RECEIPT_PHRASES`], not in a reply).
fn receipt_phrase(subject: &str) -> bool {
    let s = subject.trim().to_lowercase();
    !crate::thread::base_subject(subject).1 && RECEIPT_PHRASES.iter().any(|w| s.contains(w))
}

/// The category of a message by its header: who sent it, its list and automation headers and
/// its subject.
pub fn classify(p: &ParsedMessage) -> Category {
    let Some(from) = &p.from else { return Category::Other };
    let (local, domain) = split_address(&from.email);
    let l = &p.list;
    let bulk = matches!(l.precedence.as_deref(), Some("bulk" | "list"));
    let machine = machine_local(&local) || l.auto_submitted;
    let transactional = TRANSACTION_LOCALS.contains(&local.as_str()) || domain_in(&domain, TRANSACTION_DOMAINS);
    // Receipts before anything else: a shop's receipt also comes with list headers and from
    // a machine.
    if receipt_phrase(&p.subject)
        || receipt_subject(&p.subject) && (machine || transactional || l.unsubscribe.is_some() || bulk)
    {
        return Category::Receipts;
    }
    if domain_in(&domain, NOTIFICATION_DOMAINS) {
        return Category::Notifications;
    }
    if l.list_post {
        // A discussion list: people write there, but not to the user.
        return Category::Other;
    }
    if machine || l.precedence.as_deref() == Some("junk") {
        return Category::Notifications;
    }
    if l.unsubscribe.is_some() || l.list_id.is_some() || bulk {
        return Category::Newsletters;
    }
    Category::People
}

// ---------------------------------------------------------------------------- the lists

/// The inbox rows with what the lists decide by. `?1`: one account or all (NULL), `?2`: now,
/// `?3`: since when the Screener holds mail back (NULL: off).
pub(crate) const BASE: &str =
    "SELECT m.id, m.mailbox_id, m.uid, m.thread_id, m.subject, m.from_name, m.from_email, m.date,
       m.internal_date, m.snippet, m.seen, m.flagged, m.answered, m.has_attachments, m.account_id, m.message_id,
       CASE WHEN s.category IS NOT NULL THEN s.category WHEN m.category = 'receipts' THEN 'receipts'
         WHEN COALESCE(c.sent, 0) > 0 THEN 'people' ELSE m.category END AS cat,
       COALESCE(k.pinned, 0) AS pin,
       MAX(COALESCE(m.date, m.internal_date), COALESCE(m.bumped_at, 0)) AS at,
       (?3 IS NOT NULL AND m.internal_date >= ?3 AND m.from_email IS NOT NULL AND s.screener IS NULL
         AND COALESCE(c.sent, 0) = 0 AND COALESCE(k.pinned, 0) = 0
         AND NOT EXISTS (SELECT 1 FROM accounts a WHERE a.email = m.from_email COLLATE NOCASE)
         AND NOT EXISTS (SELECT 1 FROM messages r JOIN mailboxes rb ON rb.id = r.mailbox_id
           WHERE rb.role = 'sent' AND r.account_id = m.account_id AND r.message_id = m.in_reply_to)) AS screened
     FROM messages m
     JOIN mailboxes b ON b.id = m.mailbox_id AND b.role = 'inbox'
     LEFT JOIN senders s ON s.email = m.from_email
     LEFT JOIN contacts c ON c.email = m.from_email
     LEFT JOIN marks k ON k.account_id = m.account_id AND k.message_key = COALESCE(m.message_id, 'local:' || m.id)
     WHERE m.hidden = 0 AND (?1 IS NULL OR m.account_id = ?1)
       AND (m.snoozed_until IS NULL OR m.snoozed_until <= ?2)
       AND COALESCE(s.screener, '') <> 'blocked'";

/// What the sorted inbox shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxQuery {
    /// One account's inbox, or all of them (the unified inbox).
    pub account: Option<i64>,
    pub tab: Category,
    /// Focus: people and pinned messages only, whatever the tab.
    #[serde(default)]
    pub focus: bool,
    /// One row per conversation.
    #[serde(default)]
    pub threads: bool,
    /// The newsletters tab: one row per sender.
    #[serde(default)]
    pub bundles: bool,
}

/// The Screener's start (None: off), from the settings.
fn screener_since(store: &Store) -> Option<i64> {
    let s = crate::settings::load(store).unwrap_or_default();
    if s.screener { Some(s.screener_since.unwrap_or(0)) } else { None }
}

/// A page of the sorted inbox, pinned rows first, then the newest; and how many rows there are.
pub fn inbox(store: &Store, q: &InboxQuery, limit: u32, offset: u32) -> Result<(u32, Vec<MessageSummary>)> {
    // Every statement names ?4, so the parameters stay the same either way.
    let filter = if q.focus { "(cat = 'people' OR pin = 1) AND ?4 = ?4" } else { "cat = ?4" };
    let group = if q.bundles && q.tab == Category::Newsletters && !q.focus {
        "lower(from_email)"
    } else if q.threads {
        "thread_id"
    } else {
        "id"
    };
    let shown =
        format!("WITH base AS ({BASE}), shown AS (SELECT *, {group} AS g FROM base WHERE screened = 0 AND {filter})");
    let since = screener_since(store);
    let tab = q.tab.as_str();
    // The page and the number of rows in one pass over the inbox (the count as a window).
    let sql = format!(
        "{shown}, ranked AS (
           SELECT *, ROW_NUMBER() OVER (PARTITION BY g ORDER BY pin DESC, at DESC, internal_date DESC, uid DESC, id DESC) AS rn,
             COUNT(*) OVER (PARTITION BY g) AS n, MIN(seen) OVER (PARTITION BY g) AS all_seen,
             MAX(flagged) OVER (PARTITION BY g) AS any_flag, MAX(has_attachments) OVER (PARTITION BY g) AS any_att,
             MAX(pin) OVER (PARTITION BY g) AS any_pin, MAX(at) OVER (PARTITION BY g) AS last_at
           FROM shown)
         SELECT id, mailbox_id, uid, thread_id, subject, from_name, from_email, last_at, snippet, all_seen, any_flag,
           answered, any_att, account_id, n, cat, any_pin, COUNT(*) OVER ()
         FROM ranked WHERE rn = 1 ORDER BY any_pin DESC, last_at DESC, internal_date DESC, uid DESC, id DESC LIMIT ?5 OFFSET ?6"
    );
    let bundles = group == "lower(from_email)";
    let mut st = store.conn().prepare_cached(&sql)?;
    let mut total = None;
    let rows = st.query_map(params![q.account, now(), since, tab, limit, offset], |r| {
        let mut s = summary(r)?;
        let n: u32 = r.get(14)?;
        if bundles {
            s.bundle = n;
        } else {
            s.thread_count = n;
        }
        s.category = r.get(15)?;
        s.pinned = r.get(16)?;
        Ok((s, r.get::<_, u32>(17)?))
    })?;
    let mut items = vec![];
    for row in rows {
        let (s, n) = row?;
        total = Some(n);
        items.push(s);
    }
    // A page past the end has no row to carry the number.
    let total = match total {
        Some(n) => n,
        None => store.conn().query_row(
            &format!("{shown} SELECT COUNT(DISTINCT g) FROM shown"),
            params![q.account, now(), since, tab],
            |r| r.get(0),
        )?,
    };
    Ok((total, items))
}

/// A page of a tab of the sorted inbox (one account's or all), by keyset: pinned rows first on
/// the first page, then the newest, `limit` after `after` (see [`crate::unified::Cursor`]). The
/// rows come from the newest messages walked on the date index, so a page reads about its
/// own size, not the whole inbox. One row per message: the conversation and bundle views go by
/// [`inbox`].
pub fn inbox_page(
    store: &Store,
    q: &InboxQuery,
    after: Option<crate::unified::Cursor>,
    limit: u32,
) -> Result<crate::unified::Page> {
    let filter = if q.focus { "(cat = 'people' OR pin = 1) AND ?4 = ?4" } else { "cat = ?4" };
    // The candidates: the newest `?7` by date below the cursor (walked on the date index), the
    // ones back from snoozing (their time is when they came back) and the pinned ones; the
    // inbox's rules then run on these only.
    let base = BASE.replacen("FROM messages m\n", "FROM cand CROSS JOIN messages m ON m.id = cand.id\n", 1);
    let sql = format!(
        "WITH cand(id) AS (
           SELECT id FROM (SELECT id FROM messages INDEXED BY messages_by_date
             WHERE hidden = 0
               AND mailbox_id IN (SELECT id FROM mailboxes WHERE role = 'inbox' AND (?1 IS NULL OR account_id = ?1))
               AND (?5 IS NULL OR (COALESCE(date, internal_date), id) < (?5, ?6))
             ORDER BY COALESCE(date, internal_date) DESC, id DESC LIMIT ?7)
           UNION SELECT id FROM messages INDEXED BY messages_bumped WHERE bumped_at IS NOT NULL
           UNION {PINNED}),
         base AS ({base})
         SELECT id, mailbox_id, uid, thread_id, subject, from_name, from_email, at, snippet, seen, flagged,
           answered, has_attachments, account_id, cat, pin
         FROM base WHERE screened = 0 AND {filter}
           AND (CASE WHEN ?5 IS NULL THEN 1 ELSE pin = 0 AND (at, id) < (?5, ?6) END)
         ORDER BY CASE WHEN ?5 IS NULL THEN pin ELSE 0 END DESC, at DESC, id DESC LIMIT ?8"
    );
    let since = screener_since(store);
    let mut st = store.conn().prepare_cached(&sql)?;
    let mut window = limit * 4 + 200;
    loop {
        let rows = st.query_map(
            params![q.account, now(), since, q.tab.as_str(), after.map(|c| c.date), after.map(|c| c.id), window, limit],
            |r| {
                let mut s = summary(r)?;
                s.category = r.get(14)?;
                s.pinned = r.get(15)?;
                Ok(s)
            },
        )?;
        let items: Vec<MessageSummary> = rows.collect::<rusqlite::Result<_>>()?;
        let unpinned = items.iter().filter(|m| !m.pinned).count() as u32;
        // Too few of this tab among the newest: look further down, unless the inbox ends.
        let seen_all = window >= MAX_WINDOW
            || {
                let n: u32 = store.conn().query_row(
                "SELECT COUNT(*) FROM (SELECT 1 FROM messages INDEXED BY messages_by_date
                   WHERE hidden = 0
                     AND mailbox_id IN (SELECT id FROM mailboxes WHERE role = 'inbox' AND (?1 IS NULL OR account_id = ?1))
                     AND (?2 IS NULL OR (COALESCE(date, internal_date), id) < (?2, ?3))
                   LIMIT ?4)",
                params![q.account, after.map(|c| c.date), after.map(|c| c.id), window],
                |r| r.get(0),
            )?;
                n < window
            };
        if (items.len() as u32) < limit && !seen_all {
            window = (window * 4).min(MAX_WINDOW);
            continue;
        }
        let next = (items.len() as u32 == limit && unpinned > 0)
            .then(|| items.iter().rev().find(|m| !m.pinned).map(|m| crate::unified::Cursor { date: m.date, id: m.id }))
            .flatten();
        return Ok(crate::unified::Page { items, next });
    }
}

/// The ids of the pinned messages (by Message-ID, or `local:` and the id).
const PINNED: &str =
    "SELECT m2.id FROM marks k2 JOIN messages m2 ON m2.account_id = k2.account_id AND m2.message_id = k2.message_key
       WHERE k2.pinned = 1
     UNION SELECT CAST(substr(message_key, 7) AS INTEGER) FROM marks WHERE pinned = 1 AND message_key LIKE 'local:%'";

/// The most messages a page of [`inbox_page`] looks at.
const MAX_WINDOW: u32 = 1_000_000;

/// The numbers of one tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TabCount {
    pub category: Category,
    pub total: u32,
    pub unread: u32,
}

/// The tabs' numbers and the senders waiting in the Screener.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboxCounts {
    pub tabs: Vec<TabCount>,
    /// Focus: people and pinned.
    pub focus: TabCount,
    /// Senders waiting in „Neue Absender“.
    pub screener: u32,
}

pub fn counts(store: &Store, account: Option<i64>) -> Result<InboxCounts> {
    match screener_since(store) {
        // Without the Screener nothing depends on more than the sender: the index's numbers per
        // category, then the senders that are sorted otherwise counted again.
        None => quick_counts(store, account),
        Some(_) => full_counts(store, account),
    }
}

/// [`counts`] without the Screener: the stored categories counted on the index
/// (`messages_tabs`), corrected for the senders the user moved, blocked or wrote to and for the
/// pinned messages: the same numbers as [`full_counts`] without reading every message.
fn quick_counts(store: &Store, account: Option<i64>) -> Result<InboxCounts> {
    use std::collections::HashMap;
    let conn = store.conn();
    let at = now();
    let mut tab: HashMap<String, (i64, i64)> = HashMap::new();
    let add = |tab: &mut HashMap<String, (i64, i64)>, cat: &str, n: i64, unread: i64| {
        let e = tab.entry(cat.to_string()).or_default();
        e.0 += n;
        e.1 += unread;
    };
    let mut st = conn.prepare_cached(
        "SELECT category, COUNT(*), COALESCE(SUM(seen = 0), 0) FROM messages INDEXED BY messages_tabs
         WHERE mailbox_id IN (SELECT id FROM mailboxes WHERE role = 'inbox' AND (?1 IS NULL OR account_id = ?1))
           AND hidden = 0 AND (snoozed_until IS NULL OR snoozed_until <= ?2)
         GROUP BY mailbox_id, category",
    )?;
    for row in st.query_map(params![account, at], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))? {
        let (cat, n, unread) = row?;
        add(&mut tab, &cat, n, unread);
    }
    // The senders whose mail is not where its header put it: moved, blocked, or written to.
    let mut st = conn.prepare_cached(
        "WITH e(email) AS (SELECT email FROM senders WHERE category IS NOT NULL OR screener = 'blocked'
                            UNION SELECT email FROM contacts WHERE sent > 0)
         SELECT m.category, m.seen, s.category, COALESCE(s.screener, ''), COALESCE(c.sent, 0)
         FROM e CROSS JOIN messages m INDEXED BY messages_sender ON m.from_email = e.email COLLATE NOCASE
           LEFT JOIN senders s ON s.email = m.from_email
           LEFT JOIN contacts c ON c.email = m.from_email
         WHERE m.mailbox_id IN (SELECT id FROM mailboxes WHERE role = 'inbox' AND (?1 IS NULL OR account_id = ?1))
           AND m.hidden = 0 AND (m.snoozed_until IS NULL OR m.snoozed_until <= ?2)",
    )?;
    let rows = st.query_map(params![account, at], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, bool>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    for row in rows {
        let (stored, seen, moved, screener, sent) = row?;
        add(&mut tab, &stored, -1, -(!seen as i64));
        if screener == "blocked" {
            continue;
        }
        let cat = match moved {
            Some(c) => c,
            None if stored == "receipts" => stored,
            None if sent > 0 => "people".into(),
            None => stored,
        };
        add(&mut tab, &cat, 1, !seen as i64);
    }
    let get = |cat: Category| tab.get(cat.as_str()).copied().unwrap_or_default();
    let tabs: Vec<TabCount> = Category::ALL
        .iter()
        .map(|c| {
            let (n, unread) = get(*c);
            TabCount { category: *c, total: n.max(0) as u32, unread: unread.max(0) as u32 }
        })
        .collect();
    // Focus: people, and the pinned messages of the other tabs.
    let (people, people_unread) = get(Category::People);
    let base = BASE.replacen("FROM messages m\n", "FROM cand CROSS JOIN messages m ON m.id = cand.id\n", 1);
    let pinned: (i64, i64) = conn.query_row(
        &format!(
            "WITH cand(id) AS ({PINNED}), base AS ({base})
             SELECT COUNT(*), COALESCE(SUM(seen = 0), 0) FROM base WHERE pin = 1 AND cat <> 'people'"
        ),
        params![account, at, Option::<i64>::None],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let focus = TabCount {
        category: Category::People,
        total: (people + pinned.0).max(0) as u32,
        unread: (people_unread + pinned.1).max(0) as u32,
    };
    Ok(InboxCounts { tabs, focus, screener: 0 })
}

/// [`counts`] with the Screener on: every message of the inbox decided one by one.
pub(crate) fn full_counts(store: &Store, account: Option<i64>) -> Result<InboxCounts> {
    let since = screener_since(store);
    let mut st = store.conn().prepare(&format!(
        "WITH base AS ({BASE})
         SELECT cat, pin, COUNT(*), COALESCE(SUM(seen = 0), 0) FROM base WHERE screened = 0 GROUP BY cat, pin"
    ))?;
    let rows = st.query_map(params![account, now(), since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, u32>(2)?, r.get::<_, u32>(3)?))
    })?;
    let mut tabs: Vec<TabCount> =
        Category::ALL.iter().map(|c| TabCount { category: *c, total: 0, unread: 0 }).collect();
    let mut focus = TabCount { category: Category::People, total: 0, unread: 0 };
    for row in rows {
        let (cat, pin, total, unread) = row?;
        let cat = Category::parse(&cat).unwrap_or(Category::Other);
        if let Some(t) = tabs.iter_mut().find(|t| t.category == cat) {
            t.total += total;
            t.unread += unread;
        }
        if cat == Category::People || pin {
            focus.total += total;
            focus.unread += unread;
        }
    }
    let screener: u32 = store.conn().query_row(
        &format!("WITH base AS ({BASE}) SELECT COUNT(DISTINCT lower(from_email)) FROM base WHERE screened = 1"),
        params![account, now(), since],
        |r| r.get(0),
    )?;
    Ok(InboxCounts { tabs, focus, screener })
}

/// The messages of one sender in the inbox, newest first (a bundle opened as a stream).
pub fn bundle(store: &Store, account: Option<i64>, sender: &str, limit: u32) -> Result<Vec<MessageSummary>> {
    let since = screener_since(store);
    let mut st = store.conn().prepare(&format!(
        "WITH base AS ({BASE})
         SELECT id, mailbox_id, uid, thread_id, subject, from_name, from_email, at, snippet, seen, flagged,
           answered, has_attachments, account_id, cat, pin
         FROM base WHERE screened = 0 AND from_email = ?4 COLLATE NOCASE ORDER BY at DESC, internal_date DESC, uid DESC, id DESC LIMIT ?5"
    ))?;
    let rows = st.query_map(params![account, now(), since, sender.trim(), limit], |r| {
        let mut s = summary(r)?;
        s.category = r.get(14)?;
        s.pinned = r.get(15)?;
        Ok(s)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The category the lists show for a message (the sender's own, people for contacts, the
/// header's).
pub fn category_of(store: &Store, id: i64) -> Result<Category> {
    let cat: Option<String> = store
        .conn()
        .query_row(
            "SELECT CASE WHEN s.category IS NOT NULL THEN s.category WHEN m.category = 'receipts' THEN 'receipts'
               WHEN COALESCE(c.sent, 0) > 0 THEN 'people' ELSE m.category END
             FROM messages m LEFT JOIN senders s ON s.email = m.from_email LEFT JOIN contacts c ON c.email = m.from_email
             WHERE m.id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(cat.as_deref().and_then(Category::parse).unwrap_or(Category::Other))
}

/// The new messages worth a notification: with Focus only people and pinned ones, with the
/// Screener none that wait in „Neue Absender“.
pub fn worth_telling(store: &Store, account_id: i64, ids: &[i64]) -> Result<Vec<i64>> {
    let settings = crate::settings::load(store).unwrap_or_default();
    if !settings.focus && !settings.screener {
        return Ok(ids.to_vec());
    }
    let since = screener_since(store);
    let mut st =
        store.conn().prepare(&format!("WITH base AS ({BASE}) SELECT screened, cat, pin FROM base WHERE id = ?4"))?;
    let mut out = vec![];
    for &id in ids {
        let row = st
            .query_row(params![Some(account_id), now(), since, id], |r| {
                Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?))
            })
            .optional()?;
        let Some((screened, cat, pin)) = row else { continue };
        if screened || (settings.focus && cat != "people" && !pin) {
            continue;
        }
        out.push(id);
    }
    Ok(out)
}

/// The new messages of an account's inbox from people or other senders (not newsletters,
/// notifications or the like), and none the Screener holds back or turned away.
pub fn personal(store: &Store, account_id: i64, ids: &[i64]) -> Result<Vec<i64>> {
    let since = screener_since(store);
    let mut st =
        store.conn().prepare(&format!("WITH base AS ({BASE}) SELECT screened, cat FROM base WHERE id = ?4"))?;
    let mut out = vec![];
    for &id in ids {
        let row = st
            .query_row(params![Some(account_id), now(), since, id], |r| {
                Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?))
            })
            .optional()?;
        let Some((screened, cat)) = row else { continue };
        if screened || !matches!(Category::parse(&cat), Some(Category::People | Category::Other)) {
            continue;
        }
        out.push(id);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------- senders

/// Remembers the category of a sender for all its mail (`None`: back to the header's).
pub fn set_sender_category(store: &Store, email: &str, category: Option<Category>) -> Result<()> {
    let email = email.trim();
    if !email.contains('@') {
        return Err(Error::Invalid(tr!("Diese Adresse ist ungültig.", "This address is not valid.").into()));
    }
    store.conn().execute(
        "INSERT INTO senders (email, category, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (email) DO UPDATE SET category = excluded.category, updated_at = excluded.updated_at",
        params![email, category.map(Category::as_str), now()],
    )?;
    Ok(())
}

/// A sender waiting in „Neue Absender“.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WaitingSender {
    pub email: String,
    pub name: Option<String>,
    /// Its messages waiting, newest first.
    pub messages: Vec<MessageSummary>,
}

/// The senders the Screener holds back, the newest first.
pub fn waiting_senders(store: &Store, account: Option<i64>) -> Result<Vec<WaitingSender>> {
    let since = screener_since(store);
    if since.is_none() {
        return Ok(vec![]);
    }
    let mut st = store.conn().prepare(&format!(
        "WITH base AS ({BASE})
         SELECT id, mailbox_id, uid, thread_id, subject, from_name, from_email, at, snippet, seen, flagged,
           answered, has_attachments, account_id
         FROM base WHERE screened = 1 ORDER BY at DESC, id DESC LIMIT 500"
    ))?;
    let rows = st.query_map(params![account, now(), since], summary)?;
    let mut out: Vec<WaitingSender> = vec![];
    for row in rows {
        let m = row?;
        let Some(from) = m.from.clone() else { continue };
        match out.iter_mut().find(|w| w.email.eq_ignore_ascii_case(&from.email)) {
            Some(w) => {
                if w.name.is_none() {
                    w.name = from.name.clone();
                }
                w.messages.push(m);
            }
            None => out.push(WaitingSender { email: from.email.clone(), name: from.name.clone(), messages: vec![m] }),
        }
    }
    Ok(out)
}

/// „Zulassen“: the sender's mail shows in the inbox from now on.
pub fn allow_sender(store: &Store, email: &str) -> Result<()> {
    set_screener(store, email, Some("allowed"))
}

/// „Abweisen“: the sender's mail in the inbox moves to [`BLOCKED_FOLDER`] (and so will every
/// message that comes later); nothing is deleted.
pub fn block_sender(store: &Store, email: &str) -> Result<Undo> {
    set_screener(store, email, Some("blocked"))?;
    move_blocked(store, None)
}

/// Takes the Screener's answer for a sender back (it waits again, or shows as before).
pub fn forget_sender(store: &Store, email: &str) -> Result<()> {
    set_screener(store, email, None)
}

fn set_screener(store: &Store, email: &str, answer: Option<&str>) -> Result<()> {
    let email = email.trim();
    if !email.contains('@') {
        return Err(Error::Invalid(tr!("Diese Adresse ist ungültig.", "This address is not valid.").into()));
    }
    store.conn().execute(
        "INSERT INTO senders (email, screener, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (email) DO UPDATE SET screener = excluded.screener, updated_at = excluded.updated_at",
        params![email, answer, now()],
    )?;
    Ok(())
}

/// The senders the user turned away.
pub fn blocked_senders(store: &Store) -> Result<Vec<String>> {
    let mut st =
        store.conn().prepare("SELECT email FROM senders WHERE screener = 'blocked' ORDER BY email COLLATE NOCASE")?;
    let rows = st.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Moves the inbox mail of turned-away senders (of one account, or all) to [`BLOCKED_FOLDER`];
/// the worker calls it after each sync of the inbox.
pub fn move_blocked(store: &Store, account: Option<i64>) -> Result<Undo> {
    let mut st = store.conn().prepare(
        "SELECT m.id FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id AND b.role = 'inbox'
         JOIN senders s ON s.email = m.from_email
         WHERE s.screener = 'blocked' AND m.hidden = 0 AND (?1 IS NULL OR m.account_id = ?1)",
    )?;
    let ids: Vec<i64> = st.query_map([account], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    if ids.is_empty() {
        return Ok(Undo::default());
    }
    actions::move_to_named(store, &ids, BLOCKED_FOLDER)
}

// ---------------------------------------------------------------------------- subscriptions

/// A sender of newsletters with an unsubscribe address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subscription {
    pub sender: String,
    pub name: Option<String>,
    pub list_id: Option<String>,
    /// Messages of it here.
    pub count: u32,
    pub last: i64,
    /// The way out the newest message offers: `one_click`, `mailto` or `link`.
    pub method: Option<String>,
    /// When the user unsubscribed, and how.
    pub unsubscribed: Option<i64>,
    /// The newest message, to open it.
    pub message_id: i64,
}

/// Every sender with an unsubscribe address, the ones with the most mail first.
pub fn subscriptions(store: &Store) -> Result<Vec<Subscription>> {
    let mut st = store.conn().prepare(
        "SELECT x.from_email, x.from_name, x.list_id, x.n, x.at, x.unsubscribe, x.one_click, u.at, x.id FROM (
           SELECT m.*, COALESCE(m.date, m.internal_date) AS at,
             ROW_NUMBER() OVER (PARTITION BY lower(m.from_email) ORDER BY COALESCE(m.date, m.internal_date) DESC, m.id DESC) AS rn,
             COUNT(*) OVER (PARTITION BY lower(m.from_email)) AS n
           FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id
           WHERE m.unsubscribe IS NOT NULL AND m.hidden = 0 AND m.from_email IS NOT NULL
             AND COALESCE(b.role, '') NOT IN ('sent', 'drafts')) x
         LEFT JOIN unsubscribes u ON u.sender = x.from_email
         WHERE x.rn = 1 ORDER BY u.at IS NOT NULL, x.n DESC, x.at DESC",
    )?;
    let rows = st.query_map([], |r| {
        let header: String = r.get(5)?;
        let one_click: bool = r.get(6)?;
        Ok(Subscription {
            sender: r.get(0)?,
            name: r.get(1)?,
            list_id: r.get(2)?,
            count: r.get(3)?,
            last: r.get(4)?,
            method: crate::unsubscribe::best(&header, one_click).map(|m| m.kind().to_string()),
            unsubscribed: r.get(7)?,
            message_id: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Messages stored before a receipt rule existed that are receipts by it, decided again from
/// what the store kept of their header (sender, subject, list headers); returns how many.
pub fn reclassify_receipts(store: &Store) -> Result<usize> {
    // Each stored message as far as the store kept its header.
    let rows: Vec<(i64, ParsedMessage)> = {
        let mut st = store.conn().prepare(
            "SELECT id, from_email, subject, list_id, unsubscribe FROM messages
             WHERE category <> 'receipts' AND from_email IS NOT NULL",
        )?;
        st.query_map([], |r| {
            let p = ParsedMessage {
                from: Some(crate::parse::Address { name: None, email: r.get(1)? }),
                subject: r.get(2)?,
                list: crate::parse::ListHeaders { list_id: r.get(3)?, unsubscribe: r.get(4)?, ..Default::default() },
                ..ParsedMessage::default()
            };
            Ok((r.get(0)?, p))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut changed = 0;
    for (id, p) in rows {
        if classify(&p) == Category::Receipts {
            store.conn().execute("UPDATE messages SET category = 'receipts' WHERE id = ?1", [id])?;
            changed += 1;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::MailboxEntry;
    use crate::parse::{Address, ListHeaders, parse};
    use crate::store::tests::{header, store_with_inbox};

    fn msg(from: &str, subject: &str, list: ListHeaders) -> ParsedMessage {
        ParsedMessage {
            from: Some(Address { name: None, email: from.into() }),
            subject: subject.into(),
            list,
            ..ParsedMessage::default()
        }
    }

    #[test]
    fn categories_from_the_header() {
        let none = ListHeaders::default;
        let news = || ListHeaders { unsubscribe: Some("<https://x.example/u>".into()), ..ListHeaders::default() };
        assert_eq!(classify(&msg("jonas@example.org", "Kaffee morgen?", none())), Category::People);
        assert_eq!(classify(&msg("lea@example.org", "Rechnung März", none())), Category::People, "a person's invoice");
        assert_eq!(classify(&msg("letters@fieldnotes.example", "Field Notes #48", news())), Category::Newsletters);
        let listed = ListHeaders { list_id: Some("news.example".into()), ..ListHeaders::default() };
        assert_eq!(classify(&msg("post@verein.example", "Neuigkeiten", listed)), Category::Newsletters);
        let bulk = ListHeaders { precedence: Some("bulk".into()), ..ListHeaders::default() };
        assert_eq!(classify(&msg("info@shop.example", "Angebote", bulk)), Category::Newsletters);
        assert_eq!(classify(&msg("noreply@bank.example", "Neue Anmeldung", none())), Category::Notifications);
        assert_eq!(classify(&msg("no-reply-alerts@x.example", "Alarm", none())), Category::Notifications);
        let auto = ListHeaders { auto_submitted: true, ..ListHeaders::default() };
        assert_eq!(classify(&msg("lea@example.org", "Abwesend", auto)), Category::Notifications);
        assert_eq!(classify(&msg("notifications@github.com", "[repo] PR #4", news())), Category::Notifications);
        assert_eq!(classify(&msg("accounts@example.com", "Invoice INV-2026-031", none())), Category::Receipts);
        assert_eq!(classify(&msg("updates@parcels.example", "Your order has shipped", none())), Category::Receipts);
        assert_eq!(classify(&msg("shop@laden.example", "Deine Bestellung 4417", none())), Category::Receipts);
        assert_eq!(classify(&msg("service@paypal.de", "Zahlungsbestätigung", none())), Category::Receipts);
        assert_eq!(classify(&msg("shop@laden.example", "AW: Deine Bestellung 4417", none())), Category::People);
        let discussion = ListHeaders { list_post: true, list_id: Some("dev.lists.example".into()), ..news() };
        assert_eq!(classify(&msg("jonas@example.org", "Re: Release", discussion)), Category::Other);
        assert_eq!(classify(&ParsedMessage::default()), Category::Other, "no sender");
    }

    /// Receipts of shops go to „Belege“ before any other signal: a shop's thanks for a
    /// purchase whoever sent it, the known shops' order mail, also from a shop the user once
    /// wrote to.
    #[test]
    fn receipts_of_shops_before_anything_else() {
        let none = ListHeaders::default;
        let news = || ListHeaders { unsubscribe: Some("<https://x.example/u>".into()), ..ListHeaders::default() };
        for (from, subject, list) in [
            ("Sony@email.sonyentertainmentnetwork.com", "Danke für Ihren Einkauf", none()),
            ("reply@txn-email.playstation.com", "Danke für Ihren Einkauf", news()),
            ("PlayStation@playstation.com", "Ihr Einkauf bei PlayStation Store", none()),
            ("bestellbestaetigung@amazon.de", "Ihre Amazon.de Bestellung #302-123", none()),
            ("no_reply@email.apple.com", "Your receipt from Apple.", none()),
            ("noreply@steampowered.com", "Thank you for your Steam purchase!", none()),
            ("service@paypal.de", "Sie haben eine Zahlung an Zalando gesendet: Kaufbestätigung", none()),
            ("info@kleiner-laden.example", "Vielen Dank für Ihre Bestellung", news()),
            ("hello@shop.example", "Order confirmation #4471", none()),
            ("store@nintendo.de", "Danke für deinen Einkauf im Nintendo eShop", none()),
        ] {
            assert_eq!(classify(&msg(from, subject, list)), Category::Receipts, "{from}: {subject}");
        }
        assert_eq!(classify(&msg("lea@example.org", "Rechnung März", none())), Category::People, "a person's invoice");
        assert_eq!(classify(&msg("shop@laden.example", "Re: Danke für Ihren Einkauf", none())), Category::People);

        // A shop the user wrote to (its support) still sends receipts, not people's mail.
        let (st, _, inbox_id) = store_with_inbox();
        let id = add(
            &st,
            inbox_id,
            1,
            &raw("PlayStation Network <Sony@email.sonyentertainmentnetwork.com>", "Danke für Ihren Einkauf", ""),
        );
        st.touch_contact(&Address { name: None, email: "Sony@email.sonyentertainmentnetwork.com".into() }, true, 1)
            .unwrap();
        assert_eq!(category_of(&st, id).unwrap(), Category::Receipts);
        assert_eq!(subjects(&st, &query(Category::Receipts)), ["Danke für Ihren Einkauf"]);
        assert!(subjects(&st, &query(Category::People)).is_empty());

        // Mail stored before this rule is sorted again once.
        st.conn().execute("UPDATE messages SET category = 'people'", []).unwrap();
        assert_eq!(reclassify_receipts(&st).unwrap(), 1);
        assert_eq!(reclassify_receipts(&st).unwrap(), 0);
        assert_eq!(
            st.conn().query_row("SELECT category FROM messages", [], |r| r.get::<_, String>(0)).unwrap(),
            "receipts"
        );
    }

    fn add(s: &Store, inbox: i64, uid: u32, raw: &str) -> i64 {
        s.insert_message(inbox, &header(uid, raw.as_bytes()), &parse(raw.as_bytes()).unwrap()).unwrap()
    }

    fn raw(from: &str, subject: &str, extra: &str) -> String {
        format!(
            "From: {from}\r\nTo: maria@example.org\r\nSubject: {subject}\r\nMessage-ID: <{}@x>\r\n{extra}\r\nText\r\n",
            subject.replace(' ', ".")
        )
    }

    fn query(tab: Category) -> InboxQuery {
        InboxQuery { account: None, tab, focus: false, threads: false, bundles: true }
    }

    fn subjects(s: &Store, q: &InboxQuery) -> Vec<String> {
        inbox(s, q, 50, 0).unwrap().1.into_iter().map(|m| m.subject).collect()
    }

    #[test]
    fn tabs_counts_moves_and_bundles() {
        let (s, _, inbox_id) = store_with_inbox();
        let unsub = "List-Unsubscribe: <https://news.example/u>\r\n";
        add(&s, inbox_id, 1, &raw("Jonas <jonas@example.org>", "Kaffee", ""));
        add(&s, inbox_id, 2, &raw("Field Notes <letters@fieldnotes.example>", "Ausgabe 47", unsub));
        add(&s, inbox_id, 3, &raw("Field Notes <letters@fieldnotes.example>", "Ausgabe 48", unsub));
        add(&s, inbox_id, 4, &raw("Stadtwerke <news@stadtwerke.example>", "Oktober", unsub));
        add(&s, inbox_id, 5, &raw("Bank <noreply@bank.example>", "Neue Anmeldung", ""));
        assert_eq!(subjects(&s, &query(Category::People)), ["Kaffee"]);
        let news = inbox(&s, &query(Category::Newsletters), 50, 0).unwrap();
        assert_eq!(news.0, 2, "one row per sender");
        assert_eq!((news.1[0].subject.as_str(), news.1[0].bundle), ("Oktober", 1));
        assert_eq!((news.1[1].subject.as_str(), news.1[1].bundle), ("Ausgabe 48", 2));
        let flat = InboxQuery { bundles: false, ..query(Category::Newsletters) };
        assert_eq!(inbox(&s, &flat, 50, 0).unwrap().0, 3);
        assert_eq!(bundle(&s, None, "LETTERS@fieldnotes.example", 10).unwrap().len(), 2);
        let c = counts(&s, None).unwrap();
        let tab = |c: &InboxCounts, cat| c.tabs.iter().find(|t| t.category == cat).unwrap().clone();
        assert_eq!((tab(&c, Category::Newsletters).total, tab(&c, Category::Newsletters).unread), (3, 3));
        assert_eq!(tab(&c, Category::Notifications).total, 1);

        // The user moves a sender: all of its mail, and what comes later.
        set_sender_category(&s, "news@stadtwerke.example", Some(Category::People)).unwrap();
        assert_eq!(subjects(&s, &query(Category::People)), ["Oktober", "Kaffee"]);
        add(&s, inbox_id, 6, &raw("Stadtwerke <news@stadtwerke.example>", "November", unsub));
        assert_eq!(subjects(&s, &query(Category::People))[0], "November");
        // Someone the user wrote to is a person.
        s.touch_contact(&Address { name: None, email: "noreply@bank.example".into() }, true, 1).unwrap();
        assert!(subjects(&s, &query(Category::People)).contains(&"Neue Anmeldung".to_string()));
        assert_eq!(category_of(&s, 5).unwrap(), Category::People);
    }

    #[test]
    fn focus_shows_people_and_pinned() {
        let (s, account, inbox_id) = store_with_inbox();
        add(&s, inbox_id, 1, &raw("Jonas <jonas@example.org>", "Kaffee", ""));
        let n = add(&s, inbox_id, 2, &raw("Bank <noreply@bank.example>", "Kontoauszug", ""));
        add(&s, inbox_id, 3, &raw("Shop <noreply@shop.example>", "Status", ""));
        crate::later::set_pinned(&s, &[n], true).unwrap();
        let q = InboxQuery { focus: true, ..query(Category::People) };
        assert_eq!(subjects(&s, &q), ["Kontoauszug", "Kaffee"], "pinned first");
        assert_eq!(counts(&s, Some(account)).unwrap().focus.total, 2);
        assert_eq!(worth_telling(&s, account, &[1, 2, 3]).unwrap(), [1, 2, 3], "focus off: everything");
        let mut st = crate::settings::load(&s).unwrap();
        st.focus = true;
        crate::settings::save(&s, &st).unwrap();
        assert_eq!(worth_telling(&s, account, &[1, n, 3]).unwrap(), [1, n]);
    }

    #[test]
    fn the_screener_holds_back_first_time_senders() {
        let (s, account, inbox_id) = store_with_inbox();
        add(&s, inbox_id, 1, &raw("Jonas <jonas@example.org>", "Vorher", ""));
        let mut st = crate::settings::load(&s).unwrap();
        st.screener = true;
        st.screener_since = Some(1_760_000_002);
        crate::settings::save(&s, &st).unwrap();
        // uid 2 and later arrive after the Screener was switched on (internal date 1_760_000_000 + uid).
        add(&s, inbox_id, 2, &raw("Unbekannt <neu@example.net>", "Hallo", ""));
        add(&s, inbox_id, 3, &raw("Unbekannt <neu@example.net>", "Noch einmal", ""));
        add(&s, inbox_id, 4, &raw("Werbung <spam@example.net>", "Angebot", ""));
        s.touch_contact(&Address { name: None, email: "lea@example.org".into() }, true, 1).unwrap();
        add(&s, inbox_id, 5, &raw("Lea <lea@example.org>", "Kontakt", ""));
        // A reply to the user's own message passes.
        let sent = s
            .upsert_mailbox(
                account,
                &MailboxEntry { name: "Sent".into(), delimiter: None, attributes: vec!["\\Sent".into()] },
            )
            .unwrap();
        add(&s, sent, 1, &raw("Maria <maria@example.org>", "Frage", ""));
        add(&s, inbox_id, 6, &raw("Fremd <fremd@example.net>", "Re Frage", "In-Reply-To: <Frage@x>\r\n"));
        assert_eq!(subjects(&s, &query(Category::People)), ["Re Frage", "Kontakt", "Vorher"]);
        let waiting = waiting_senders(&s, None).unwrap();
        assert_eq!(
            waiting.iter().map(|w| w.email.as_str()).collect::<Vec<_>>(),
            ["spam@example.net", "neu@example.net"]
        );
        assert_eq!(waiting[1].messages.len(), 2);
        assert_eq!(counts(&s, None).unwrap().screener, 2);
        assert!(worth_telling(&s, account, &[2]).unwrap().is_empty(), "no notification while it waits");

        allow_sender(&s, "neu@example.net").unwrap();
        assert!(subjects(&s, &query(Category::People)).contains(&"Hallo".to_string()));
        let undo = block_sender(&s, "spam@example.net").unwrap();
        assert_eq!(undo.hidden.len(), 1, "moved, not deleted");
        assert!(s.mailbox_by_name(account, BLOCKED_FOLDER).unwrap().is_some());
        assert!(waiting_senders(&s, None).unwrap().is_empty());
        let kinds: Vec<String> = crate::outbox::entries(&s).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["create_folder", "move"]);
        // Later mail of the sender goes the same way.
        add(&s, inbox_id, 7, &raw("Werbung <spam@example.net>", "Neues Angebot", ""));
        assert!(!subjects(&s, &query(Category::People)).contains(&"Neues Angebot".to_string()));
        assert_eq!(move_blocked(&s, Some(account)).unwrap().hidden.len(), 1);
        assert_eq!(blocked_senders(&s).unwrap(), ["spam@example.net"]);
    }

    /// A turned-away sender stays turned away whatever case the address comes in.
    #[test]
    fn blocked_senders_match_in_any_case() {
        let (s, account, inbox_id) = store_with_inbox();
        block_sender(&s, "news@example.net").unwrap();
        add(&s, inbox_id, 1, &raw("News <News@Example.NET>", "Angebot", ""));
        add(&s, inbox_id, 2, &raw("Jonas <jonas@example.org>", "Hallo", ""));
        assert_eq!(move_blocked(&s, Some(account)).unwrap().hidden.len(), 1);
        assert_eq!(subjects(&s, &query(Category::People)), ["Hallo"]);
    }

    #[test]
    fn subscriptions_list_senders_with_a_way_out() {
        let (s, _, inbox_id) = store_with_inbox();
        let one_click =
            "List-Unsubscribe: <https://news.example/u?x=1>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n";
        add(&s, inbox_id, 1, &raw("News <news@news.example>", "Eins", one_click));
        add(&s, inbox_id, 2, &raw("News <news@news.example>", "Zwei", one_click));
        add(
            &s,
            inbox_id,
            3,
            &raw("Verein <post@verein.example>", "Rundbrief", "List-Unsubscribe: <mailto:aus@verein.example>\r\n"),
        );
        add(&s, inbox_id, 4, &raw("Jonas <jonas@example.org>", "Kaffee", ""));
        let subs = subscriptions(&s).unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(
            (subs[0].sender.as_str(), subs[0].count, subs[0].method.as_deref()),
            ("news@news.example", 2, Some("one_click"))
        );
        assert_eq!(subs[1].method.as_deref(), Some("mailto"));
        crate::unsubscribe::record(&s, "news@news.example", "one_click").unwrap();
        let subs = subscriptions(&s).unwrap();
        assert_eq!(subs[1].sender, "news@news.example", "unsubscribed ones go last");
        assert!(subs[1].unsubscribed.is_some());
    }
}
