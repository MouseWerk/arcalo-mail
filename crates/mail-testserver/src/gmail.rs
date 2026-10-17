//! A mailbox shaped like a German Gmail account, for the tests that guard what a real Gmail
//! account showed on a phone: the `[Gmail]/…` folders with localized names and SPECIAL-USE
//! attributes, labels with brackets and nesting (`[Notion]`, `[Notion]/Inbox`), a label that has
//! the name of a special folder (`Entwürfe`, made by an earlier client), the same message in
//! INBOX, „Alle Nachrichten“ and a label, thousands of messages from 2015 until now, HTML mail
//! with pictures inside (`cid:`) and from the web (`https://bilder.zeit.example/…`), receipts
//! and newsletters as their senders write them, and old large messages whose bodies the server hands out slowly
//! (`Shared::slow_old_body_ms`).
//!
//! `X-GM-EXT-1` is offered and `X-GM-MSGID`/`X-GM-THRID` answered (the same id for a message in
//! every folder), as Gmail does; Gmail has CONDSTORE but no QRESYNC.

use mail_core::imap::fake::{FakeImap, FakeMailbox};
use serde::Serialize;

/// The folders of the account: name and SPECIAL-USE attributes.
pub const FOLDERS: &[(&str, &[&str])] = &[
    ("INBOX", &[]),
    ("[Gmail]", &["\\Noselect"]),
    ("[Gmail]/Alle Nachrichten", &["\\All"]),
    ("[Gmail]/Entwürfe", &["\\Drafts"]),
    ("[Gmail]/Gesendet", &["\\Sent"]),
    ("[Gmail]/Markiert", &["\\Flagged"]),
    ("[Gmail]/Papierkorb", &["\\Trash"]),
    ("[Gmail]/Spam", &["\\Junk"]),
    ("[Gmail]/Wichtig", &["\\Important"]),
    ("[Notion]", &[]),
    ("[Notion]/Inbox", &[]),
    ("Belege", &[]),
    // A label an earlier mail program made for its drafts: a folder of its own, not a second
    // drafts folder.
    ("Entwürfe", &[]),
];

pub const ALL_MAIL: &str = "[Gmail]/Alle Nachrichten";

/// 2015-01-02 08:00 UTC: the oldest message.
const START: i64 = 1_420_185_600;

/// Old messages from this size on come slowly (`Shared::slow_old_body_ms`).
pub const LARGE: usize = 200_000;
/// … when they are older than this (seconds).
pub const OLD: i64 = 365 * 86_400;

/// A 24×24 red PNG (the picture inside the HTML messages).
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAABgAAAAYCAIAAABvFaqvAAAAH0lEQVR42mO4Y2NDFcQwatCoQaMGjRo0atCoQQNvEADETf0fDf9fSAAAAABJRU5ErkJggg==";

/// What the tests check against.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    /// Messages per folder.
    pub counts: std::collections::BTreeMap<String, usize>,
    /// The subjects of the newest inbox messages, newest first.
    pub newest: Vec<String>,
    pub oldest: String,
    /// An old message larger than [`LARGE`]: its body comes slowly and is never fetched ahead.
    pub old_large: String,
    /// HTML with a picture inside (`cid:`).
    pub cid: String,
    /// A newsletter with a picture from the web (blocked until the user loads it).
    pub remote: String,
    /// The PlayStation receipt, in INBOX, „Belege“ and „Alle Nachrichten“.
    pub playstation: String,
    /// The folder names as the server lists them.
    pub folders: Vec<String>,
    /// Recent inbox messages and the inbox tab each belongs in.
    pub expected: Vec<Expected>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Expected {
    pub subject: String,
    pub from: String,
    pub message_id: String,
    pub category: &'static str,
}

/// `X-GM-MSGID` of a message: the same in every folder (from its Message-ID).
pub fn gm_msgid(raw: &[u8]) -> u64 {
    let id = mail_core::parse::parse(raw).ok().and_then(|p| p.message_id).unwrap_or_default();
    // FNV-1a, kept in Gmail's range (a 64-bit number below 2^63).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h >> 1
}

struct Mail {
    from: String,
    subject: String,
    date: i64,
    headers: Vec<String>,
    body: Body,
    folders: Vec<&'static str>,
    category: &'static str,
    seen: bool,
}

