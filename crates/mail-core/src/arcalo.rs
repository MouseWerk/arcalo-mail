//! „An Arcalo senden“: a mail becomes a page or a task in Arcalo. The page is the mail as
//! Markdown (subject, sender, recipients, date, the text) with a link back to the message
//! (`mid:`, RFC 2392, by its Message-ID); the task is one line with that link. The shell hands
//! them to Arcalo's own command line (`arcalo pages create … --stdin`, `arcalo tasks add …`,
//! docs/cli.md in Arcalo) when it is on the PATH; without it the page is saved as a Markdown
//! file. Nothing goes over the network.

use serde::Serialize;

use crate::store::MessageDetail;

/// The link back to a message, by its Message-ID.
pub fn link(message_id: &str) -> String {
    let id = message_id.trim().trim_matches(['<', '>']);
    // Characters that would end a Markdown link or an address are escaped.
    let escaped: String = id
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'@'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("mid:{escaped}")
}

/// `24.10.2026 14:30` (German) or `2026-10-24 14:30`, in the device's zone.
fn when(secs: i64) -> String {
    use chrono::TimeZone;
    let t = chrono::Local.timestamp_opt(secs, 0).single().unwrap_or_default();
    match crate::i18n::lang() {
        crate::i18n::Lang::De => t.format("%d.%m.%Y %H:%M").to_string(),
        crate::i18n::Lang::En => t.format("%Y-%m-%d %H:%M").to_string(),
    }
}

fn person(a: &crate::parse::Address) -> String {
    match a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} <{}>", a.email),
        None => a.email.clone(),
    }
}

