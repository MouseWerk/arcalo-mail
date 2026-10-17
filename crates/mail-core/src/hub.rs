//! The attachment hub: every attachment of the stored mail in one list, newest first, each once
//! (a message in two folders shows its files once), filtered by kind, sender, time and name.
//! Opening, saving and the preview go through the same paths as in the reading pane.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::parse::Address;
use crate::store::Store;

/// What kind of file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Images,
    Pdf,
    Documents,
    Spreadsheets,
    Presentations,
    Archives,
    Calendar,
    Other,
}

impl FileKind {
    pub const ALL: [FileKind; 8] = [
        FileKind::Images,
        FileKind::Pdf,
        FileKind::Documents,
        FileKind::Spreadsheets,
        FileKind::Presentations,
        FileKind::Archives,
        FileKind::Calendar,
        FileKind::Other,
    ];

    /// The kind of a file by its type and name.
    pub fn of(mime: &str, name: &str) -> FileKind {
        let m = mime.to_ascii_lowercase();
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        let any = |list: &[&str]| list.contains(&ext.as_str());
        if m.starts_with("image/") || any(&["png", "jpg", "jpeg", "gif", "webp", "heic", "svg", "bmp", "tif", "tiff"]) {
            FileKind::Images
        } else if m == "application/pdf" || ext == "pdf" {
            FileKind::Pdf
        } else if m.contains("spreadsheet")
            || m.contains("excel")
            || m == "text/csv"
            || any(&["xls", "xlsx", "ods", "csv", "numbers"])
        {
            FileKind::Spreadsheets
        } else if m.contains("presentation") || m.contains("powerpoint") || any(&["ppt", "pptx", "odp", "key"]) {
            FileKind::Presentations
        } else if m.contains("word")
            || m.contains("opendocument.text")
            || m == "text/plain"
            || m == "application/rtf"
            || any(&["doc", "docx", "odt", "rtf", "txt", "md", "pages"])
        {
            FileKind::Documents
        } else if m.contains("zip")
            || m.contains("compressed")
            || m.contains("tar")
            || any(&["zip", "7z", "rar", "gz", "tgz", "tar"])
        {
            FileKind::Archives
        } else if m == "text/calendar" || ext == "ics" {
            FileKind::Calendar
        } else {
            FileKind::Other
        }
    }
}

/// The hub's filter; empty fields filter nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub kind: Option<FileKind>,
    /// The sender's address.
    pub sender: Option<String>,
    /// Unix seconds.
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Words of the file name.
    pub query: String,
    pub account_id: Option<i64>,
}

/// One file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub id: i64,
    pub message_id: i64,
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: u64,
    pub kind: FileKind,
    pub date: i64,
    pub from: Option<Address>,
    pub subject: String,
    pub account_id: i64,
}

/// A sender of files, for the filter's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sender {
    pub email: String,
    pub name: Option<String>,
    pub files: u32,
}

const BASE: &str = "SELECT a.id, a.message_id, a.filename, a.mime_type, a.size, COALESCE(m.date, m.internal_date) AS at,
       m.from_name, m.from_email, m.subject, m.account_id,
       ROW_NUMBER() OVER (PARTITION BY m.account_id, COALESCE(m.message_id, 'local:' || m.id), a.part_index ORDER BY m.id) AS copy
     FROM attachments a JOIN messages m ON m.id = a.message_id JOIN mailboxes b ON b.id = m.mailbox_id
     WHERE a.inline = 0 AND m.hidden = 0 AND COALESCE(b.role, '') NOT IN ('trash', 'junk')
       AND (?1 IS NULL OR m.account_id = ?1)";

/// A page of files and how many there are.
pub fn list(store: &Store, f: &Filter, limit: u32, offset: u32) -> Result<(u32, Vec<Item>)> {
    let mut st = store.conn().prepare(&format!(
        "SELECT * FROM ({BASE}) WHERE copy = 1 AND (?2 IS NULL OR from_email = ?2 COLLATE NOCASE)
           AND (?3 IS NULL OR at >= ?3) AND (?4 IS NULL OR at < ?4)
         ORDER BY at DESC, id DESC"
    ))?;
    let words: Vec<String> = f.query.split_whitespace().map(|w| crate::search::fold(&w.to_lowercase())).collect();
    let rows = st.query_map(params![f.account_id, f.sender.as_deref().map(str::trim), f.since, f.until], |r| {
        let email: Option<String> = r.get(7)?;
        // Shown without bidirectional controls, so the real extension is what the user reads.
        let filename: Option<String> = r.get::<_, Option<String>>(2)?.as_deref().map(crate::files::display_name);
        let mime: String = r.get(3)?;
        Ok(Item {
            id: r.get(0)?,
            message_id: r.get(1)?,
            kind: FileKind::of(&mime, filename.as_deref().unwrap_or_default()),
            filename,
            mime_type: mime,
            size: r.get::<_, i64>(4)? as u64,
            date: r.get(5)?,
            from: email.map(|email| Address { name: r.get::<_, Option<String>>(6).ok().flatten(), email }),
            subject: r.get(8)?,
            account_id: r.get(9)?,
        })
    })?;
    let mut all = vec![];
    for item in rows {
        let item = item?;
        if f.kind.is_some_and(|k| k != item.kind) {
            continue;
        }
        if !words.is_empty() {
            let name = crate::search::fold(&item.filename.clone().unwrap_or_default().to_lowercase());
            if !words.iter().all(|w| name.contains(w.as_str())) {
                continue;
            }
        }
        all.push(item);
    }
    let total = all.len() as u32;
    Ok((total, all.into_iter().skip(offset as usize).take(limit as usize).collect()))
}

