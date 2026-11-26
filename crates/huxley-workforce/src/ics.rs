//! Busy time from an iCalendar feed (RFC 5545), for calendar sync
//! (ADR-061): each event that takes up time, within a window, recurring
//! ones expanded. Only what free/busy needs is read: when, whether it
//! blocks time (cancelled and transparent events don't), and a subject for
//! public events.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use chrono::TimeZone as _;
use jiff::civil::DateTime as CivilDateTime;
use jiff::tz::TimeZone;
use rrule::{RRuleSet, Tz};

use crate::calendar::{Block, BlockStatus};

/// At most this many occurrences of one recurring event in a window.
const MAX_OCCURRENCES: u16 = 1000;

/// One property line: its name, parameters and value.
#[derive(Debug, Clone)]
struct Prop {
    name: String,
    params: BTreeMap<String, String>,
    value: String,
}

/// Unfolds lines (a line starting with a space or tab continues the one
/// before) and splits them into properties.
fn props(text: &str) -> Vec<Prop> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
            }
        } else if !line.is_empty() {
            lines.push(line.to_owned());
        }
    }
    lines.iter().filter_map(|l| parse_line(l)).collect()
}

fn parse_line(line: &str) -> Option<Prop> {
    // The value starts at the first ':' outside a quoted parameter.
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(i, c)| match c {
        '"' => {
            quoted = !quoted;
            None
        }
        ':' if !quoted => Some(i),
        _ => None,
    })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_owned()))
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_owned(),
    })
}

/// A few Windows time zone names Outlook writes, as IANA names.
fn iana(tzid: &str) -> &str {
    match tzid {
        "UTC" | "Coordinated Universal Time" => "UTC",
        "GMT Standard Time" => "Europe/London",
        "Eastern Standard Time" => "America/New_York",
        "Central Standard Time" => "America/Chicago",
        "Mountain Standard Time" => "America/Denver",
        "US Mountain Standard Time" => "America/Phoenix",
        "Pacific Standard Time" => "America/Los_Angeles",
        "Alaskan Standard Time" => "America/Anchorage",
        "Hawaiian Standard Time" => "Pacific/Honolulu",
        "Atlantic Standard Time" => "America/Halifax",
        "Canada Central Standard Time" => "America/Regina",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Romance Standard Time" => "Europe/Paris",
        "Central Europe Standard Time" => "Europe/Budapest",
        "Central European Standard Time" => "Europe/Warsaw",
        "E. Europe Standard Time" => "Europe/Chisinau",
        "FLE Standard Time" => "Europe/Kiev",
        "GTB Standard Time" => "Europe/Bucharest",
        "Russian Standard Time" => "Europe/Moscow",
        "South Africa Standard Time" => "Africa/Johannesburg",
        "India Standard Time" => "Asia/Kolkata",
        "China Standard Time" => "Asia/Shanghai",
        "Singapore Standard Time" => "Asia/Singapore",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "AUS Eastern Standard Time" => "Australia/Sydney",
        "E. Australia Standard Time" => "Australia/Brisbane",
        "Cen. Australia Standard Time" => "Australia/Adelaide",
        "W. Australia Standard Time" => "Australia/Perth",
        "New Zealand Standard Time" => "Pacific/Auckland",
        other => other,
    }
}

/// A moment from a DATE-TIME or DATE value: unix seconds, and whether it
/// was a whole day.
#[derive(Debug, Clone)]
struct When {
    at: i64,
    all_day: bool,
    /// The time zone it was given in (for expanding recurrences).
    tz: String,
    /// The value as written, without `Z` (for RRULE's DTSTART).
    local: String,
}

