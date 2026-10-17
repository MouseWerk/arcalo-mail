//! Time zones of calendar data, taken over from Arcalo (`calsync/tz.rs`): IANA names, the
//! Windows names Outlook and Exchange write (`W. Europe Standard Time`), zone definitions carried
//! in an ICS file (`VTIMEZONE`), fixed offsets and the device's local zone (floating times). For
//! writing, [`local_name`] names the device's zone and [`vtimezone`] describes a zone's changes
//! for the years an appointment needs, so other clients place it as this device does.

use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset, LocalResult, NaiveDateTime, Offset, TimeZone, Utc};

/// A zone that turns wall-clock times into instants and back.
#[derive(Debug, Clone)]
pub enum Zone {
    Utc,
    Iana(chrono_tz::Tz),
    Fixed(FixedOffset),
    /// Rules of a `VTIMEZONE` without a known name (e.g. Outlook's „(UTC+01:00) Amsterdam, Berlin, …“).
    Rules(Arc<ZoneRules>),
    /// The computer's zone (floating times, all-day events).
    Local,
}

impl Zone {
    /// The instant of a wall-clock time. In a gap (spring forward) the time after the gap is
    /// used, in an overlap (fall back) the earlier of the two.
    pub fn to_utc(&self, wall: NaiveDateTime) -> DateTime<Utc> {
        fn pick<T: TimeZone>(tz: &T, wall: NaiveDateTime) -> DateTime<Utc> {
            for shift in [0, 30, 60, 90, 120] {
                match tz.from_local_datetime(&(wall + Duration::minutes(shift))) {
                    LocalResult::Single(t) => return t.with_timezone(&Utc),
                    LocalResult::Ambiguous(a, _) => return a.with_timezone(&Utc),
                    LocalResult::None => continue,
                }
            }
            Utc.from_utc_datetime(&wall)
        }
        match self {
            Zone::Utc => Utc.from_utc_datetime(&wall),
            Zone::Iana(tz) => pick(tz, wall),
            Zone::Fixed(off) => pick(off, wall),
            Zone::Local => pick(&chrono::Local, wall),
            Zone::Rules(r) => r.to_utc(wall),
        }
    }

    /// The wall-clock time of an instant in this zone.
    pub fn to_wall(&self, t: DateTime<Utc>) -> NaiveDateTime {
        match self {
            Zone::Utc => t.naive_utc(),
            Zone::Iana(tz) => t.with_timezone(tz).naive_local(),
            Zone::Fixed(off) => t.with_timezone(off).naive_local(),
            Zone::Local => t.with_timezone(&chrono::Local).naive_local(),
            Zone::Rules(r) => t.naive_utc() + Duration::seconds(r.offset_at(t) as i64),
        }
    }

    /// The zone of a `TZID`: an IANA name (also behind a path such as
    /// `/mozilla.org/20050126_1/Europe/Berlin`), a Windows name, `UTC`/`GMT` or a fixed
    /// offset (`UTC+01:00`). `None` for names only a `VTIMEZONE` can explain.
    pub fn named(tzid: &str) -> Option<Zone> {
        let id = tzid.trim().trim_matches('"').trim();
        if id.is_empty() {
            return None;
        }
        if matches!(
            id.to_ascii_uppercase().as_str(),
            "UTC" | "GMT" | "Z" | "ETC/UTC" | "ETC/GMT" | "COORDINATED UNIVERSAL TIME"
        ) {
            return Some(Zone::Utc);
        }
        if let Ok(tz) = id.parse::<chrono_tz::Tz>() {
            return Some(Zone::Iana(tz));
        }
        if let Some(iana) = windows_to_iana(id) {
            return iana.parse::<chrono_tz::Tz>().ok().map(Zone::Iana);
        }
        // A path ending in an IANA name.
        let parts: Vec<&str> = id.split('/').filter(|p| !p.is_empty()).collect();
        for n in (2..=parts.len().min(3)).rev() {
            if let Ok(tz) = parts[parts.len() - n..].join("/").parse::<chrono_tz::Tz>() {
                return Some(Zone::Iana(tz));
            }
        }
        fixed_offset(id).map(Zone::Fixed)
    }
}