enum Body {
    Text(String),
    /// HTML only (as most shops send it).
    Html(String),
    /// Text and HTML with a picture inside (`cid:`).
    Related(String),
    /// Text and a large attachment.
    Large {
        text: String,
        size: usize,
    },
}

const PEOPLE: &[&str] = &[
    "Jonas Berger <jonas.berger@gmail.com>",
    "Lea Sommer <lea.sommer@web.de>",
    "Paul Wagner <paul@wagner-familie.de>",
    "Mia Krause <mia.krause@gmx.de>",
    "Tim Hoffmann <tim.hoffmann@posteo.de>",
    "Oma Gisela <gisela.muster@t-online.de>",
];

const TOPICS: &[&str] = &[
    "Wochenende",
    "Fotos vom Ausflug",
    "Kurze Frage",
    "Geburtstag",
    "Rezept",
    "Treffen nächste Woche",
    "Umzug",
    "Urlaubsplanung",
];

fn date_header(secs: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let d = crate::internal_date(secs);
    // "16-Oct-2026 07:30:00 +0000" → "Fri, 16 Oct 2026 07:30:00 +0000"
    let wd = DAYS[secs.div_euclid(86_400).rem_euclid(7) as usize];
    format!("{wd}, {}", d.replacen('-', " ", 2))
}

fn render(m: &Mail, id: &str, to: &str) -> Vec<u8> {
    let mut head = vec![
        format!("From: {}", m.from),
        format!("To: {to}"),
        format!("Subject: {}", m.subject),
        format!("Date: {}", date_header(m.date)),
        format!("Message-ID: <{id}>"),
    ];
    head.extend(m.headers.iter().cloned());
    head.push("MIME-Version: 1.0".into());
    let body = match &m.body {
        Body::Text(t) => {
            head.push("Content-Type: text/plain; charset=utf-8".into());
            head.push("Content-Transfer-Encoding: 8bit".into());
            t.clone()
        }
        Body::Html(h) => {
            head.push("Content-Type: text/html; charset=utf-8".into());
            head.push("Content-Transfer-Encoding: 8bit".into());
            h.clone()
        }
        Body::Related(text) => {
            head.push("Content-Type: multipart/alternative; boundary=\"ALT\"".into());
            let html = format!(
                "<html><body><p>{text}</p><p><img src=\"cid:logo-{id}\" width=\"24\" height=\"24\" alt=\"Logo\"></p></body></html>"
            );
            format!(
                "--ALT\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n\
--ALT\r\nContent-Type: multipart/related; boundary=\"REL\"\r\n\r\n\
--REL\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n\
--REL\r\nContent-Type: image/png; name=\"logo.png\"\r\nContent-Transfer-Encoding: base64\r\n\
Content-ID: <logo-{id}>\r\nContent-Disposition: inline; filename=\"logo.png\"\r\n\r\n{PNG_B64}\r\n--REL--\r\n--ALT--"
            )
        }
        Body::Large { text, size } => {
            head.push("Content-Type: multipart/mixed; boundary=\"MIX\"".into());
            let line = "QXJjYWxvIE1haWwgdGVzdCBkYXRhIGZvciBhIGxhcmdlIGF0dGFjaG1lbnQuIEFyY2FsbyBNYWls\r\n";
            let data = line.repeat(size / line.len() + 1);
            format!(
                "--MIX\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n\
--MIX\r\nContent-Type: application/pdf; name=\"scan.pdf\"\r\nContent-Disposition: attachment; filename=\"scan.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{data}--MIX--"
            )
        }
    };
    format!("{}\r\n\r\n{}\r\n", head.join("\r\n"), body.replace("\r\n", "\n").replace('\n', "\r\n")).into_bytes()
}

fn person(i: usize, date: i64) -> Mail {
    let topic = TOPICS[i % TOPICS.len()];
    Mail {
        from: PEOPLE[i % PEOPLE.len()].into(),
        subject: format!("{topic} ({i})"),
        date,
        headers: vec![],
        body: Body::Text(format!(
            "Hallo Maria,\n\nwie besprochen: {topic}. Sag mir einfach Bescheid, ob es bei dir passt.\n\nLiebe Grüße"
        )),
        folders: vec!["INBOX", ALL_MAIL],
        category: "people",
        seen: true,
    }
}