fn when(prop: &Prop, default_tz: &str) -> Option<When> {
    let value = prop.value.trim();
    let date_only = prop
        .params
        .get("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"))
        || (value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit()));
    let (text, utc) = match value.strip_suffix('Z') {
        Some(t) => (t, true),
        None => (value, false),
    };
    let tz_name = if utc {
        "UTC".to_owned()
    } else {
        prop.params
            .get("TZID")
            .map_or_else(|| default_tz.to_owned(), |t| iana(t).to_owned())
    };
    let tz = TimeZone::get(&tz_name)
        .or_else(|_| TimeZone::get(default_tz))
        .unwrap_or(TimeZone::UTC);
    let civil = if date_only {
        CivilDateTime::strptime("%Y%m%d", text.get(..8)?).ok()?
    } else {
        CivilDateTime::strptime("%Y%m%dT%H%M%S", text.get(..15)?).ok()?
    };
    let at = civil.to_zoned(tz.clone()).ok()?.timestamp().as_second();
    let used = tz.iana_name().unwrap_or(default_tz).to_owned();
    Some(When {
        at,
        all_day: date_only,
        tz: used,
        local: if date_only {
            format!("{}T000000", &text[..8])
        } else {
            text[..15].to_owned()
        },
    })
}

/// `DURATION` (`P1D`, `PT1H30M`, `-PT15M`…) in seconds.
fn duration(value: &str) -> Option<i64> {
    let (sign, rest) = match value.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let rest = rest.strip_prefix('P')?;
    let (mut total, mut number, mut in_time) = (0i64, String::new(), false);
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            d if d.is_ascii_digit() => number.push(d),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += n * match (unit, in_time) {
                    ('W', _) => 604_800,
                    ('D', _) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(sign * total)
}

/// One VEVENT's properties.
type Event = Vec<Prop>;

fn events(text: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let mut depth: Vec<String> = Vec::new();
    let mut current: Option<Event> = None;
    for prop in props(text) {
        match prop.name.as_str() {
            "BEGIN" => {
                let what = prop.value.to_ascii_uppercase();
                if what == "VEVENT" && depth.last().is_some_and(|d| d == "VCALENDAR") {
                    current = Some(Vec::new());
                }
                depth.push(what);
            }
            "END" => {
                if depth.pop().as_deref() == Some("VEVENT")
                    && let Some(event) = current.take()
                {
                    out.push(event);
                }
            }
            // Properties of the event itself (not of its alarms).
            _ if depth.last().is_some_and(|d| d == "VEVENT") => {
                if let Some(event) = current.as_mut() {
                    event.push(prop);
                }
            }
            _ => {}
        }
    }
    out
}

fn get<'a>(event: &'a Event, name: &str) -> Option<&'a Prop> {
    event.iter().find(|p| p.name == name)
}

fn status_of(event: &Event) -> Option<BlockStatus> {
    if get(event, "STATUS").is_some_and(|p| p.value.eq_ignore_ascii_case("CANCELLED")) {
        return None;
    }
    // Outlook says what it means; otherwise transparency decides.
    if let Some(p) = get(event, "X-MICROSOFT-CDO-BUSYSTATUS") {
        return match p.value.to_ascii_uppercase().as_str() {
            "FREE" => None,
            "TENTATIVE" => Some(BlockStatus::Tentative),
            "OOF" => Some(BlockStatus::Oof),
            "WORKINGELSEWHERE" => Some(BlockStatus::Elsewhere),
            _ => Some(BlockStatus::Busy),
        };
    }
    if get(event, "TRANSP").is_some_and(|p| p.value.eq_ignore_ascii_case("TRANSPARENT")) {
        return None;
    }
    if get(event, "STATUS").is_some_and(|p| p.value.eq_ignore_ascii_case("TENTATIVE")) {
        return Some(BlockStatus::Tentative);
    }
    Some(BlockStatus::Busy)
}

fn subject_of(event: &Event) -> Option<String> {
    let private = get(event, "CLASS").is_some_and(|p| !p.value.eq_ignore_ascii_case("PUBLIC"));
    if private {
        return None;
    }
    get(event, "SUMMARY").map(|p| {
        p.value
            .replace("\\n", " ")
            .replace("\\N", " ")
            .replace("\\,", ",")
            .replace("\\;", ";")
            .replace("\\\\", "\\")
            .chars()
            .take(500)
            .collect()
    })
}