/// The senders of files, the most first.
pub fn senders(store: &Store, account_id: Option<i64>) -> Result<Vec<Sender>> {
    let mut st = store.conn().prepare(&format!(
        "SELECT from_email, MAX(from_name), COUNT(*) FROM ({BASE}) WHERE copy = 1 AND from_email IS NOT NULL
         GROUP BY lower(from_email) ORDER BY COUNT(*) DESC, lower(from_email) LIMIT 50"
    ))?;
    let rows = st.query_map([account_id], |r| Ok(Sender { email: r.get(0)?, name: r.get(1)?, files: r.get(2)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::MailboxEntry;
    use crate::parse::parse;
    use crate::store::tests::{header, store_with_inbox};

    fn with_files(s: &Store, mailbox: i64, uid: u32, from: &str, id: &str, files: &[(&str, &str)]) -> i64 {
        let mut raw = format!(
            "From: {from}\r\nTo: maria@example.org\r\nSubject: Dateien {uid}\r\nMessage-ID: <{id}>\r\nDate: Fri, 16 Oct 2026 09:{uid:02}:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n--B\r\nContent-Type: text/plain\r\n\r\nAnbei.\r\n"
        );
        for (name, mime) in files {
            raw.push_str(&format!("--B\r\nContent-Type: {mime}; name=\"{name}\"\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\nAAAA\r\n"));
        }
        raw.push_str("--B--\r\n");
        let p = parse(raw.as_bytes()).unwrap();
        let mid = s.insert_message(mailbox, &header(uid, raw.as_bytes()), &p).unwrap();
        s.set_body(mid, &p).unwrap();
        mid
    }

    #[test]
    fn every_file_once_filtered_by_kind_sender_time_and_name() {
        let (s, account, inbox) = store_with_inbox();
        let archive = s
            .upsert_mailbox(
                account,
                &MailboxEntry { name: "Archive".into(), delimiter: None, attributes: vec!["\\Archive".into()] },
            )
            .unwrap();
        with_files(
            &s,
            inbox,
            1,
            "Lea <lea@example.org>",
            "a@x",
            &[("Angebot Website.pdf", "application/pdf"), ("foto.jpg", "image/jpeg")],
        );
        // The same message in the archive: its files are not listed twice.
        with_files(
            &s,
            archive,
            1,
            "Lea <lea@example.org>",
            "a@x",
            &[("Angebot Website.pdf", "application/pdf"), ("foto.jpg", "image/jpeg")],
        );
        with_files(
            &s,
            inbox,
            2,
            "Jonas <jonas@example.org>",
            "b@x",
            &[("Zahlen Q3.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")],
        );
        let (total, items) = list(&s, &Filter::default(), 50, 0).unwrap();
        assert_eq!(total, 3);
        assert_eq!(items[0].filename.as_deref(), Some("Zahlen Q3.xlsx"), "newest first");
        assert_eq!(items[0].kind, FileKind::Spreadsheets);
        let only = |f: Filter| {
            list(&s, &f, 50, 0).unwrap().1.into_iter().map(|i| i.filename.unwrap_or_default()).collect::<Vec<_>>()
        };
        assert_eq!(only(Filter { kind: Some(FileKind::Pdf), ..Default::default() }), ["Angebot Website.pdf"]);
        assert_eq!(only(Filter { sender: Some("LEA@example.org".into()), ..Default::default() }).len(), 2);
        assert_eq!(only(Filter { query: "angebot web".into(), ..Default::default() }), ["Angebot Website.pdf"]);
        let t = 1_792_141_290; // 09:01:30
        assert_eq!(only(Filter { since: Some(t), ..Default::default() }), ["Zahlen Q3.xlsx"]);
        assert_eq!(only(Filter { until: Some(t), ..Default::default() }).len(), 2);
        let (_, page) = list(&s, &Filter::default(), 1, 1).unwrap();
        assert_eq!(page.len(), 1);
        let who = senders(&s, None).unwrap();
        assert_eq!((who[0].email.as_str(), who[0].files), ("lea@example.org", 2));
        assert_eq!(FileKind::of("application/octet-stream", "plan.docx"), FileKind::Documents);
        assert_eq!(FileKind::of("text/calendar", "invite.ics"), FileKind::Calendar);
    }
}