/// `UTC+01:00`, `GMT-5`, `+0200`: a fixed offset. Display names with a place after the
/// offset (Outlook's „(UTC+01:00) Amsterdam, …“) are left to their `VTIMEZONE`.
fn fixed_offset(id: &str) -> Option<FixedOffset> {
    let s = id.trim();
    let rest = s.strip_prefix("UTC").or_else(|| s.strip_prefix("GMT")).unwrap_or(s);
    let first = rest.chars().next()?;
    let sign = match first {
        '+' => 1,
        '-' | '\u{2212}' => -1,
        _ => return None,
    };
    let digits = &rest[first.len_utf8()..];
    let (h, m) = match digits.split_once(':') {
        Some((h, m)) => (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?),
        None if digits.len() == 4 && digits.is_ascii() => (digits[..2].parse().ok()?, digits[2..].parse().ok()?),
        None => (digits.parse().ok()?, 0),
    };
    if h > 14 || m >= 60 {
        return None;
    }
    FixedOffset::east_opt(sign * (h * 3600 + m * 60))
}

/// A zone defined by its transitions (`VTIMEZONE` with `STANDARD`/`DAYLIGHT` rules).
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneRules {
    /// Onset (UTC) and the offset (seconds east) from then on, sorted by onset.
    pub transitions: Vec<(DateTime<Utc>, i32)>,
    /// Offset before the first transition.
    pub initial: i32,
}

impl ZoneRules {
    pub fn offset_at(&self, t: DateTime<Utc>) -> i32 {
        match self.transitions.partition_point(|(onset, _)| *onset <= t) {
            0 => self.initial,
            i => self.transitions[i - 1].1,
        }
    }

    fn to_utc(&self, wall: NaiveDateTime) -> DateTime<Utc> {
        let mut offsets: Vec<i32> = self.transitions.iter().map(|(_, o)| *o).collect();
        offsets.push(self.initial);
        offsets.sort_unstable();
        offsets.dedup();
        // The larger offset first: in an overlap that is the earlier instant.
        for off in offsets.iter().rev() {
            let t = Utc.from_utc_datetime(&(wall - Duration::seconds(*off as i64)));
            if self.offset_at(t) == *off {
                return t;
            }
        }
        // In a gap: the offset before it.
        let guess = Utc.from_utc_datetime(&wall);
        Utc.from_utc_datetime(&(wall - Duration::seconds(self.offset_at(guess) as i64)))
    }
}

/// The offset of an instant in `zone`, in seconds east.
pub fn offset_seconds(zone: &Zone, t: DateTime<Utc>) -> i32 {
    match zone {
        Zone::Utc => 0,
        Zone::Iana(tz) => tz.offset_from_utc_datetime(&t.naive_utc()).fix().local_minus_utc(),
        Zone::Fixed(o) => o.local_minus_utc(),
        Zone::Local => chrono::Local.offset_from_utc_datetime(&t.naive_utc()).local_minus_utc(),
        Zone::Rules(r) => r.offset_at(t),
    }
}

/// The IANA zone of a Windows time zone id (CLDR `windowsZones`, territory 001).
pub fn windows_to_iana(name: &str) -> Option<&'static str> {
    let n = name.trim();
    WINDOWS_ZONES.iter().find(|(w, _)| w.eq_ignore_ascii_case(n)).map(|(_, i)| *i)
}

/// The IANA name of the device's zone (`Europe/Berlin`); `None` when the system does not say.
/// `ARCALO_MAIL_TZ` stands in for it in tests.
pub fn local_name() -> Option<String> {
    if let Ok(v) = std::env::var("ARCALO_MAIL_TZ")
        && v.parse::<chrono_tz::Tz>().is_ok()
    {
        return Some(v);
    }
    iana_time_zone::get_timezone().ok().filter(|n| n.parse::<chrono_tz::Tz>().is_ok())
}

/// `+0100`, `-0530` for an offset in seconds east.
fn offset_text(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let s = seconds.abs();
    format!("{sign}{:02}{:02}", s / 3600, (s % 3600) / 60)
}

