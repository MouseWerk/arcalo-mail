//! Tasks and dates from a message: the model's JSON read leniently, and what the user makes of
//! an item: a reminder at its time (`later::remind_task`) or a calendar entry to open in the
//! calendar app (an iCalendar file, RFC 5545, made here; nothing is sent anywhere).

use serde::{Deserialize, Serialize};

use super::{civil, days_from_civil};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Task,
    Event,
}

/// One task or appointment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    /// `YYYY-MM-DD`.
    pub date: Option<String>,
    /// `HH:MM`.
    pub time: Option<String>,
    pub end: Option<String>,
    pub place: Option<String>,
    /// The moment it is due (Unix seconds, local time as given by `offset`), for a reminder.
    pub at: Option<i64>,
}

fn date_parts(s: &str) -> Option<(i64, u32, u32)> {
    let mut it = s.trim().splitn(3, '-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.get(..2)?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d) && (1970..=2200).contains(&y)).then_some((y, m, d))
}

fn time_parts(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.get(..2)?.parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

/// The items of a model's answer; `offset`: the user's offset from UTC in seconds (the times
/// in a message are local).
pub fn parse(answer: &str, offset: i64) -> Vec<Item> {
    let Some(v) = super::prompts::json_in(answer) else { return vec![] };
    let list = v["items"].as_array().cloned().unwrap_or_default();
    let text =
        |x: &serde_json::Value| x.as_str().map(str::trim).filter(|s| !s.is_empty() && *s != "null").map(str::to_owned);
    list.iter()
        .filter_map(|i| {
            let title = text(&i["title"])?;
            let kind = if i["kind"].as_str() == Some("event") { Kind::Event } else { Kind::Task };
            let date = text(&i["date"]).filter(|d| date_parts(d).is_some());
            let time = text(&i["time"]).filter(|t| time_parts(t).is_some());
            let end = text(&i["end"]).filter(|t| time_parts(t).is_some());
            let at = date.as_deref().and_then(date_parts).map(|(y, m, d)| {
                let (h, min) = time.as_deref().and_then(time_parts).unwrap_or((9, 0));
                days_from_civil(y, m, d) * 86400 + (h * 3600 + min * 60) as i64 - offset
            });
            Some(Item { kind, title, date, time, end, place: text(&i["place"]), at })
        })
        .take(20)
        .collect()
}

fn ics_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n")
}

/// An iCalendar file with one appointment (a whole day without a time), for the calendar app.
/// `now`: for DTSTAMP; `uid_seed`: makes the UID stable for the same item.
pub fn ics(item: &Item, description: &str, now: i64, uid_seed: &str) -> Option<String> {
    let (y, m, d) = date_parts(item.date.as_deref()?)?;
    let stamp = {
        let (yy, mm, dd, _) = civil(now);
        let t = now.rem_euclid(86400);
        format!("{yy:04}{mm:02}{dd:02}T{:02}{:02}{:02}Z", t / 3600, t % 3600 / 60, t % 60)
    };
    let (start, end) = match item.time.as_deref().and_then(time_parts) {
        Some((h, min)) => {
            let (eh, emin) = item.end.as_deref().and_then(time_parts).unwrap_or(((h + 1).min(23), min));
            (
                format!("DTSTART:{y:04}{m:02}{d:02}T{h:02}{min:02}00"),
                format!("DTEND:{y:04}{m:02}{d:02}T{eh:02}{emin:02}00"),
            )
        }
        None => {
            let (ny, nm, nd, _) = civil((days_from_civil(y, m, d) + 1) * 86400);
            (format!("DTSTART;VALUE=DATE:{y:04}{m:02}{d:02}"), format!("DTEND;VALUE=DATE:{ny:04}{nm:02}{nd:02}"))
        }
    };
    let uid = {
        use sha2::{Digest, Sha256};
        let h = Sha256::digest(format!("{uid_seed}|{}|{}", item.title, item.date.as_deref().unwrap_or_default()));
        h.iter().take(12).map(|b| format!("{b:02x}")).collect::<String>()
    };
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//MouseWerk//Arcalo Mail//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{uid}@arcalo-mail"),
        format!("DTSTAMP:{stamp}"),
        start,
        end,
        format!("SUMMARY:{}", ics_escape(&item.title)),
    ];
    if let Some(p) = &item.place {
        lines.push(format!("LOCATION:{}", ics_escape(p)));
    }
    if !description.trim().is_empty() {
        lines.push(format!("DESCRIPTION:{}", ics_escape(description.trim())));
    }
    lines.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
    Some(lines.join("\r\n") + "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_items_leniently() {
        let answer = r#"Hier: {"items": [
            {"kind": "event", "title": "Abstimmung Website", "date": "2026-10-20", "time": "14:00", "end": "15:00", "place": "Raum 2"},
            {"kind": "task", "title": "Angebot prüfen", "date": "2026-10-19", "time": null},
            {"kind": "task", "title": "", "date": "2026-10-19"},
            {"kind": "task", "title": "Irgendwann", "date": "bald"}
        ]}"#;
        let items = parse(answer, 7200);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind, Kind::Event);
        assert_eq!(items[0].at, Some(days_from_civil(2026, 10, 20) * 86400 + 14 * 3600 - 7200));
        assert_eq!(
            items[1].at,
            Some(days_from_civil(2026, 10, 19) * 86400 + 9 * 3600 - 7200),
            "9 o'clock without a time"
        );
        assert_eq!((items[2].date.as_deref(), items[2].at), (None, None));
        assert!(parse("nichts", 0).is_empty());
    }

    #[test]
    fn calendar_entries() {
        let items = parse(
            r#"{"items":[{"kind":"event","title":"Abstimmung, Website","date":"2026-10-20","time":"14:00","place":"Raum 2"},{"kind":"event","title":"Messe","date":"2026-10-31"}]}"#,
            0,
        );
        let a = ics(&items[0], "Aus der Mail von Lea", 1_792_195_200, "m1").unwrap();
        assert!(a.contains("DTSTART:20261020T140000\r\nDTEND:20261020T150000"), "{a}");
        assert!(a.contains("SUMMARY:Abstimmung\\, Website"));
        assert!(a.contains("LOCATION:Raum 2"));
        assert!(a.contains("DTSTAMP:20261017T000000Z"));
        let b = ics(&items[1], "", 0, "m1").unwrap();
        assert!(b.contains("DTSTART;VALUE=DATE:20261031\r\nDTEND;VALUE=DATE:20261101"), "{b}");
        assert_eq!(ics(&items[1], "", 0, "m1"), Some(b), "the same item, the same UID");
    }
}