/// Text that starts a Markdown line safely (a `#` or `-` at the start of a mail line stays text).
fn markdown_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with(['#', '>']) || t.starts_with("- [") || t.starts_with("```") {
                format!("\\{l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What „An Arcalo senden“ hands over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Export {
    /// The page's title (the subject).
    pub title: String,
    /// The page as Markdown.
    pub markdown: String,
    /// The task's line.
    pub task: String,
    /// A file name for the Markdown file (`2026-10-24 Angebot.md`).
    pub file_name: String,
}

/// The page and the task of a message; `text`: its body as text (decrypted, if it was).
pub fn export(m: &MessageDetail, text: &str) -> Export {
    let s = &m.summary;
    let title = if s.subject.trim().is_empty() {
        crate::tr!("Ohne Betreff", "No subject").to_string()
    } else {
        s.subject.trim().to_string()
    };
    let from = s.from.as_ref().map(person).unwrap_or_default();
    let to = m.to.iter().map(person).collect::<Vec<_>>().join(", ");
    let date = when(s.date);
    let back = m.message_id.as_deref().map(link);
    let mut md = format!("# {title}\n\n");
    let (l_from, l_to, l_date, l_mail) = match crate::i18n::lang() {
        crate::i18n::Lang::De => ("Von", "An", "Datum", "E-Mail"),
        crate::i18n::Lang::En => ("From", "To", "Date", "E-mail"),
    };
    if !from.is_empty() {
        md.push_str(&format!("- **{l_from}:** {from}\n"));
    }
    if !to.is_empty() {
        md.push_str(&format!("- **{l_to}:** {to}\n"));
    }
    md.push_str(&format!("- **{l_date}:** {date}\n"));
    if let Some(b) = &back {
        md.push_str(&format!("- **{l_mail}:** [{title}]({b})\n"));
    }
    md.push('\n');
    md.push_str(markdown_text(text.trim()).trim_end());
    md.push('\n');
    let sender = s
        .from
        .as_ref()
        .map(|a| a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| a.email.clone()))
        .unwrap_or_default();
    let day = chrono::DateTime::from_timestamp(s.date, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%d.%m.%Y").to_string())
        .unwrap_or_default();
    let label = format!("{l_mail}: {title} ({sender}, {day})").replace(['[', ']'], "");
    let task = match &back {
        Some(b) => format!("{title} [{label}]({b})"),
        None => title.clone(),
    };
    let safe: String = title.chars().map(|c| if "/\\:*?\"<>|".contains(c) { '-' } else { c }).collect();
    let file_day = chrono::DateTime::from_timestamp(s.date, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    Export { file_name: format!("{file_day} {}.md", safe.trim()).trim().to_string(), title, markdown: md, task }
}

/// The program `arcalo` on the PATH (`arcalo.exe` on Windows), if there is one.
pub fn find_cli(path: Option<&std::ffi::OsStr>) -> Option<std::path::PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["arcalo.exe", "Arcalo.exe"] } else { &["arcalo"] };
    for dir in std::env::split_paths(path?) {
        for n in names {
            let p = dir.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// What Arcalo answered: the page's title or the task's text, for the confirmation.
pub fn answer(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    v["path"].as_str().or(v["title"].as_str()).or(v["text"].as_str()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Address;
    use crate::store::MessageSummary;

    fn detail() -> MessageDetail {
        MessageDetail {
            summary: MessageSummary {
                subject: "Angebot [Q4]".into(),
                from: Some(Address { name: Some("Jonas Berger".into()), email: "jonas@example.org".into() }),
                date: 1_792_000_000,
                ..MessageSummary::default()
            },
            account_id: 1,
            to: vec![Address { name: None, email: "maria@example.org".into() }],
            cc: vec![],
            reply_to: vec![],
            message_id: Some("<abc 1@example.org>".into()),
            in_reply_to: None,
            references: vec![],
            size: 0,
            draft: false,
            has_body: true,
            text: None,
            html: None,
            remote_allowed: false,
            attachments: vec![],
            list_id: None,
            unsubscribe: None,
            one_click: false,
        }
    }

    #[test]
    fn a_mail_as_a_page_and_a_task() {
        // The tests run in English (the default of backend texts).
        let e = export(&detail(), "Hallo Maria,\n# kein Titel\n- [ ] keine Aufgabe\nGrüße");
        assert!(
            e.markdown.starts_with(
                "# Angebot [Q4]\n\n- **From:** Jonas Berger <jonas@example.org>\n- **To:** maria@example.org\n"
            ),
            "{}",
            e.markdown
        );
        assert!(e.markdown.contains("](mid:abc%201@example.org)"), "{}", e.markdown);
        assert!(
            e.markdown.contains("\n\\# kein Titel\n\\- [ ] keine Aufgabe\n"),
            "mail lines stay text: {}",
            e.markdown
        );
        assert!(e.task.starts_with("Angebot [Q4] [E-mail: Angebot Q4 (Jonas Berger, "), "{}", e.task);
        assert!(e.task.ends_with("](mid:abc%201@example.org)"));
        assert!(e.file_name.ends_with(" Angebot [Q4].md") && !e.file_name.contains('/'));
        assert_eq!(
            answer(r#"{"id": 42, "title": "Angebot [Q4]", "path": "Inbox/Angebot [Q4]"}"#).as_deref(),
            Some("Inbox/Angebot [Q4]")
        );
        assert_eq!(answer(r#"{"page_id": 3, "text": "Angebot"}"#).as_deref(), Some("Angebot"));
        assert!(answer("kein json").is_none());
    }

    #[test]
    fn finds_the_program_on_the_path() {
        let dir = std::env::temp_dir().join(format!("arcalo-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_cli(Some(dir.as_os_str())).is_none());
        let name = if cfg!(windows) { "arcalo.exe" } else { "arcalo" };
        std::fs::write(dir.join(name), "#!/bin/sh\n").unwrap();
        let path = std::env::join_paths([std::path::PathBuf::from("/nowhere"), dir.clone()]).unwrap();
        assert_eq!(find_cli(Some(&path)), Some(dir.join(name)));
        assert!(find_cli(None).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