/// Its pictures come from the web (a host that is never reached: the tests check the address
/// the frame gets, not the picture).
fn newsletter(i: usize, date: i64) -> Mail {
    Mail {
        from: "ZEIT ONLINE <newsletter@zeit.de>".into(),
        subject: format!("Das Wichtigste am Morgen ({i})"),
        date,
        headers: vec![
            "List-Id: Der Morgen <morgen.newsletter.zeit.de>".into(),
            format!(
                "List-Unsubscribe: <https://abmelden.zeit.example/morgen/{i}>, <mailto:unsubscribe@zeit.de?subject=abmelden>"
            ),
            "List-Unsubscribe-Post: List-Unsubscribe=One-Click".into(),
            "Precedence: bulk".into(),
        ],
        body: Body::Html(format!(
            "<html><body><table width=\"600\"><tr><td><img src=\"https://bilder.zeit.example/morgen-{i}.png\" width=\"600\" height=\"120\" alt=\"ZEIT ONLINE\">\
<h1>Guten Morgen</h1><p>Die wichtigsten Nachrichten des Tages, kurz zusammengefasst.</p>\
<p><a href=\"https://www.zeit.de/\">Weiterlesen</a></p></td></tr></table></body></html>"
        )),
        folders: vec!["INBOX", ALL_MAIL],
        category: "newsletters",
        seen: true,
    }
}

fn receipt(i: usize, date: i64) -> Mail {
    let (from, subject, html, folders): (&str, String, String, Vec<&'static str>) = match i % 5 {
        0 => (
            "PlayStation <Sony@txn-email.playstation.com>",
            "Vielen Dank für deinen Kauf".into(),
            "<html><body><table><tr><td><h2>Vielen Dank für deinen Kauf</h2><p>Bestellnummer: \
T-1234-5678</p><p>Gesamtbetrag: 69,99 €</p><p>Sony Interactive Entertainment Europe</p></td></tr></table></body></html>"
                .into(),
            vec!["INBOX", ALL_MAIL, "Belege"],
        ),
        1 => (
            "Amazon.de <bestellbestaetigung@amazon.de>",
            format!("Deine Amazon.de-Bestellung von „Kopfhörer“ ({i})"),
            "<html><body><p>Hallo Maria, danke für deine Bestellung. Voraussichtliche Lieferung: Donnerstag.</p></body></html>"
                .into(),
            vec!["INBOX", ALL_MAIL, "Belege"],
        ),
        2 => (
            "PayPal <service@paypal.de>",
            format!("Du hast eine Zahlung an Steam gesendet ({i})"),
            "<html><body><p>Hallo Maria Muster, du hast 19,99 € EUR an Valve Corporation gesendet.</p>\
<p>Transaktionscode: 4AB12345CD678901E</p></body></html>"
                .into(),
            vec!["INBOX", ALL_MAIL],
        ),
        3 => (
            "Apple <no_reply@email.apple.com>",
            format!("Deine Rechnung von Apple ({i})"),
            "<html><body><p>iCloud+ mit 50 GB Speicher: 0,99 €</p></body></html>".into(),
            vec!["INBOX", ALL_MAIL, "Belege"],
        ),
        _ => (
            "DB Vertrieb GmbH <buchungsbestaetigung@bahn.de>",
            format!("Ihre Buchungsbestätigung (Auftrag {})", 400_000 + i),
            "<html><body><p>Vielen Dank für Ihre Buchung. Ihr Ticket finden Sie im Anhang.</p></body></html>".into(),
            vec!["INBOX", ALL_MAIL],
        ),
    };
    Mail {
        from: from.into(),
        subject,
        date,
        headers: vec![],
        body: Body::Html(html),
        folders,
        category: "receipts",
        seen: true,
    }
}

fn notion(i: usize, date: i64) -> Mail {
    Mail {
        from: "Notion <notify@mail.notion.so>".into(),
        subject: format!("Lea hat dich in „Haushalt“ erwähnt ({i})"),
        date,
        headers: vec!["Auto-Submitted: auto-generated".into()],
        body: Body::Text("Lea Sommer hat dich in einem Kommentar erwähnt.".into()),
        // A filter skips the inbox: the label and All Mail only.
        folders: vec!["[Notion]/Inbox", ALL_MAIL],
        category: "notifications",
        seen: true,
    }
}

