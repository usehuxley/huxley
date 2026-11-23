//! Five-field cron expressions for schedule triggers:
//! `minute hour day-of-month month day-of-week`.
//!
//! Each field takes `*`, a value, a range `a-b`, a step `*/n` or `a-b/n`,
//! and comma-separated lists of those. Months and weekdays also take names
//! (`jan`, `mon`). Weekday 0 and 7 are both Sunday. As in classic cron, when
//! both day fields are restricted a day matches if either does.
//!
//! Times are evaluated in a time zone. A time skipped by a DST change runs at
//! the first moment after the gap; a time that occurs twice runs once.

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan as _, Zoned};

/// A parsed cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    days_restricted: bool,
    weekdays_restricted: bool,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn value(text: &str, min: u32, max: u32, names: &[&str]) -> Result<u32, String> {
    let lower = text.to_ascii_lowercase();
    if let Some(i) = names.iter().position(|n| *n == lower) {
        return Ok(min + u32::try_from(i).unwrap_or(0));
    }
    let n: u32 = text
        .parse()
        .map_err(|_| format!("{text:?} isn't a number"))?;
    if n < min || n > max {
        return Err(format!("{n} is outside {min}-{max}"));
    }
    Ok(n)
}

/// Parses one field into a bit set. Returns (bits, restricted).
fn field(text: &str, min: u32, max: u32, names: &[&str]) -> Result<(u64, bool), String> {
    let mut bits = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .parse()
                    .ok()
                    .filter(|s| *s > 0)
                    .ok_or_else(|| format!("{step:?} isn't a step"))?;
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            let (a, b) = (value(a, min, max, names)?, value(b, min, max, names)?);
            if a > b {
                return Err(format!("{range:?}: the range runs backwards"));
            }
            (a, b)
        } else {
            let v = value(range, min, max, names)?;
            // `5/15` means from 5 to the end, every 15.
            (v, if step > 1 { max } else { v })
        };
        let mut v = start;
        while v <= end {
            bits |= 1 << v;
            v += step;
        }
    }
    Ok((bits, text != "*"))
}

impl Cron {
    pub fn parse(text: &str) -> Result<Self, String> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields[..] else {
            return Err("use five fields: minute hour day-of-month month day-of-week".into());
        };
        let at = |name: &str, r: Result<(u64, bool), String>| r.map_err(|e| format!("{name}: {e}"));
        let (minutes, _) = at("minute", field(minute, 0, 59, &[]))?;
        let (hours, _) = at("hour", field(hour, 0, 23, &[]))?;
        let (days, days_restricted) = at("day of month", field(day, 1, 31, &[]))?;
        let (months, _) = at("month", field(month, 1, 12, &MONTHS))?;
        let (mut weekdays, weekdays_restricted) =
            at("day of week", field(weekday, 0, 7, &WEEKDAYS))?;
        if weekdays & (1 << 7) != 0 {
            weekdays |= 1;
        }
        Ok(Self {
            minutes,
            hours,
            days,
            months,
            weekdays,
            days_restricted,
            weekdays_restricted,
        })
    }

    fn day_matches(&self, date: Date) -> bool {
        let dom = self.days & (1 << date.day()) != 0;
        let dow = self.weekdays & (1 << date.weekday().to_sunday_zero_offset()) != 0;
        match (self.days_restricted, self.weekdays_restricted) {
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }

    /// The first matching time strictly after `after`, in `tz`. `None` if
    /// there is none within eight years (e.g. February 30th).
    pub fn next_after(&self, after: Timestamp, tz: &TimeZone) -> Option<Timestamp> {
        let local = after.to_zoned(tz.clone()).datetime();
        // The next whole minute.
        let mut t = local.with().second(0).subsec_nanosecond(0).build().ok()?;
        t = t.checked_add(1.minute()).ok()?;
        let limit = t.checked_add(8.years()).ok()?;
        while t < limit {
            if self.months & (1 << t.month()) == 0 {
                t = first_of_next_month(t)?;
            } else if !self.day_matches(t.date()) {
                t = t
                    .date()
                    .tomorrow()
                    .ok()?
                    .to_datetime(jiff::civil::Time::midnight());
            } else if self.hours & (1 << t.hour()) == 0 {
                t = t
                    .with()
                    .minute(0)
                    .build()
                    .ok()?
                    .checked_add(1.hour())
                    .ok()?;
            } else if self.minutes & (1 << t.minute()) == 0 {
                t = t.checked_add(1.minute()).ok()?;
            } else {
                let zoned: Zoned = t.to_zoned(tz.clone()).ok()?;
                let at = zoned.timestamp();
                // A repeated local time (DST fall-back) maps to its earlier
                // instant, which may not be after `after`: skip it.
                if at > after {
                    return Some(at);
                }
                t = t.checked_add(1.minute()).ok()?;
            }
        }
        None
    }
}