/// Starts of a recurring event's occurrences within `[from, to)`.
fn occurrences(event: &Event, start: &When, from: i64, to: i64, length: i64) -> Vec<i64> {
    let Some(rule) = get(event, "RRULE") else {
        return vec![start.at];
    };
    let dtstart = if start.tz == "UTC" {
        format!("DTSTART:{}Z", start.local)
    } else {
        format!("DTSTART;TZID={}:{}", start.tz, start.local)
    };
    let mut text = format!("{dtstart}\nRRULE:{}", rule.value);
    for ex in event.iter().filter(|p| p.name == "EXDATE") {
        for value in ex.value.split(',') {
            let one = Prop {
                value: value.to_owned(),
                ..ex.clone()
            };
            if let Some(w) = when(&one, &start.tz) {
                let utc = chrono::Utc
                    .timestamp_opt(w.at, 0)
                    .single()
                    .map(|d| d.format("%Y%m%dT%H%M%SZ").to_string());
                if let Some(utc) = utc {
                    let _ = write!(text, "\nEXDATE:{utc}");
                }
            }
        }
    }
    let Ok(set) = text.parse::<RRuleSet>() else {
        return vec![start.at];
    };
    let (Some(after), Some(before)) = (
        Tz::UTC.timestamp_opt(from - length, 0).single(),
        Tz::UTC.timestamp_opt(to, 0).single(),
    ) else {
        return Vec::new();
    };
    set.after(after)
        .before(before)
        .all(MAX_OCCURRENCES)
        .dates
        .iter()
        .map(chrono::DateTime::timestamp)
        .collect()
}

/// Busy blocks from a feed, overlapping `[from, to)` (unix seconds).
/// Times without a zone are in `default_tz` (the person's).
pub fn busy(text: &str, from: i64, to: i64, default_tz: &str) -> Vec<Block> {
    let all = events(text);
    // Moved or cancelled single occurrences: (UID, original start).
    let overridden: BTreeSet<(String, i64)> = all
        .iter()
        .filter_map(|e| {
            let uid = get(e, "UID")?.value.clone();
            let original = when(get(e, "RECURRENCE-ID")?, default_tz)?;
            Some((uid, original.at))
        })
        .collect();
    let mut out = Vec::new();
    for event in &all {
        let Some(status) = status_of(event) else {
            continue;
        };
        let Some(start) = get(event, "DTSTART").and_then(|p| when(p, default_tz)) else {
            continue;
        };
        let end = get(event, "DTEND")
            .and_then(|p| when(p, default_tz))
            .map(|w| w.at)
            .or_else(|| {
                get(event, "DURATION")
                    .and_then(|p| duration(&p.value))
                    .map(|d| start.at + d)
            })
            .unwrap_or(start.at + if start.all_day { 86_400 } else { 0 });
        let length = (end - start.at).max(0);
        if length == 0 {
            continue;
        }
        let uid = get(event, "UID")
            .map(|p| p.value.clone())
            .unwrap_or_default();
        let is_override = get(event, "RECURRENCE-ID").is_some();
        let subject = subject_of(event);
        for at in occurrences(event, &start, from, to, length) {
            if !is_override && overridden.contains(&(uid.clone(), at)) {
                continue;
            }
            let (s, e) = (at, at + length);
            if e > from && s < to {
                out.push(Block {
                    starts_at: s,
                    ends_at: e,
                    status,
                    subject: subject.clone(),
                });
            }
        }
    }
    out.sort_by_key(|b| (b.starts_at, b.ends_at));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str, tz: &str) -> i64 {
        CivilDateTime::strptime("%Y-%m-%d %H:%M", s)
            .unwrap()
            .to_zoned(TimeZone::get(tz).unwrap())
            .unwrap()
            .timestamp()
            .as_second()
    }

    const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:one\r\nDTSTART:20261201T150000Z\r\nDTEND:20261201T160000Z\r\nSUMMARY:Board\r\n meeting\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:free\r\nDTSTART:20261201T170000Z\r\nDTEND:20261201T180000Z\r\nTRANSP:TRANSPARENT\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:gone\r\nDTSTART:20261201T190000Z\r\nDTEND:20261201T200000Z\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:secret\r\nDTSTART;TZID=Europe/London:20261202T090000\r\nDURATION:PT30M\r\nCLASS:PRIVATE\r\nSUMMARY:Dentist\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly\r\nDTSTART;TZID=America/New_York:20261130T100000\r\nDTEND;TZID=America/New_York:20261130T103000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=4\r\nEXDATE;TZID=America/New_York:20261207T100000\r\nSUMMARY:Stand-up\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT10M\r\nSUMMARY:Ignore me\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly\r\nRECURRENCE-ID;TZID=America/New_York:20261214T100000\r\n\