fn github(i: usize, date: i64) -> Mail {
    Mail {
        from: "GitHub <noreply@github.com>".into(),
        subject: format!("[MouseWerk/arcalo-mail] Build erfolgreich ({i})"),
        date,
        headers: vec![],
        body: Body::Text("All checks have passed.".into()),
        folders: vec!["INBOX", ALL_MAIL],
        category: "notifications",
        seen: true,
    }
}

fn picture(i: usize, date: i64) -> Mail {
    Mail {
        from: PEOPLE[(i + 1) % PEOPLE.len()].into(),
        subject: format!("Unser neues Logo ({i})"),
        date,
        headers: vec![],
        body: Body::Related("So sieht das neue Logo aus. Gefällt es dir?".into()),
        folders: vec!["INBOX", ALL_MAIL],
        category: "people",
        seen: true,
    }
}

/// Fills `f` with the Gmail account: `count` messages (at least 50) from 2015 until `now`.
pub fn fill(f: &mut FakeImap, count: usize, now: i64, user: &str) -> Summary {
    let count = count.max(50);
    f.caps.gmail = true;
    f.caps.qresync = false;
    f.mailboxes.clear();
    for (name, attrs) in FOLDERS {
        f.mailboxes.insert((*name).to_string(), FakeMailbox::new(attrs));
    }
    let last = now - 3600;
    let at = |i: usize| START + (last - START) * i as i64 / (count as i64 - 1);
    let mut mails: Vec<(Mail, String)> = Vec::with_capacity(count + 8);
    let mut old_large = String::new();
    for i in 0..count {
        let date = at(i);
        let mut m = match i % 10 {
            4 => newsletter(i, date),
            5 => receipt(i / 10, date),
            6 => github(i, date),
            7 => picture(i, date),
            8 => notion(i, date),
            _ => person(i, date),
        };
        // Old, large messages: a scan from years ago.
        if now - date > 2 * OLD && i % 97 == 3 {
            m.subject = format!("Scan vom Amt ({i})");
            m.body = Body::Large { text: "Anbei der Scan.".into(), size: LARGE + 100_000 };
            m.category = "people";
            m.folders = vec!["INBOX", ALL_MAIL];
        }
        if i == count / 10 {
            m = person(i, date);
            m.subject = "Fotos vom Umzug 2016".into();
            m.body = Body::Large { text: "Hier die Fotos vom Umzug, als Archiv.".into(), size: LARGE + 400_000 };
            old_large = m.subject.clone();
        }
        if i == 0 {
            m = person(i, date);
            m.subject = "Ältester Brief aus 2015".into();
        }
        m.seen = now - date > 30 * 86_400;
        mails.push((m, format!("g{i}@fixture.gmail.example")));
    }
    // The newest ones: one of each kind, the newest person last.
    let fresh = |k: i64| now - 600 + k * 60;
    let specials = vec![
        {
            let mut m = receipt(1, fresh(0));
            m.subject = "Deine Amazon.de-Bestellung von „Kopfhörer“".into();
            m
        },
        {
            let mut m = receipt(2, fresh(1));
            m.subject = "Du hast eine Zahlung an Steam gesendet".into();
            m
        },
        {
            let mut m = receipt(3, fresh(2));
            m.subject = "Deine Rechnung von Apple".into();
            m
        },
        {
            let mut m = receipt(4, fresh(3));
            m.subject = "Ihre Buchungsbestätigung (Auftrag 123456)".into();
            m
        },
        {
            let mut m = newsletter(count, fresh(4));
            m.subject = "Herbst-Newsletter mit Bildern".into();
            m
        },
        {
            let mut m = picture(count, fresh(5));
            m.subject = "Logo im Text".into();
            m
        },
        receipt(0, fresh(6)),
        {
            let mut m = person(count, fresh(7));
            m.from = PEOPLE[1].into();
            m.subject = "Neueste Nachricht: Treffen heute um sechs".into();
            m.body = Body::Text("Hallo Maria,\n\nbleibt es bei heute um sechs am Markt?\n\nLea".into());
            m
        },
    ];
    for (k, mut m) in specials.into_iter().enumerate() {
        m.seen = false;
        mails.push((m, format!("new{k}@fixture.gmail.example")));
    }
    // A few drafts, sent messages and one message in the old drafts label.
    for (k, (folder, subject)) in [
        ("[Gmail]/Entwürfe", "Entwurf: Antwort an den Vermieter"),
        ("[Gmail]/Gesendet", "Re: Wochenende"),
        ("[Gmail]/Gesendet", "Fotos anbei"),
        ("Entwürfe", "Alter Entwurf aus Thunderbird"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut m = person(k, now - 86_400 * (k as i64 + 2));
        m.from = format!("Maria Muster <{user}>");
        m.subject = subject.into();
        m.folders = if folder.starts_with("[Gmail]/Gesendet") { vec![folder, ALL_MAIL] } else { vec![folder] };
        mails.push((m, format!("own{k}@fixture.gmail.example")));
    }

    let to = format!("Maria Muster <{user}>");
    let mut expected = vec![];
    // Delivered in date order: Gmail's UIDs grow with the arrival.
    mails.sort_by_key(|(m, _)| m.date);
    for (m, id) in &mails {
        let raw = render(m, id, &to);
        for folder in &m.folders {
            let uid = f.deliver(folder, &raw, m.date);
            let flags =
                mail_core::imap::Flags { seen: m.seen, draft: folder.contains("Entwürfe"), ..Default::default() };
            f.set_flags(folder, uid, flags);
        }
        if m.folders.contains(&"INBOX") && now - m.date < 3600 {
            expected.push(Expected {
                subject: m.subject.clone(),
                from: m.from.clone(),
                message_id: id.clone(),
                category: m.category,
            });
        }
    }
    let inbox: Vec<&(Mail, String)> = mails.iter().filter(|(m, _)| m.folders.contains(&"INBOX")).collect();
    Summary {
        counts: f.mailboxes.iter().map(|(k, v)| (k.clone(), v.messages.len())).collect(),
        newest: inbox.iter().rev().take(20).map(|(m, _)| m.subject.clone()).collect(),
        oldest: "Ältester Brief aus 2015".into(),
        old_large,
        cid: "Logo im Text".into(),
        remote: "Herbst-Newsletter mit Bildern".into(),
        playstation: "Vielen Dank für deinen Kauf".into(),
        folders: FOLDERS.iter().map(|(n, _)| (*n).to_string()).collect(),
        expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gmail_account() {
        let mut f = FakeImap::default();
        let now = 1_792_135_800;
        let s = fill(&mut f, 600, now, "maria@example.org");
        assert!(f.caps.gmail && !f.caps.qresync);
        assert_eq!(s.newest[0], "Neueste Nachricht: Treffen heute um sechs");
        assert!(s.counts["[Gmail]/Alle Nachrichten"] > s.counts["INBOX"]);
        // The same message in three folders, with one X-GM-MSGID.
        let raw_of = |folder: &str| {
            f.mailboxes[folder]
                .messages
                .values()
                .find(|m| String::from_utf8_lossy(&m.raw).contains("Subject: Vielen Dank für deinen Kauf"))
                .map(|m| m.raw.clone())
                .unwrap()
        };
        let ids: Vec<u64> = ["INBOX", ALL_MAIL, "Belege"].iter().map(|b| gm_msgid(&raw_of(b))).collect();
        assert!(ids.windows(2).all(|w| w[0] == w[1]));
        // The picture inside is referenced from the HTML.
        let p = f.mailboxes["INBOX"]
            .messages
            .values()
            .find(|m| String::from_utf8_lossy(&m.raw).contains("Subject: Logo im Text"))
            .unwrap();
        let text = String::from_utf8_lossy(&p.raw);
        assert!(text.contains("src=\"cid:logo-new5@fixture.gmail.example\""), "{text}");
        let parsed = mail_core::parse::parse(&p.raw).unwrap();
        assert!(parsed.html.is_some());
        // Old and large: slower than everything else.
        let large = f.mailboxes["INBOX"]
            .messages
            .values()
            .find(|m| m.raw.len() > LARGE && String::from_utf8_lossy(&m.raw[..400]).contains("Fotos vom Umzug 2016"))
            .unwrap();
        assert!(large.raw.len() > LARGE && now - large.internal_date > OLD);
        // UIDs in date order.
        let dates: Vec<i64> = f.mailboxes["INBOX"].messages.values().map(|m| m.internal_date).collect();
        assert!(dates.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(date_header(1_792_135_800), "Fri, 16 Oct 2026 07:30:00 +0000");
    }
}
