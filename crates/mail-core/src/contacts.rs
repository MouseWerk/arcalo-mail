//! Contacts: the people of the user's mail (everyone who wrote, everyone the user wrote to:
//! the `contacts` table, filled as mail arrives and goes out) and the cards of address books
//! (CardDAV, [`crate::carddav`]; Microsoft's contacts, [`crate::graph`]). The compose sheet's
//! suggestions and the contact card read both.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::carddav::Card;
use crate::error::{Error, Result};
use crate::store::{Contact, MessageSummary, SUMMARY_COLS, Store, now, summary};
use crate::tr;

/// An address book as the settings list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddressBook {
    pub id: i64,
    /// `carddav` or `graph`.
    pub kind: String,
    pub name: String,
    pub url: String,
    pub username: String,
    pub account_id: Option<i64>,
    pub cards: u32,
    pub last_sync: Option<i64>,
    pub error: Option<String>,
}

/// The credential store name of a CardDAV book's password.
pub fn secret_key(book_id: i64) -> String {
    format!("carddav-{book_id}-password")
}

pub fn books(store: &Store) -> Result<Vec<AddressBook>> {
    let mut st = store.conn().prepare(
        "SELECT b.id, b.kind, b.name, b.url, b.username, b.account_id, (SELECT COUNT(*) FROM cards c WHERE c.book_id = b.id),
           b.last_sync, b.error FROM address_books b ORDER BY b.id",
    )?;
    let rows = st.query_map([], |r| {
        Ok(AddressBook {
            id: r.get(0)?,
            kind: r.get(1)?,
            name: r.get(2)?,
            url: r.get(3)?,
            username: r.get(4)?,
            account_id: r.get(5)?,
            cards: r.get(6)?,
            last_sync: r.get(7)?,
            error: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn book(store: &Store, id: i64) -> Result<AddressBook> {
    books(store)?
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| Error::NotFound(tr!("Das Adressbuch gibt es nicht mehr.", "The address book is gone.").into()))
}

/// Adds an address book; returns its id.
pub fn add_book(
    store: &Store,
    kind: &str,
    name: &str,
    url: &str,
    username: &str,
    account_id: Option<i64>,
) -> Result<i64> {
    store.conn().execute(
        "INSERT INTO address_books (kind, name, url, username, account_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![kind, name.trim(), url.trim(), username.trim(), account_id, now()],
    )?;
    Ok(store.conn().last_insert_rowid())
}

/// The Microsoft contacts of an account, if they are on.
pub fn graph_book(store: &Store, account_id: i64) -> Result<Option<i64>> {
    Ok(store
        .conn()
        .query_row("SELECT id FROM address_books WHERE kind = 'graph' AND account_id = ?1", [account_id], |r| r.get(0))
        .optional()?)
}

pub fn remove_book(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("DELETE FROM address_books WHERE id = ?1", [id])?;
    Ok(())
}

/// The stored cards of a book: href and ETag (for a sync).
pub fn known(store: &Store, book_id: i64) -> Result<Vec<(String, Option<String>)>> {
    let mut st = store.conn().prepare("SELECT href, etag FROM cards WHERE book_id = ?1")?;
    let rows = st.query_map([book_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Writes what a sync found.
pub fn apply(store: &Store, book_id: i64, changed: &[Card], removed: &[String]) -> Result<()> {
    store.batch(|| {
        for href in removed {
            store.conn().execute("DELETE FROM cards WHERE book_id = ?1 AND href = ?2", params![book_id, href])?;
        }
        for c in changed {
            let (photo_type, photo) = match &c.photo {
                Some((m, d)) => (Some(m.as_str()), Some(d.as_slice())),
                None => (None, None),
            };
            store.conn().execute(
                "INSERT INTO cards (book_id, href, etag, name, emails, phones, org, photo, photo_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (book_id, href) DO UPDATE SET etag = excluded.etag, name = excluded.name, emails = excluded.emails,
                   phones = excluded.phones, org = excluded.org, photo = excluded.photo, photo_type = excluded.photo_type",
                params![
                    book_id,
                    c.href,
                    c.etag,
                    c.name,
                    serde_json::to_string(&c.emails)?,
                    serde_json::to_string(&c.phones)?,
                    c.org,
                    photo,
                    photo_type
                ],
            )?;
            let id: i64 =
                store.conn().query_row("SELECT id FROM cards WHERE book_id = ?1 AND href = ?2", params![book_id, c.href], |r| r.get(0))?;
            store.conn().execute("DELETE FROM card_emails WHERE card_id = ?1", [id])?;
            for e in &c.emails {
                store.conn().execute("INSERT OR IGNORE INTO card_emails (email, card_id) VALUES (?1, ?2)", params![e.trim(), id])?;
            }
        }
        store.conn().execute("UPDATE address_books SET last_sync = ?2, error = NULL WHERE id = ?1", params![book_id, now()])?;
        Ok(())
    })
}

/// Notes why a sync failed.
pub fn failed(store: &Store, book_id: i64, error: &str) -> Result<()> {
    store.conn().execute("UPDATE address_books SET error = ?2 WHERE id = ?1", params![book_id, error])?;
    Ok(())
}

/// Suggestions for an address field: the cards and the people of the mail, the ones the user
/// wrote to first, each address once.
pub fn suggest(store: &Store, prefix: &str, limit: u32) -> Result<Vec<Contact>> {
    let p = prefix.trim();
    if p.is_empty() {
        return Ok(vec![]);
    }
    let like = format!("{}%", p.replace(['%', '_'], ""));
    let word = format!("% {}%", p.replace(['%', '_'], ""));
    let mut st = store.conn().prepare(
        "SELECT email, MAX(name), MAX(rank) FROM (
           SELECT c.email AS email, c.name AS name, c.sent * 1000 + c.received + 500 * EXISTS (SELECT 1 FROM card_emails e WHERE e.email = c.email) AS rank, c.last_used AS used
             FROM contacts c WHERE c.email LIKE ?1 OR c.name LIKE ?1 OR c.name LIKE ?2
           UNION ALL
           SELECT e.email, k.name, 500, 0 FROM card_emails e JOIN cards k ON k.id = e.card_id
             WHERE e.email LIKE ?1 OR k.name LIKE ?1 OR k.name LIKE ?2)
         GROUP BY lower(email) ORDER BY MAX(rank) DESC, MAX(used) DESC, email LIMIT ?3",
    )?;
    let rows = st.query_map(params![like, word, limit], |r| {
        Ok(Contact { email: r.get(0)?, name: r.get::<_, Option<String>>(1)?.filter(|n| !n.trim().is_empty()) })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// An attachment of a contact's mail, for the card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardFile {
    pub id: i64,
    pub message_id: i64,
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: u64,
    pub date: i64,
}

/// The contact card of an address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactCard {
    pub email: String,
    pub name: Option<String>,
    pub org: Option<String>,
    pub phones: Vec<String>,
    /// The other addresses of the same card.
    pub emails: Vec<String>,
    /// The address book it comes from.
    pub book: Option<String>,
    /// Messages from them, and from the user to them.
    pub received: u32,
    pub sent: u32,
    pub recent: Vec<MessageSummary>,
    pub files: Vec<CardFile>,
}

/// Everything the app knows about an address.
pub fn card(store: &Store, email: &str) -> Result<ContactCard> {
    let email = email.trim().to_string();
    let local: Option<(Option<String>, u32, u32)> = store
        .conn()
        .query_row("SELECT name, received, sent FROM contacts WHERE email = ?1", [&email], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let card: Option<(String, String, String, Option<String>, String)> = store
        .conn()
        .query_row(
            "SELECT k.name, k.emails, k.phones, k.org, b.name FROM card_emails e JOIN cards k ON k.id = e.card_id
             JOIN address_books b ON b.id = k.book_id WHERE e.email = ?1 LIMIT 1",
            [&email],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let (received, sent) = local.as_ref().map_or((0, 0), |l| (l.1, l.2));
    let mut out = ContactCard {
        email: email.clone(),
        name: local.and_then(|l| l.0).filter(|n| !n.trim().is_empty()),
        org: None,
        phones: vec![],
        emails: vec![],
        book: None,
        received,
        sent,
        recent: vec![],
        files: vec![],
    };
    if let Some((name, emails, phones, org, book)) = card {
        if !name.trim().is_empty() {
            out.name = Some(name);
        }
        out.emails = serde_json::from_str::<Vec<String>>(&emails)?
            .into_iter()
            .filter(|e| !e.eq_ignore_ascii_case(&email))
            .collect();
        out.phones = serde_json::from_str(&phones)?;
        out.org = org;
        out.book = Some(book);
    }
    if out.name.is_none() {
        out.name = store
            .conn()
            .query_row(
                "SELECT from_name FROM messages WHERE from_email = ?1 COLLATE NOCASE AND from_name <> '' ORDER BY internal_date DESC LIMIT 1",
                [&email],
                |r| r.get(0),
            )
            .optional()?;
    }
    let to_like = format!("%\"email\":\"{}\"%", email.to_ascii_lowercase().replace('%', ""));
    let mut st = store.conn().prepare(&format!(
        "SELECT {SUMMARY_COLS} FROM messages m WHERE m.id IN (
           SELECT MIN(x.id) FROM messages x JOIN mailboxes b ON b.id = x.mailbox_id
           WHERE x.hidden = 0 AND COALESCE(b.role, '') NOT IN ('trash', 'junk', 'all', 'flagged', 'important')
             AND (x.from_email = ?1 COLLATE NOCASE OR lower(x.to_json) LIKE ?2 OR lower(x.cc_json) LIKE ?2)
           GROUP BY COALESCE(x.message_id, x.id))
         ORDER BY COALESCE(m.date, m.internal_date) DESC LIMIT 8"
    ))?;
    out.recent = st.query_map(params![email, to_like], summary)?.collect::<rusqlite::Result<_>>()?;
    let mut st = store.conn().prepare(
        "SELECT a.id, a.message_id, a.filename, a.mime_type, a.size, COALESCE(m.date, m.internal_date)
         FROM attachments a JOIN messages m ON m.id = a.message_id
         WHERE a.inline = 0 AND m.hidden = 0 AND m.from_email = ?1 COLLATE NOCASE
         GROUP BY COALESCE(m.message_id, m.id), a.part_index
         ORDER BY COALESCE(m.date, m.internal_date) DESC LIMIT 12",
    )?;
    out.files = st
        .query_map([&email], |r| {
            Ok(CardFile {
                id: r.get(0)?,
                message_id: r.get(1)?,
                filename: r.get(2)?,
                mime_type: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                date: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Address, parse};
    use crate::store::tests::{header, store_with_inbox};

    fn card(name: &str, emails: &[&str]) -> Card {
        Card {
            href: format!("/{name}.vcf"),
            etag: Some("e1".into()),
            name: name.into(),
            emails: emails.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn suggestions_merge_cards_and_mail() {
        let (s, _, _) = store_with_inbox();
        s.touch_contact(&Address { name: Some("Lea Weber".into()), email: "lea@example.org".into() }, true, 10)
            .unwrap();
        s.touch_contact(&Address { name: Some("Leander".into()), email: "leander@example.com".into() }, false, 20)
            .unwrap();
        let b = add_book(&s, "carddav", "iCloud", "https://contacts.icloud.com", "maria", None).unwrap();
        apply(
            &s,
            b,
            &[
                card("Lea Weber", &["lea@example.org", "lea.weber@firma.example"]),
                card("Leo Hahn", &["leo@hahn.example"]),
            ],
            &[],
        )
        .unwrap();
        let got: Vec<String> = suggest(&s, "le", 10).unwrap().into_iter().map(|c| c.email).collect();
        assert_eq!(got[0], "lea@example.org", "wrote to and in the address book: {got:?}");
        assert!(got.contains(&"lea.weber@firma.example".into()) && got.contains(&"leo@hahn.example".into()));
        assert_eq!(got.iter().filter(|e| e.as_str() == "lea@example.org").count(), 1);
        assert_eq!(suggest(&s, "hahn", 10).unwrap()[0].name.as_deref(), Some("Leo Hahn"), "by a word of the name");
        // A second sync: one card gone, one changed.
        let changed = Card { href: "/Lea Weber.vcf".into(), ..card("Lea W.", &["lea@example.org"]) };
        apply(&s, b, &[changed], &["/Leo Hahn.vcf".to_string()]).unwrap();
        assert!(suggest(&s, "leo", 10).unwrap().is_empty());
        assert_eq!(books(&s).unwrap()[0].cards, 1);
        assert!(suggest(&s, "  ", 10).unwrap().is_empty());
    }

    #[test]
    fn contact_cards_show_recent_mail_and_files() {
        let (s, _, inbox) = store_with_inbox();
        let raw = "From: Lea Weber <lea@example.org>\r\nTo: maria@example.org\r\nSubject: Angebot\r\nMessage-ID: <c1@x>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/plain\r\n\r\nAnbei.\r\n--B\r\nContent-Type: application/pdf; name=\"angebot.pdf\"\r\nContent-Disposition: attachment; filename=\"angebot.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0x\r\n--B--\r\n";
        let p = parse(raw.as_bytes()).unwrap();
        let id = s.insert_message(inbox, &header(1, raw.as_bytes()), &p).unwrap();
        s.set_body(id, &p).unwrap();
        let mine = "From: Maria <maria@example.org>\r\nTo: Lea Weber <lea@example.org>\r\nSubject: Re: Angebot\r\nMessage-ID: <c2@x>\r\n\r\nDanke.\r\n";
        let p2 = parse(mine.as_bytes()).unwrap();
        s.insert_message(inbox, &header(2, mine.as_bytes()), &p2).unwrap();
        let b = add_book(&s, "graph", "Kontakte", "", "", None).unwrap();
        apply(
            &s,
            b,
            &[Card {
                phones: vec!["+49 1".into()],
                org: Some("Weber GmbH".into()),
                ..card("Lea Weber", &["lea@example.org", "lea@weber.example"])
            }],
            &[],
        )
        .unwrap();
        let c = super::card(&s, "LEA@example.org").unwrap();
        assert_eq!(c.name.as_deref(), Some("Lea Weber"));
        assert_eq!(c.org.as_deref(), Some("Weber GmbH"));
        assert_eq!(c.emails, ["lea@weber.example"]);
        assert_eq!(c.book.as_deref(), Some("Kontakte"));
        assert_eq!(c.recent.len(), 2, "from them and to them");
        assert_eq!(c.files.len(), 1);
        assert_eq!(c.files[0].filename.as_deref(), Some("angebot.pdf"));
        let unknown = super::card(&s, "nobody@example.org").unwrap();
        assert!(unknown.name.is_none() && unknown.recent.is_empty());
    }
}