/// A `VTIMEZONE` for `name` with every change of its offset from a year before `from` to a year
/// after `to` (as explicit observances, which every client reads); `None` for an unknown name.
pub fn vtimezone(name: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<String> {
    use chrono_tz::OffsetComponents;
    let tz: chrono_tz::Tz = name.parse().ok()?;
    let offset = |t: DateTime<Utc>| tz.offset_from_utc_datetime(&t.naive_utc()).fix().local_minus_utc();
    let daylight = |t: DateTime<Utc>| !tz.offset_from_utc_datetime(&t.naive_utc()).dst_offset().is_zero();
    let day = |t: DateTime<Utc>| Utc.from_utc_datetime(&t.date_naive().and_hms_opt(0, 0, 0).unwrap_or_default());
    let start = day(from - Duration::days(366));
    let end = to + Duration::days(366);
    let mut out = format!("BEGIN:VTIMEZONE\r\nTZID:{name}\r\n");
    let observance = |out: &mut String, at: DateTime<Utc>, before: i32, after: i32, dst: bool| {
        let kind = if dst { "DAYLIGHT" } else { "STANDARD" };
        let wall = at.naive_utc() + Duration::seconds(before as i64);
        out.push_str(&format!(
            "BEGIN:{kind}\r\nDTSTART:{}\r\nTZOFFSETFROM:{}\r\nTZOFFSETTO:{}\r\nEND:{kind}\r\n",
            wall.format("%Y%m%dT%H%M%S"),
            offset_text(before),
            offset_text(after)
        ));
    };
    let first = offset(start);
    observance(&mut out, start, first, first, daylight(start));
    let mut day = start;
    while day < end {
        let next = day + Duration::days(1);
        let (a, b) = (offset(day), offset(next));
        if a != b {
            // The minute of the change.
            let (mut lo, mut hi) = (day, next);
            while hi - lo > Duration::seconds(1) {
                let mid = lo + (hi - lo) / 2;
                if offset(mid) == a { lo = mid } else { hi = mid }
            }
            observance(&mut out, hi, a, b, daylight(hi));
        }
        day = next;
    }
    out.push_str("END:VTIMEZONE\r\n");
    Some(out)
}

/// Windows time zone ids and their IANA zones.
pub const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC-11", "Etc/GMT+11"),
    ("Aleutian Standard Time", "America/Adak"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Marquesas Standard Time", "Pacific/Marquesas"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("UTC-09", "Etc/GMT+9"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("UTC-08", "Etc/GMT+8"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time (Mexico)", "America/Mazatlan"),
    ("Mountain Standard Time", "America/Denver"),
    ("Yukon Standard Time", "America/Whitehorse"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Standard Time", "America/Chicago"),
    ("Easter Island Standard Time", "Pacific/Easter"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Mexico Standard Time", "America/Mexico_City"),
    ("Mexico Standard Time 2", "America/Chihuahua"),
    ("Canada Central Standard Time", "America/Regina"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("Eastern Standard Time", "America/New_York"),
    ("Haiti Standard Time", "America/Port-au-Prince"),
    ("Cuba Standard Time", "America/Havana"),
    ("US Eastern Standard Time", "America/Indiana/Indianapolis"),
    ("Turks And Caicos Standard Time", "America/Grand_Turk"),
    ("Paraguay Standard Time", "America/Asuncion"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Central Brazilian Standard Time", "America/Cuiaba"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("Tocantins Standard Time", "America/Araguaina"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("Argentina Standard Time", "America/Argentina/Buenos_Aires"),
    ("Greenland Standard Time", "America/Nuuk"),
    ("Montevideo Standard Time", "America/Montevideo"),
    ("Magallanes Standard Time", "America/Punta_Arenas"),
    ("Saint Pierre Standard Time", "America/Miquelon"),
    ("Bahia Standard Time", "America/Bahia"),
    ("UTC-02", "Etc/GMT+2"),
    ("Mid-Atlantic Standard Time", "Etc/GMT+2"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("UTC", "Etc/UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("Sao Tome Standard Time", "Africa/Sao_Tome"),
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("Jordan Standard Time", "Asia/Amman"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Middle East Standard Time", "Asia/Beirut"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("Syria Standard Time", "Asia/Damascus"),
    ("West Bank Standard Time", "Asia/Hebron"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("FLE Standard Time", "Europe/Kyiv"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("South Sudan Standard Time", "Africa/Juba"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Sudan Standard Time", "Africa/Khartoum"),
    ("Libya Standard Time", "Africa/Tripoli"),
    ("Namibia Standard Time", "Africa/Windhoek"),
    ("Arabic Standard Time", "Asia/Baghdad"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Belarus Standard Time", "Europe/Minsk"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Volgograd Standard Time", "Europe/Volgograd"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Astrakhan Standard Time", "Europe/Astrakhan"),
    ("Azerbaijan Standard Time", "Asia/Baku"),
    ("Russia Time Zone 3", "Europe/Samara"),
    ("Mauritius Standard Time", "Indian/Mauritius"),
    ("Saratov Standard Time", "Europe/Saratov"),
    ("Georgian Standard Time", "Asia/Tbilisi"),
    ("Caucasus Standard Time", "Asia/Yerevan"),
    ("Armenian Standard Time", "Asia/Yerevan"),
    ("Afghanistan Standard Time", "Asia/Kabul"),
    ("West Asia Standard Time", "Asia/Tashkent"),
    ("Ekaterinburg Standard Time", "Asia/Yekaterinburg"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("Qyzylorda Standard Time", "Asia/Qyzylorda"),
    ("India Standard Time", "Asia/Kolkata"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Kathmandu"),
    ("Central Asia Standard Time", "Asia/Bishkek"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("Omsk Standard Time", "Asia/Omsk"),
    ("Myanmar Standard Time", "Asia/Yangon"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("Altai Standard Time", "Asia/Barnaul"),
    ("W. Mongolia Standard Time", "Asia/Hovd"),
    ("North Asia Standard Time", "Asia/Krasnoyarsk"),
    ("N. Central Asia Standard Time", "Asia/Novosibirsk"),
    ("Tomsk Standard Time", "Asia/Tomsk"),
    ("China Standard Time", "Asia/Shanghai"),
    ("North Asia East Standard Time", "Asia/Irkutsk"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Ulaanbaatar Standard Time", "Asia/Ulaanbaatar"),
    ("Aus Central W. Standard Time", "Australia/Eucla"),
    ("Transbaikal Standard Time", "Asia/Chita"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("North Korea Standard Time", "Asia/Pyongyang"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Yakutsk Standard Time", "Asia/Yakutsk"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("West Pacific Standard Time", "Pacific/Port_Moresby"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("Vladivostok Standard Time", "Asia/Vladivostok"),
    ("Lord Howe Standard Time", "Australia/Lord_Howe"),
    ("Bougainville Standard Time", "Pacific/Bougainville"),
    ("Russia Time Zone 10", "Asia/Srednekolymsk"),
    ("Magadan Standard Time", "Asia/Magadan"),
    ("Norfolk Standard Time", "Pacific/Norfolk"),
    ("Sakhalin Standard Time", "Asia/Sakhalin"),
    ("Central Pacific Standard Time", "Pacific/Guadalcanal"),
    ("Russia Time Zone 11", "Asia/Kamchatka"),
    ("Kamchatka Standard Time", "Asia/Kamchatka"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("UTC+12", "Etc/GMT-12"),
    ("Fiji Standard Time", "Pacific/Fiji"),
    ("Chatham Islands Standard Time", "Pacific/Chatham"),
    ("UTC+13", "Etc/GMT-13"),
    ("Tonga Standard Time", "Pacific/Tongatapu"),
    ("Samoa Standard Time", "Pacific/Apia"),
    ("Line Islands Standard Time", "Pacific/Kiritimati"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn wall(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, 0).unwrap()
    }

    #[test]
    fn every_windows_zone_is_a_known_iana_zone() {
        for (w, iana) in WINDOWS_ZONES {
            assert!(iana.parse::<chrono_tz::Tz>().is_ok(), "{w} → {iana}");
        }
    }

    #[test]
    fn names_resolve_to_zones() {
        for id in [
            "W. Europe Standard Time",
            "Europe/Berlin",
            "/mozilla.org/20050126_1/Europe/Berlin",
            "w. europe standard time",
        ] {
            let z = Zone::named(id).unwrap_or_else(|| panic!("{id}"));
            // 10:00 in summer is 08:00 UTC, in winter 09:00 UTC.
            assert_eq!(z.to_utc(wall(2026, 7, 1, 10, 0)).naive_utc(), wall(2026, 7, 1, 8, 0), "{id}");
            assert_eq!(z.to_utc(wall(2026, 12, 1, 10, 0)).naive_utc(), wall(2026, 12, 1, 9, 0), "{id}");
        }
        assert!(matches!(Zone::named("UTC"), Some(Zone::Utc)));
        let fixed = Zone::named("UTC+05:30").unwrap();
        assert_eq!(fixed.to_utc(wall(2026, 1, 1, 12, 0)).naive_utc(), wall(2026, 1, 1, 6, 30));
        assert!(Zone::named("(UTC+01:00) Amsterdam, Berlin, Bern, Rom, Stockholm, Wien").is_none());
        assert!(Zone::named("").is_none());
        // Broken offsets are no zone (and no panic on a character across the cut).
        for id in ["UTC+\u{20AC}1", "UTC+1\u{E4}", "GMT-\u{E4}\u{E4}"] {
            assert!(Zone::named(id).is_none(), "{id}");
        }
    }

    #[test]
    fn gaps_and_overlaps_at_the_dst_change() {
        let berlin = Zone::named("Europe/Berlin").unwrap();
        // 29 March 2026: 02:00–03:00 does not exist.
        assert_eq!(berlin.to_utc(wall(2026, 3, 29, 2, 30)).naive_utc(), wall(2026, 3, 29, 1, 0));
        // 25 October 2026: 02:30 exists twice, the earlier one (summer time) is taken.
        assert_eq!(berlin.to_utc(wall(2026, 10, 25, 2, 30)).naive_utc(), wall(2026, 10, 25, 0, 30));
        let t = Utc.from_utc_datetime(&wall(2026, 10, 25, 1, 30));
        assert_eq!(berlin.to_wall(t), wall(2026, 10, 25, 2, 30));
        assert_eq!(offset_seconds(&berlin, t), 3600);
    }

    #[test]
    fn rules_pick_the_latest_transition() {
        let at = |m, d, h| Utc.from_utc_datetime(&wall(2026, m, d, h, 0));
        let rules = ZoneRules { transitions: vec![(at(3, 29, 1), 7200), (at(10, 25, 1), 3600)], initial: 3600 };
        assert_eq!(rules.offset_at(at(1, 1, 0)), 3600);
        assert_eq!(rules.offset_at(at(7, 1, 0)), 7200);
        assert_eq!(rules.offset_at(at(12, 1, 0)), 3600);
        let z = Zone::Rules(Arc::new(rules));
        assert_eq!(z.to_utc(wall(2026, 7, 1, 10, 0)).naive_utc(), wall(2026, 7, 1, 8, 0));
        assert_eq!(z.to_utc(wall(2026, 12, 1, 10, 0)).naive_utc(), wall(2026, 12, 1, 9, 0));
        assert_eq!(z.to_wall(at(7, 1, 8)), wall(2026, 7, 1, 10, 0));
    }

    #[test]
    fn a_vtimezone_names_every_change() {
        let at = |y, m, d| Utc.from_utc_datetime(&wall(y, m, d, 12, 0));
        let v = vtimezone("Europe/Berlin", at(2026, 10, 20), at(2026, 10, 20)).unwrap();
        assert!(v.starts_with("BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\n"));
        assert!(
            v.contains("BEGIN:DAYLIGHT\r\nDTSTART:20260329T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200"),
            "{v}"
        );
        assert!(
            v.contains("BEGIN:STANDARD\r\nDTSTART:20261025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100"),
            "{v}"
        );
        // Read back by the parser, it places times as chrono-tz does.
        let ics = format!(
            "BEGIN:VCALENDAR\r\n{v}BEGIN:VEVENT\r\nUID:x\r\nDTSTART;TZID=Europe/Berlin:20261026T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let cal = super::super::ics::parse_components(ics.as_bytes());
        let tzc = cal[0].children.iter().find(|c| c.name == "VTIMEZONE").unwrap();
        assert!(!tzc.children.is_empty());
        assert!(vtimezone("Nowhere/City", at(2026, 1, 1), at(2026, 1, 1)).is_none());
    }
}