DTSTART;TZID=America/New_York:20261214T140000\r\nDTEND;TZID=America/New_York:20261214T143000\r\nSUMMARY:Stand-up (moved)\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:holiday\r\nDTSTART;VALUE=DATE:20261224\r\nDTEND;VALUE=DATE:20261225\r\nX-MICROSOFT-CDO-BUSYSTATUS:OOF\r\nSUMMARY:Away\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn feeds_become_busy_blocks() {
        let from = at("2026-11-29 00:00", "UTC");
        let to = at("2027-01-01 00:00", "UTC");
        let blocks = busy(FEED, from, to, "UTC");
        let summary: Vec<(i64, i64, BlockStatus, Option<&str>)> = blocks
            .iter()
            .map(|b| (b.starts_at, b.ends_at, b.status, b.subject.as_deref()))
            .collect();
        let ny = |s: &str| at(s, "America/New_York");
        assert_eq!(
            summary,
            vec![
                (
                    ny("2026-11-30 10:00"),
                    ny("2026-11-30 10:30"),
                    BlockStatus::Busy,
                    Some("Stand-up")
                ),
                (
                    at("2026-12-01 15:00", "UTC"),
                    at("2026-12-01 16:00", "UTC"),
                    BlockStatus::Busy,
                    Some("Boardmeeting")
                ),
                // Private: no subject. The free and cancelled ones are gone.
                (
                    at("2026-12-02 09:00", "Europe/London"),
                    at("2026-12-02 09:30", "Europe/London"),
                    BlockStatus::Busy,
                    None
                ),
                // The 7th is excluded; the 14th moved to the afternoon.
                (
                    ny("2026-12-14 14:00"),
                    ny("2026-12-14 14:30"),
                    BlockStatus::Busy,
                    Some("Stand-up (moved)")
                ),
                (
                    ny("2026-12-21 10:00"),
                    ny("2026-12-21 10:30"),
                    BlockStatus::Busy,
                    Some("Stand-up")
                ),
                (
                    at("2026-12-24 00:00", "UTC"),
                    at("2026-12-25 00:00", "UTC"),
                    BlockStatus::Oof,
                    Some("Away")
                ),
            ]
        );
    }

    #[test]
    fn windows_and_durations() {
        assert_eq!(duration("PT1H30M"), Some(5400));
        assert_eq!(duration("P1DT2H"), Some(93_600));
        assert_eq!(duration("-PT15M"), Some(-900));
        assert_eq!(iana("Pacific Standard Time"), "America/Los_Angeles");
        // Only what overlaps the window.
        let from = at("2026-12-01 15:30", "UTC");
        let to = at("2026-12-01 15:45", "UTC");
        assert_eq!(busy(FEED, from, to, "UTC").len(), 1);
        // Floating times are the person's.
        let floating = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:f\nDTSTART:20261201T090000\nDTEND:20261201T100000\nEND:VEVENT\nEND:VCALENDAR\n";
        let b = busy(
            floating,
            at("2026-12-01 00:00", "UTC"),
            at("2026-12-02 00:00", "UTC"),
            "Europe/Paris",
        );
        assert_eq!(b[0].starts_at, at("2026-12-01 09:00", "Europe/Paris"));
    }
}
