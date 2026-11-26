//! Capacity planning (ADR-058): each person's hours a week, from their
//! working schedule less approved time off, against what's booked:
//! appointments, and the remaining estimate of their open project tasks,
//! spread over each task's working days (start to due). Work already
//! overdue counts in the first week; tasks without dates are unscheduled.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value as Json;
use sqlx::PgConnection;
use time::{Date, Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::DeskResult;

/// Hours a weekday (Monday first) for people without a schedule.
const DEFAULT_DAY: [f64; 7] = [8.0, 8.0, 8.0, 8.0, 8.0, 0.0, 0.0];

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonWeek {
    /// Hours they work that week (schedule, less approved time off).
    pub capacity_hours: f64,
    pub appointment_hours: f64,
    /// Remaining estimates of their project tasks falling that week.
    pub project_hours: f64,
    pub booked_hours: f64,
    /// Booked beyond capacity.
    pub over: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonCapacity {
    pub user_id: Uuid,
    pub name: String,
    pub weeks: Vec<PersonWeek>,
    /// Remaining estimates of open tasks without dates.
    pub unscheduled_hours: f64,
    /// Open project tasks assigned to them.
    pub open_tasks: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Capacity {
    /// Each week's Monday, `YYYY-MM-DD`.
    pub weeks: Vec<String>,
    pub people: Vec<PersonCapacity>,
}

/// The Monday of `d`'s week.
pub fn monday(d: Date) -> Date {
    d - Duration::days(i64::from(d.weekday().number_days_from_monday()))
}

/// Hours a weekday from a schedule's `{"mon": [["09:00", "17:00"]], …}`.
fn day_hours(hours: &Json) -> [f64; 7] {
    let minutes = |s: &str| -> Option<f64> {
        let (h, m) = s.split_once(':')?;
        Some(h.parse::<f64>().ok()? * 60.0 + m.parse::<f64>().ok()?)
    };
    let mut out = [0.0; 7];
    for (i, key) in ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
        .iter()
        .enumerate()
    {
        for span in hours[key].as_array().into_iter().flatten() {
            if let (Some(a), Some(b)) = (
                span[0].as_str().and_then(minutes),
                span[1].as_str().and_then(minutes),
            ) && b > a
            {
                out[i] += (b - a) / 60.0;
            }
        }
    }
    out
}

/// A task's remaining hours, spread over the days it's worked.
struct Load {
    user: Uuid,
    remaining: f64,
    start: Option<Date>,
    due: Option<Date>,
}

type TaskRow = (Uuid, f64, Option<Date>, Option<Date>);

struct Calendar {
    from: Date,
    days: i64,
    hours: BTreeMap<Uuid, [f64; 7]>,
    off: BTreeSet<(Uuid, Date)>,
    /// Hours busy in their synced calendar each day (ADR-061).
    busy: BTreeMap<(Uuid, Date), f64>,
}

impl Calendar {
    fn works(&self, user: Uuid, d: Date) -> f64 {
        if self.off.contains(&(user, d)) {
            return 0.0;
        }
        let week = self.hours.get(&user).unwrap_or(&DEFAULT_DAY);
        let scheduled = week[usize::from(d.weekday().number_days_from_monday())];
        let busy = self.busy.get(&(user, d)).copied().unwrap_or(0.0);
        (scheduled - busy).max(0.0)
    }

    /// Which week a day falls in: earlier days count in the first.
    fn week(&self, d: Date) -> Option<usize> {
        let n = (d - self.from).whole_days().max(0);
        (n < self.days).then(|| usize::try_from(n / 7).unwrap_or(0))
    }

    /// Spreads a task's hours over its working days.
    fn spread(&self, load: &Load) -> Vec<(Date, f64)> {
        let (start, due) = match (load.start, load.due) {
            (Some(s), Some(d)) => (s, d),
            (Some(s), None) => (s, s),
            (None, Some(d)) => (d, d),
            (None, None) => return Vec::new(),
        };
        // Not before today's week starts: unfinished earlier work is due now.
        let start = start.max(self.from);
        let due = due.max(start);
        let mut days = Vec::new();
        let mut d = start;
        while d <= due && days.len() < 400 {
            if self.works(load.user, d) > 0.0 {
                days.push(d);
            }
            d += Duration::days(1);
        }
        if days.is_empty() {
            return vec![(due, load.remaining)];
        }
        #[allow(clippy::cast_precision_loss)]
        let each = load.remaining / days.len() as f64;
        days.into_iter().map(|d| (d, each)).collect()
    }
}

fn round(h: f64) -> f64 {
    (h * 10.0).round() / 10.0
}

/// Capacity for `weeks` weeks from `from`'s Monday: everyone with a
/// schedule, project tasks or appointments (or one team's members).
pub async fn plan(
    conn: &mut PgConnection,
    from: Date,
    weeks: u8,
    team: Option<Uuid>,
) -> DeskResult<Capacity> {
    let from = monday(from);
    let weeks = weeks.clamp(1, 26);
    let days = i64::from(weeks) * 7;
    let to = from + Duration::days(days);
    let tasks: Vec<TaskRow> = sqlx::query_as(
        "SELECT t.assignee_user_id, greatest(coalesce(k.estimate_hours::float8, 0) - \
                coalesce((SELECT sum(e.minutes) FROM sd_time_entries e WHERE e.ticket_id = k.ticket_id), 0)::float8 / 60, 0), \
                k.start_on, k.due_on \
           FROM prj_tasks k JOIN sd_tickets t ON t.id = k.ticket_id JOIN sd_statuses s ON s.id = t.status_id \
           JOIN prj_projects p ON p.id = k.project_id \
          WHERE t.assignee_user_id IS NOT NULL AND s.category NOT IN ('resolved', 'closed') \
            AND p.status IN ('planned', 'active')",
    )
    .fetch_all(&mut *conn)
    .await?;
    let appointments: Vec<(Uuid, OffsetDateTime, f64)> = sqlx::query_as(
        "SELECT user_id, starts_at, extract(epoch FROM ends_at - starts_at)::float8 / 3600 \
           FROM sd_appointments WHERE starts_at < $2::date AND ends_at > $1::date",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&mut *conn)
    .await?;
    let mut ids: BTreeSet<Uuid> = tasks.iter().map(|t| t.0).collect();
    ids.extend(appointments.iter().map(|a| a.0));
    let people: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT u.id, u.display_name FROM users u \
          WHERE u.status = 'active' AND u.kind = 'human' \
            AND CASE WHEN $2::uuid IS NULL \
                     THEN u.id = ANY($1) OR EXISTS (SELECT 1 FROM wf_schedules w WHERE w.user_id = u.id) \
                     ELSE EXISTS (SELECT 1 FROM team_members m WHERE m.team_id = $2 AND m.user_id = u.id) END \
          ORDER BY lower(u.display_name)",
    )
    .bind(ids.iter().copied().collect::<Vec<_>>())
    .bind(team)
    .fetch_all(&mut *conn)
    .await?;
    let who: Vec<Uuid> = people.iter().map(|p| p.0).collect();
    let schedules: Vec<(Uuid, Json)> =
        sqlx::query_as("SELECT user_id, hours FROM wf_schedules WHERE user_id = ANY($1)")
            .bind(&who)
            .fetch_all(&mut *conn)
            .await?;
    let time_off: Vec<(Uuid, Date, Date)> = sqlx::query_as(
        "SELECT user_id, starts_on, ends_on FROM wf_time_off \
          WHERE user_id = ANY($1) AND status = 'approved' AND starts_on < $3 AND ends_on >= $2",
    )
    .bind(&who)
    .bind(from)
    .bind(to)
    .fetch_all(&mut *conn)
    .await?;
    let busy = busy_hours(conn, &who, from, to).await?;
    let mut off = BTreeSet::new();
    for (u, s, e) in time_off {
        let mut d = s.max(from);
        while d <= e && d < to {
            off.insert((u, d));
            d += Duration::days(1);
        }
    }
    let cal = Calendar {
        from,
        days,
        hours: schedules
            .into_iter()
            .map(|(u, h)| (u, day_hours(&h)))
            .collect(),
        off,
        busy,
    };
    Ok(Capacity {
        weeks: (0..weeks)
            .map(|w| (from + Duration::days(i64::from(w) * 7)).to_string())
            .collect(),
        people: people
            .into_iter()
            .map(|(user, name)| person(&cal, weeks, user, name, &tasks, &appointments))
            .collect(),
    })
}

/// Hours each person is busy (or out of office) in their synced calendar,
/// by day (UTC), within `[from, to)`.
async fn busy_hours(
    conn: &mut PgConnection,
    who: &[Uuid],
    from: Date,
    to: Date,
) -> DeskResult<BTreeMap<(Uuid, Date), f64>> {
    let blocks: Vec<(Uuid, OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
        "SELECT user_id, starts_at, ends_at FROM wf_busy_blocks \
          WHERE user_id = ANY($1) AND status IN ('busy', 'oof') AND starts_at < $3::date AND ends_at > $2::date",
    )
    .bind(who)
    .bind(from)
    .bind(to)
    .fetch_all(conn)
    .await?;
    let mut out: BTreeMap<(Uuid, Date), f64> = BTreeMap::new();
    for (user, starts, ends) in blocks {
        let mut day = starts.date().max(from);
        while day < to && day <= ends.date() {
            let day_start = day.midnight().assume_utc();
            let day_end = day_start + Duration::days(1);
            let overlap = (ends.min(day_end) - starts.max(day_start)).as_seconds_f64() / 3600.0;
            if overlap > 0.0 {
                *out.entry((user, day)).or_default() += overlap;
            }
            day += Duration::days(1);
        }
    }
    Ok(out)
}

fn person(
    cal: &Calendar,
    weeks: u8,
    user: Uuid,
    name: String,
    tasks: &[TaskRow],
    appointments: &[(Uuid, OffsetDateTime, f64)],
) -> PersonCapacity {
    let mut out = vec![PersonWeek::default(); usize::from(weeks)];
    let mut d = cal.from;
    while d < cal.from + Duration::days(cal.days) {
        if let Some(w) = cal.week(d) {
            out[w].capacity_hours += cal.works(user, d);
        }
        d += Duration::days(1);
    }
    for (_, at, hours) in appointments.iter().filter(|a| a.0 == user) {
        if let Some(w) = cal.week(at.date()) {
            out[w].appointment_hours += hours;
        }
    }
    let mut unscheduled = 0.0;
    let mut open = 0;
    for (_, remaining, start, due) in tasks.iter().filter(|t| t.0 == user) {
        open += 1;
        if start.is_none() && due.is_none() {
            unscheduled += remaining;
            continue;
        }
        let load = Load {
            user,
            remaining: *remaining,
            start: *start,
            due: *due,
        };
        for (day, h) in cal.spread(&load) {
            if let Some(w) = cal.week(day) {
                out[w].project_hours += h;
            }
        }
    }
    for w in &mut out {
        w.booked_hours = round(w.appointment_hours + w.project_hours);
        w.capacity_hours = round(w.capacity_hours);
        w.appointment_hours = round(w.appointment_hours);
        w.project_hours = round(w.project_hours);
        w.over = w.booked_hours > w.capacity_hours + 0.05;
    }
    PersonCapacity {
        user_id: user,
        name,
        weeks: out,
        unscheduled_hours: round(unscheduled),
        open_tasks: open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::Weekday;

    fn date(s: &str) -> Date {
        crate::projects::parse_date(s, "d").unwrap()
    }

    #[test]
    fn schedules_and_spreading() {
        assert_eq!(
            day_hours(&json!({ "mon": [["09:00", "12:00"], ["13:00", "17:30"]], "sat": [] }))[0],
            7.5
        );
        assert_eq!(monday(date("2026-12-03")).weekday(), Weekday::Monday);
        let user = Uuid::nil();
        let cal = Calendar {
            from: date("2026-11-30"),
            days: 14,
            hours: BTreeMap::new(),
            off: [(user, date("2026-12-02"))].into_iter().collect(),
            busy: BTreeMap::new(),
        };
        // Mon to Fri with Wednesday off: four working days.
        let spread = cal.spread(&Load {
            user,
            remaining: 8.0,
            start: Some(date("2026-11-30")),
            due: Some(date("2026-12-04")),
        });
        assert_eq!(spread.len(), 4);
        assert!(spread.iter().all(|(_, h)| (*h - 2.0).abs() < 1e-9));
        // Overdue work lands in the first week.
        let late = cal.spread(&Load {
            user,
            remaining: 3.0,
            start: None,
            due: Some(date("2026-11-01")),
        });
        assert_eq!(cal.week(late[0].0), Some(0));
        assert_eq!(cal.week(date("2026-12-14")), None);
    }
}