fn first_of_next_month(t: DateTime) -> Option<DateTime> {
    let first = t.date().first_of_month().checked_add(1.month()).ok()?;
    Some(first.to_datetime(jiff::civil::Time::midnight()))
}

/// Looks up an IANA time zone name.
pub fn time_zone(name: &str) -> Option<TimeZone> {
    TimeZone::get(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn next(cron: &str, after: &str, tz: &str) -> String {
        Cron::parse(cron)
            .unwrap()
            .next_after(at(after), &time_zone(tz).unwrap())
            .unwrap()
            .to_string()
    }

    #[test]
    fn parses_fields() {
        assert!(Cron::parse("*/15 9-17 * * mon-fri").is_ok());
        assert!(Cron::parse("0 0 1,15 jan,jul *").is_ok());
        for bad in [
            "* * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "5-1 * * * *",
            "*/0 * * * *",
            "x * * * *",
        ] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn finds_the_next_time() {
        assert_eq!(
            next("*/15 * * * *", "2026-01-01T10:07:30Z", "UTC"),
            "2026-01-01T10:15:00Z"
        );
        assert_eq!(
            next("0 9 * * mon-fri", "2026-01-02T09:00:00Z", "UTC"),
            "2026-01-05T09:00:00Z"
        );
        // 07:00 in London in summer is 06:00 UTC.
        assert_eq!(
            next("0 7 * * *", "2026-07-01T12:00:00Z", "Europe/London"),
            "2026-07-02T06:00:00Z"
        );
        // Day-of-month OR day-of-week when both are restricted.
        assert_eq!(
            next("0 0 13 * fri", "2026-02-01T00:00:00Z", "UTC"),
            "2026-02-06T00:00:00Z"
        );
        assert_eq!(
            next("0 0 29 2 *", "2026-03-01T00:00:00Z", "UTC"),
            "2028-02-29T00:00:00Z"
        );
        assert!(
            Cron::parse("0 0 30 2 *")
                .unwrap()
                .next_after(at("2026-01-01T00:00:00Z"), &TimeZone::UTC)
                .is_none()
        );
        // Sunday as 7; a step from a start value.
        assert_eq!(
            next("0 12 * * 7", "2026-01-01T00:00:00Z", "UTC"),
            "2026-01-04T12:00:00Z"
        );
        assert_eq!(
            next("5/20 * * * *", "2026-01-01T00:30:00Z", "UTC"),
            "2026-01-01T00:45:00Z"
        );
    }

    #[test]
    fn daylight_saving_changes() {
        // 02:30 doesn't exist in New York on 2026-03-08: it runs an hour later.
        assert_eq!(
            next("30 2 * * *", "2026-03-08T05:00:00Z", "America/New_York"),
            "2026-03-08T07:30:00Z"
        );
        // 01:30 happens twice on 2026-11-01; it runs once.
        let tz = time_zone("America/New_York").unwrap();
        let cron = Cron::parse("30 1 * * *").unwrap();
        let first = cron.next_after(at("2026-11-01T04:00:00Z"), &tz).unwrap();
        assert_eq!(first.to_string(), "2026-11-01T05:30:00Z");
        let second = cron.next_after(first, &tz).unwrap();
        assert_eq!(second.to_string(), "2026-11-02T06:30:00Z");
    }
}
