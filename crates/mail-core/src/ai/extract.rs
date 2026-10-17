//! Tasks and dates from a message: the model's JSON read leniently, and what the user makes of
//! an item: a reminder at its time (`later::remind_task`) or a draft appointment in the app's
//! calendar (`itip::draft_from`; nothing is written before the user creates it).

use serde::{Deserialize, Serialize};

use super::days_from_civil;

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
        let a = crate::itip::draft_from(&items[0], "Aus der Mail von Lea").unwrap();
        assert_eq!((a.title.as_str(), a.location.as_str(), a.all_day), ("Abstimmung, Website", "Raum 2", false));
        assert_eq!((a.end - a.start).num_minutes(), 60, "an hour without an end");
        let b = crate::itip::draft_from(&items[1], "").unwrap();
        assert!(b.all_day && (b.end - b.start).num_days() == 1);
    }
}
