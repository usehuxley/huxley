//! Business time (ADR-037): weekly opening hours in a time zone, or around
//! the clock, less closed days (holidays). Times are Unix seconds. Used by
//! SLAs and people's schedules.

use std::collections::BTreeSet;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde_json::{Value as Json, json};

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
/// How far ahead to look for open time before giving up (about 10 years).
const MAX_DAYS: usize = 3700;

#[derive(Debug, Clone)]
pub enum Calendar {
    /// Every second counts.
    Always,
    /// Opening hours per weekday (Monday first), as minutes from midnight.
    Weekly {
        tz_name: String,
        tz: TimeZone,
        days: Box<[Vec<(u32, u32)>; 7]>,
        /// Days closed all day (holidays), in the calendar's zone.
        closed: Box<BTreeSet<Date>>,
    },
}

fn minutes(value: &Json) -> Option<u32> {
    let (h, m) = value.as_str()?.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (m < 60 && (h < 24 || (h == 24 && m == 0))).then_some(h * 60 + m)
}

fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

impl Calendar {
    /// Weekly hours (`{"mon": [["09:00", "17:00"]], …}`) in a time zone.
    /// `None` if the zone is unknown or no day has any open time.
    pub fn weekly(tz_name: &str, hours: &Json) -> Option<Self> {
        let tz = TimeZone::get(tz_name).ok()?;
        let mut days: [Vec<(u32, u32)>; 7] = Default::default();
        for (i, day) in DAYS.iter().enumerate() {
            for range in hours
                .get(day)
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                let (Some(open), Some(close)) = (
                    range.get(0).and_then(minutes),
                    range.get(1).and_then(minutes),
                ) else {
                    continue;
                };
                if open < close {
                    days[i].push((open, close));
                }
            }
            days[i].sort_unstable();
        }
        days.iter().any(|d| !d.is_empty()).then(|| Self::Weekly {
            tz_name: tz_name.to_owned(),
            tz,
            days: Box::new(days),
            closed: Box::default(),
        })
    }

    /// These days closed too (`YYYY-MM-DD`; others are ignored). No effect
    /// around the clock.
    #[must_use]
    pub fn closed_on<'a>(mut self, dates: impl IntoIterator<Item = &'a str>) -> Self {
        if let Self::Weekly { closed, .. } = &mut self {
            closed.extend(dates.into_iter().filter_map(|d| d.parse::<Date>().ok()));
        }
        self
    }

    pub fn to_json(&self) -> Json {
        match self {
            Self::Always => json!({ "kind": "always" }),
            Self::Weekly {
                tz_name,
                days,
                closed,
                ..
            } => {
                let hours: serde_json::Map<String, Json> = DAYS
                    .iter()
                    .zip(days.iter())
                    .filter(|(_, ranges)| !ranges.is_empty())
                    .map(|(day, ranges)| {
                        let list = ranges
                            .iter()
                            .map(|(o, c)| json!([hhmm(*o), hhmm(*c)]))
                            .collect();
                        ((*day).to_owned(), Json::Array(list))
                    })
                    .collect();
                let closed: Vec<String> = closed.iter().map(ToString::to_string).collect();
                if closed.is_empty() {
                    json!({ "kind": "weekly", "tz": tz_name, "hours": hours })
                } else {
                    json!({ "kind": "weekly", "tz": tz_name, "hours": hours, "closed": closed })
                }
            }
        }
    }

    /// From [`Calendar::to_json`]; anything unreadable counts as always.
    pub fn from_json(value: &Json) -> Self {
        match (
            value.get("kind").and_then(Json::as_str),
            value.get("tz").and_then(Json::as_str),
        ) {
            (Some("weekly"), Some(tz)) => Self::weekly(
                tz,
                value.get("hours").unwrap_or(&Json::Null),
            )
            .map_or(Self::Always, |c| {
                c.closed_on(
                    value
                        .get("closed")
                        .and_then(Json::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Json::as_str),
                )
            }),
            _ => Self::Always,
        }
    }

    /// Open windows (as Unix-second ranges) on the days from `from`'s day,
    /// in order, until `f` returns `false`.
    fn windows(&self, from: i64, mut f: impl FnMut(i64, i64) -> bool) {
        let Self::Weekly {
            tz, days, closed, ..
        } = self
        else {
            f(from, i64::MAX);
            return;
        };
        let Ok(start) = jiff::Timestamp::from_second(from) else {
            return;
        };
        // From the day before, in case the zone's offset moves windows across midnight.
        let mut date: Date = start
            .to_zoned(tz.clone())
            .date()
            .yesterday()
            .unwrap_or(Date::MIN);
        let at = |date: Date, minute: u32| -> Option<i64> {
            let (date, minute) = if minute == 24 * 60 {
                (date.tomorrow().ok()?, 0)
            } else {
                (date, minute)
            };
            let hour = i8::try_from(minute / 60).ok()?;
            let min = i8::try_from(minute % 60).ok()?;
            Some(
                date.at(hour, min, 0, 0)
                    .to_zoned(tz.clone())
                    .ok()?
                    .timestamp()
                    .as_second(),
            )
        };
        for _ in 0..MAX_DAYS {
            let weekday = usize::try_from(date.weekday().to_monday_zero_offset()).unwrap_or(0);
            let today: &[(u32, u32)] = if closed.contains(&date) {
                &[]
            } else {
                &days[weekday]
            };
            for &(open, close) in today {
                let (Some(s), Some(e)) = (at(date, open), at(date, close)) else {
                    continue;
                };
                if e > from && !f(s.max(from), e) {
                    return;
                }
            }
            let Ok(next) = date.tomorrow() else { return };
            date = next;
        }
    }

    /// Open seconds between `from` and `to`.
    pub fn open_between(&self, from: i64, to: i64) -> i64 {
        if to <= from {
            return 0;
        }
        let mut total = 0;
        self.windows(from, |s, e| {
            if s >= to {
                return false;
            }
            total += e.min(to) - s;
            true
        });
        total
    }

    /// Whether it's open at `at` (the minute starting then).
    pub fn is_open(&self, at: i64) -> bool {
        self.open_between(at, at + 60) > 0
    }

    /// The hours as given to [`Calendar::weekly`] (normalized), if weekly.
    pub fn hours(&self) -> Option<Json> {
        match self {
            Self::Always => None,
            Self::Weekly { .. } => Some(self.to_json()["hours"].clone()),
        }
    }

    /// When `seconds` of open time have passed since `from`.
    pub fn add(&self, from: i64, seconds: i64) -> i64 {
        if seconds <= 0 {
            return from;
        }
        let mut remaining = seconds;
        let mut due = None;
        self.windows(from, |s, e| {
            if remaining <= e - s {
                due = Some(s + remaining);
                return false;
            }
            remaining -= e - s;
            true
        });
        // No open time within reach: fall back to wall-clock time.
        due.unwrap_or(from.saturating_add(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> i64 {
        s.parse::<jiff::Timestamp>().unwrap().as_second()
    }

    fn nine_to_five(tz: &str) -> Calendar {
        let day = json!([["09:00", "17:00"]]);
        Calendar::weekly(
            tz,
            &json!({ "mon": day, "tue": day, "wed": day, "thu": day, "fri": day }),
        )
        .unwrap()
    }

    #[test]
    fn always_is_wall_clock() {
        let c = Calendar::Always;
        assert_eq!(c.add(1000, 60), 1060);
        assert_eq!(c.open_between(1000, 1060), 60);
        assert_eq!(c.open_between(1060, 1000), 0);
    }

    #[test]
    fn business_hours_skip_nights_and_weekends() {
        let c = nine_to_five("UTC");
        // Friday 16:00 + 2 business hours = Monday 10:00.
        let fri = ts("2026-11-27T16:00:00Z");
        assert_eq!(c.add(fri, 2 * 3600), ts("2026-11-30T10:00:00Z"));
        assert_eq!(c.open_between(fri, ts("2026-11-30T10:00:00Z")), 2 * 3600);
        // Saturday counts nothing; starts at Monday 09:00.
        assert_eq!(
            c.add(ts("2026-11-28T12:00:00Z"), 60),
            ts("2026-11-30T09:01:00Z")
        );
        // Ending exactly at closing time stays on that day.
        assert_eq!(
            c.add(ts("2026-11-30T09:00:00Z"), 8 * 3600),
            ts("2026-11-30T17:00:00Z")
        );
        // Overnight: 23 wall hours, 8 business hours.
        assert_eq!(
            c.open_between(ts("2026-11-30T09:00:00Z"), ts("2026-12-01T08:00:00Z")),
            8 * 3600
        );
    }

    #[test]
    fn hours_are_local_across_dst() {
        // London: 09:00 local is 09:00Z in winter and 08:00Z in summer.
        let c = nine_to_five("Europe/London");
        assert_eq!(
            c.add(ts("2026-01-05T00:00:00Z"), 60),
            ts("2026-01-05T09:01:00Z")
        );
        assert_eq!(
            c.add(ts("2026-07-06T00:00:00Z"), 60),
            ts("2026-07-06T08:01:00Z")
        );
        // Friday before the March change to Monday after: one business hour each side.
        assert_eq!(
            c.open_between(ts("2026-03-27T16:00:00Z"), ts("2026-03-30T09:00:00Z")),
            2 * 3600
        );
    }

    #[test]
    fn split_days_and_round_trips() {
        let hours = json!({ "mon": [["09:00", "12:00"], ["13:00", "24:00"]] });
        let c = Calendar::weekly("UTC", &hours).unwrap();
        // 3h in the morning, then the rest after lunch.
        assert_eq!(
            c.add(ts("2026-11-30T09:00:00Z"), 4 * 3600),
            ts("2026-11-30T14:00:00Z")
        );
        // Open until midnight.
        assert_eq!(
            c.add(ts("2026-11-30T23:00:00Z"), 3600),
            ts("2026-12-01T00:00:00Z")
        );
        let back = Calendar::from_json(&c.to_json());
        assert_eq!(back.to_json(), c.to_json());
        assert_eq!(c.to_json()["hours"], hours);
    }

    #[test]
    fn closed_days_are_skipped_and_kept() {
        // Friday 2026-12-25 closed: from Thursday 16:00, 2 hours of work is
        // done Monday 10:00, not Friday 10:00.
        let c = nine_to_five("UTC").closed_on(["2026-12-25", "not a date"]);
        assert_eq!(
            c.add(ts("2026-12-24T16:00:00Z"), 2 * 3600),
            ts("2026-12-28T10:00:00Z")
        );
        assert!(!c.is_open(ts("2026-12-25T12:00:00Z")));
        let again = Calendar::from_json(&c.to_json());
        assert_eq!(again.to_json()["closed"], json!(["2026-12-25"]));
        assert_eq!(
            again.add(ts("2026-12-24T16:00:00Z"), 2 * 3600),
            ts("2026-12-28T10:00:00Z")
        );
        // Around the clock, nothing is closed.
        assert!(
            Calendar::Always
                .closed_on(["2026-12-25"])
                .is_open(ts("2026-12-25T12:00:00Z"))
        );
    }

    #[test]
    fn open_at_a_moment() {
        let c = nine_to_five("Europe/London");
        assert!(c.is_open(ts("2026-11-30T10:00:00Z")));
        assert!(!c.is_open(ts("2026-11-30T17:00:00Z")));
        assert!(!c.is_open(ts("2026-11-28T12:00:00Z")));
        assert!(Calendar::Always.is_open(0));
    }

    #[test]
    fn unusable_calendars() {
        assert!(Calendar::weekly("UTC", &json!({})).is_none());
        assert!(
            Calendar::weekly("Mars/Olympus", &json!({ "mon": [["09:00", "17:00"]] })).is_none()
        );
        assert!(matches!(
            Calendar::from_json(&json!({ "kind": "nope" })),
            Calendar::Always
        ));
    }
}
