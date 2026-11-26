//! Timesheets (design doc §8.15): a person's time for a period, weekly by
//! default. They submit it; their team lead (or someone who manages the
//! workforce) approves it, which locks its entries, or rejects it with a
//! comment. Reopening is recorded and sends it back through review.
//!
//! A timesheet row exists once someone acts on a period; until then the
//! period reads as `open`. While a timesheet is submitted or approved, the
//! person's time entries on its days can't change (checked by the service
//! desk, and by a database trigger).

use std::collections::HashMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::{Date, Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{WorkforceError, WorkforceResult, text};

const YMD: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]");

fn ymd(d: Date) -> String {
    d.format(YMD).unwrap_or_default()
}

/// A `YYYY-MM-DD` date.
pub fn parse_day(field: &str, value: &str) -> WorkforceResult<Date> {
    Date::parse(value.trim(), YMD).map_err(|_| WorkforceError::invalid(field, "a date, YYYY-MM-DD"))
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TimesheetSettings)]
pub struct Settings {
    /// 7 (weekly) or 14 (every two weeks).
    pub period_days: i32,
    /// `YYYY-MM-DD`: a day periods start on.
    pub anchor: String,
    /// Billing previews use only time on approved timesheets.
    pub bill_approved_only: bool,
}

pub async fn settings(conn: &mut PgConnection) -> WorkforceResult<Settings> {
    let (period_days, anchor, bill_approved_only): (i32, Date, bool) = sqlx::query_as(
        "SELECT period_days, anchor, bill_approved_only FROM wf_timesheet_settings WHERE id = 1",
    )
    .fetch_one(conn)
    .await?;
    Ok(Settings {
        period_days,
        anchor: ymd(anchor),
        bill_approved_only,
    })
}

pub async fn set_settings(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &Settings,
) -> WorkforceResult<Settings> {
    if !matches!(input.period_days, 7 | 14) {
        return Err(WorkforceError::invalid("periodDays", "7 or 14"));
    }
    let anchor = parse_day("anchor", &input.anchor)?;
    sqlx::query(
        "UPDATE wf_timesheet_settings SET period_days = $1, anchor = $2, bill_approved_only = $3, \
                updated_at = now() WHERE id = 1",
    )
    .bind(input.period_days)
    .bind(anchor)
    .bind(input.bill_approved_only)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("timesheet.settings_changed").details(json!({
            "periodDays": input.period_days, "anchor": ymd(anchor),
            "billApprovedOnly": input.bill_approved_only,
        })),
    )
    .await?;
    settings(conn).await
}

/// The period (first and last day) containing `day`.
pub fn period_of(period_days: i32, anchor: Date, day: Date) -> (Date, Date) {
    let n = i64::from(period_days);
    let offset = (day - anchor).whole_days().rem_euclid(n);
    let start = day - Duration::days(offset);
    (start, start + Duration::days(n - 1))
}

async fn period_containing(conn: &mut PgConnection, day: Date) -> WorkforceResult<(Date, Date)> {
    let (period_days, anchor): (i32, Date) =
        sqlx::query_as("SELECT period_days, anchor FROM wf_timesheet_settings WHERE id = 1")
            .fetch_one(conn)
            .await?;
    Ok(period_of(period_days, anchor, day))
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TimesheetDay)]
pub struct Day {
    pub date: String,
    pub minutes: i32,
    pub billable_minutes: i32,
    /// From their working schedule (none without one); 0 on time off.
    pub expected_minutes: Option<i32>,
    /// Approved time off that day: its kind.
    pub time_off: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = TimesheetEntry)]
pub struct Entry {
    pub id: Uuid,
    pub worked_on: String,
    pub minutes: i32,
    pub billable: bool,
    pub work_type_name: String,
    pub ticket_id: Option<Uuid>,
    pub ticket_reference: Option<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = TimesheetEvent)]
pub struct Event {
    pub action: String,
    pub by: Option<String>,
    pub note: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A person's timesheet for a period, with its days and entries.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Timesheet {
    /// None until someone acts on the period.
    pub id: Option<Uuid>,
    pub user_id: Uuid,
    pub person: String,
    pub period_start: String,
    pub period_end: String,
    /// `open`, `submitted`, `approved` or `rejected`.
    pub status: String,
    pub version: i32,
    pub days: Vec<Day>,
    pub entries: Vec<Entry>,
    pub total_minutes: i32,
    pub billable_minutes: i32,
    pub expected_minutes: Option<i32>,
    /// Entries without notes.
    pub missing_notes: i32,
    pub history: Vec<Event>,
}

/// Minutes in a schedule's ranges for a weekday (`"mon"`…).
fn scheduled_minutes(hours: &Json, weekday: &str) -> i32 {
    let minutes_of = |s: &str| -> Option<i32> {
        let (h, m) = s.split_once(':')?;
        Some(h.parse::<i32>().ok()? * 60 + m.parse::<i32>().ok()?)
    };
    hours[weekday]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|range| {
            let from = minutes_of(range.get(0)?.as_str()?)?;
            let to = minutes_of(range.get(1)?.as_str()?)?;
            Some((to - from).max(0))
        })
        .sum()
}

fn weekday_key(d: Date) -> &'static str {
    match d.weekday() {
        time::Weekday::Monday => "mon",
        time::Weekday::Tuesday => "tue",
        time::Weekday::Wednesday => "wed",
        time::Weekday::Thursday => "thu",
        time::Weekday::Friday => "fri",
        time::Weekday::Saturday => "sat",
        time::Weekday::Sunday => "sun",
    }
}

/// `user_id`'s timesheet for the period containing `day`.
pub async fn for_day(
    conn: &mut PgConnection,
    user_id: Uuid,
    day: Date,
) -> WorkforceResult<Timesheet> {
    let (start, end) = period_containing(&mut *conn, day).await?;
    build(conn, user_id, start, end).await
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> WorkforceResult<Timesheet> {
    let (user_id, start, end): (Uuid, Date, Date) =
        sqlx::query_as("SELECT user_id, period_start, period_end FROM wf_timesheets WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(WorkforceError::NotFound)?;
    build(conn, user_id, start, end).await
}

/// Each day of the period: logged, billable and expected minutes.
fn days_of(
    start: Date,
    end: Date,
    entries: &[Entry],
    schedule: Option<&Json>,
    time_off: &[(Date, Date, String)],
) -> Vec<Day> {
    let mut by_day: HashMap<String, (i32, i32)> = HashMap::new();
    for e in entries {
        let d = by_day.entry(e.worked_on.clone()).or_default();
        d.0 += e.minutes;
        if e.billable {
            d.1 += e.minutes;
        }
    }
    let mut days = Vec::new();
    let mut day = start;
    while day <= end {
        let off = time_off
            .iter()
            .find(|(from, to, _)| *from <= day && day <= *to)
            .map(|(_, _, kind)| kind.clone());
        let expected = schedule.as_ref().map(|h| {
            if off.is_some() {
                0
            } else {
                scheduled_minutes(h, weekday_key(day))
            }
        });
        let (minutes, billable) = by_day.get(&ymd(day)).copied().unwrap_or_default();
        days.push(Day {
            date: ymd(day),
            minutes,
            billable_minutes: billable,
            expected_minutes: expected,
            time_off: off,
        });
        day += Duration::days(1);
    }
    days
}

async fn build(
    conn: &mut PgConnection,
    user_id: Uuid,
    start: Date,
    end: Date,
) -> WorkforceResult<Timesheet> {
    let person: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(WorkforceError::NotFound)?;
    let row: Option<(Uuid, String, i32)> = sqlx::query_as(
        "SELECT id, status, version FROM wf_timesheets WHERE user_id = $1 AND period_start = $2",
    )
    .bind(user_id)
    .bind(start)
    .fetch_optional(&mut *conn)
    .await?;
    let entries: Vec<Entry> = sqlx::query_as(
        "SELECT e.id, to_char(e.worked_on, 'YYYY-MM-DD') AS worked_on, e.minutes, e.billable, \
                w.name AS work_type_name, e.ticket_id, t.number_prefix || '-' || t.number AS ticket_reference, e.notes \
           FROM sd_time_entries e JOIN sd_work_types w ON w.id = e.work_type_id \
           LEFT JOIN sd_tickets t ON t.id = e.ticket_id \
          WHERE e.user_id = $1 AND e.worked_on BETWEEN $2 AND $3 \
          ORDER BY e.worked_on, e.created_at, e.id",
    )
    .bind(user_id)
    .bind(start)
    .bind(end)
    .fetch_all(&mut *conn)
    .await?;
    let schedule: Option<Json> =
        sqlx::query_scalar("SELECT hours FROM wf_schedules WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await?;
    let time_off: Vec<(Date, Date, String)> = sqlx::query_as(
        "SELECT starts_on, ends_on, kind FROM wf_time_off \
          WHERE user_id = $1 AND status = 'approved' AND starts_on <= $3 AND ends_on >= $2",
    )
    .bind(user_id)
    .bind(start)
    .bind(end)
    .fetch_all(&mut *conn)
    .await?;
    let days = days_of(start, end, &entries, schedule.as_ref(), &time_off);
    let history: Vec<Event> = match row {
        Some((id, _, _)) => {
            sqlx::query_as(
                "SELECT e.action, u.display_name AS by, e.note, e.at FROM wf_timesheet_events e \
                   LEFT JOIN users u ON u.id = e.by_user WHERE e.timesheet_id = $1 ORDER BY e.at, e.id",
            )
            .bind(id)
            .fetch_all(&mut *conn)
            .await?
        }
        None => Vec::new(),
    };
    Ok(Timesheet {
        id: row.as_ref().map(|r| r.0),
        user_id,
        person,
        period_start: ymd(start),
        period_end: ymd(end),
        status: row
            .as_ref()
            .map_or_else(|| "open".to_owned(), |r| r.1.clone()),
        version: row.as_ref().map_or(0, |r| r.2),
        total_minutes: days.iter().map(|d| d.minutes).sum(),
        billable_minutes: days.iter().map(|d| d.billable_minutes).sum(),
        expected_minutes: schedule
            .as_ref()
            .map(|_| days.iter().filter_map(|d| d.expected_minutes).sum()),
        missing_notes: i32::try_from(entries.iter().filter(|e| e.notes.trim().is_empty()).count())
            .unwrap_or(i32::MAX),
        days,
        entries,
        history,
    })
}

/// Whether `approver` may review `person`'s timesheets: someone else, who
/// leads a team they're in, or (with `manages_workforce`) anyone with no
/// team lead.
pub async fn can_review(
    conn: &mut PgConnection,
    approver: Uuid,
    person: Uuid,
    manages_workforce: bool,
) -> WorkforceResult<bool> {
    if approver == person {
        return Ok(false);
    }
    let (leads_them, has_lead): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM team_members l JOIN team_members m ON m.team_id = l.team_id \
                         WHERE l.user_id = $1 AND l.lead AND m.user_id = $2), \
                EXISTS (SELECT 1 FROM team_members l JOIN team_members m ON m.team_id = l.team_id \
                         WHERE l.lead AND l.user_id <> $2 AND m.user_id = $2)",
    )
    .bind(approver)
    .bind(person)
    .fetch_one(conn)
    .await?;
    Ok(leads_them || (manages_workforce && !has_lead))
}

async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    action: &str,
    by: Uuid,
    note: &str,
) -> WorkforceResult<Timesheet> {
    sqlx::query(
        "INSERT INTO wf_timesheet_events (id, timesheet_id, action, by_user, note) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(huxley_core::new_id())
    .bind(id)
    .bind(action)
    .bind(by)
    .bind(note)
    .execute(&mut *conn)
    .await?;
    let sheet = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(&format!("timesheet.{action}"))
            .target("user", sheet.user_id)
            .details(json!({
                "timesheet": id, "periodStart": sheet.period_start, "periodEnd": sheet.period_end,
                "note": note,
            })),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: &format!("timesheet.{action}"),
            org_id: None,
            subject_type: "user",
            subject_id: sheet.user_id,
            payload: json!({
                "timesheet_id": id,
                "user_id": sheet.user_id,
                "person": sheet.person,
                "period_start": sheet.period_start,
                "period_end": sheet.period_end,
                "status": sheet.status,
                "total_minutes": sheet.total_minutes,
                "note": note,
            }),
        },
    )
    .await?;
    Ok(sheet)
}

/// Submits `user_id`'s timesheet for the period starting `period_start`
/// (open or rejected; not a future period).
pub async fn submit(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    period_start: Date,
    note: Option<&str>,
    today: Date,
) -> WorkforceResult<Timesheet> {
    let note = text("note", note, 2000)?.unwrap_or_default();
    let (start, end) = period_containing(&mut *conn, period_start).await?;
    if start != period_start {
        return Err(WorkforceError::invalid(
            "periodStart",
            format!("periods start on days like {}", ymd(start)),
        ));
    }
    if start > today {
        return Err(WorkforceError::invalid(
            "periodStart",
            "that period hasn't started",
        ));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO wf_timesheets (id, user_id, period_start, period_end) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (user_id, period_start) DO UPDATE SET user_id = EXCLUDED.user_id RETURNING id",
    )
    .bind(huxley_core::new_id())
    .bind(user_id)
    .bind(start)
    .bind(end)
    .fetch_one(&mut *conn)
    .await?;
    let changed = sqlx::query(
        "UPDATE wf_timesheets SET status = 'submitted', submitted_at = now(), decided_by = NULL, \
                decided_at = NULL, version = version + 1 \
          WHERE id = $1 AND status IN ('open', 'rejected')",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(WorkforceError::invalid(
            "status",
            "it's already submitted or approved",
        ));
    }
    record(conn, ctx, id, "submitted", user_id, &note).await
}

/// Approves or rejects a submitted timesheet (`by` may review it; a
/// rejection needs a comment).
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    approve: bool,
    by: Uuid,
    note: Option<&str>,
) -> WorkforceResult<Timesheet> {
    let note = text("note", note, 2000)?.unwrap_or_default();
    if !approve && note.is_empty() {
        return Err(WorkforceError::invalid("note", "say what needs changing"));
    }
    let changed = sqlx::query(
        "UPDATE wf_timesheets SET status = $2, decided_by = $3, decided_at = now(), version = version + 1 \
          WHERE id = $1 AND status = 'submitted' AND user_id <> $3",
    )
    .bind(id)
    .bind(if approve { "approved" } else { "rejected" })
    .bind(by)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        get(&mut *conn, id).await?;
        return Err(WorkforceError::invalid(
            "status",
            "only someone else's submitted timesheet can be decided",
        ));
    }
    record(
        conn,
        ctx,
        id,
        if approve { "approved" } else { "rejected" },
        by,
        &note,
    )
    .await
}

/// Reopens a timesheet: a reviewer reopens a submitted or approved one
/// (`reviewer`), or its owner withdraws a submitted one. Its entries can
/// change again, and it goes back through review.
pub async fn reopen(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    by: Uuid,
    reviewer: bool,
    note: Option<&str>,
) -> WorkforceResult<Timesheet> {
    let note = text("note", note, 2000)?.unwrap_or_default();
    let (owner, status): (Uuid, String) =
        sqlx::query_as("SELECT user_id, status FROM wf_timesheets WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(WorkforceError::NotFound)?;
    let action = match (status.as_str(), owner == by, reviewer) {
        ("submitted", true, _) => "withdrawn",
        ("submitted" | "approved", false, true) => "reopened",
        ("approved", true, _) => {
            return Err(WorkforceError::invalid(
                "status",
                "ask your lead to reopen an approved timesheet",
            ));
        }
        _ => {
            return Err(WorkforceError::invalid(
                "status",
                format!("a {status} timesheet can't be reopened"),
            ));
        }
    };
    if action == "reopened" && note.is_empty() {
        return Err(WorkforceError::invalid(
            "note",
            "say why it's being reopened",
        ));
    }
    sqlx::query(
        "UPDATE wf_timesheets SET status = 'open', decided_by = NULL, decided_at = NULL, version = version + 1 \
          WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    record(conn, ctx, id, action, by, &note).await
}

/// A timesheet in a list.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TimesheetSummary {
    pub id: Uuid,
    pub user_id: Uuid,
    pub person: String,
    pub period_start: String,
    pub period_end: String,
    pub status: String,
    pub total_minutes: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<OffsetDateTime>,
}

/// Timesheets with a status (and of some people, if given), oldest first.
pub async fn list(
    conn: &mut PgConnection,
    status: Option<&str>,
    people: Option<&[Uuid]>,
) -> WorkforceResult<Vec<TimesheetSummary>> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.user_id, u.display_name AS person, to_char(s.period_start, 'YYYY-MM-DD') AS period_start, \
                to_char(s.period_end, 'YYYY-MM-DD') AS period_end, s.status, \
                COALESCE((SELECT sum(minutes) FROM sd_time_entries e \
                           WHERE e.user_id = s.user_id AND e.worked_on BETWEEN s.period_start AND s.period_end), 0)::int \
                  AS total_minutes, s.submitted_at \
           FROM wf_timesheets s JOIN users u ON u.id = s.user_id \
          WHERE ($1::text IS NULL OR s.status = $1) AND ($2::uuid[] IS NULL OR s.user_id = ANY($2)) \
          ORDER BY s.period_start, u.display_name LIMIT 500",
    )
    .bind(status)
    .bind(people)
    .fetch_all(conn)
    .await?)
}

/// The people `approver` reviews as a team lead.
pub async fn led_by(conn: &mut PgConnection, approver: Uuid) -> WorkforceResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT m.user_id FROM team_members l JOIN team_members m ON m.team_id = l.team_id \
          WHERE l.user_id = $1 AND l.lead AND m.user_id <> $1",
    )
    .bind(approver)
    .fetch_all(conn)
    .await?)
}

/// A line of the payroll export: approved time and approved time off.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PayrollRow {
    pub person: String,
    pub email: String,
    pub date: String,
    /// A work type, or `Time off: <kind>`.
    pub kind: String,
    pub minutes: i32,
    pub billable: bool,
}

/// Approved timesheets' time (by person, day and work type) and approved
/// time off (as each scheduled day's minutes, or 8 hours without a
/// schedule) between two days.
pub async fn payroll(
    conn: &mut PgConnection,
    from: Date,
    to: Date,
) -> WorkforceResult<Vec<PayrollRow>> {
    let mut rows: Vec<PayrollRow> = sqlx::query_as(
        "SELECT u.display_name AS person, u.email, to_char(e.worked_on, 'YYYY-MM-DD') AS date, \
                w.name AS kind, sum(e.minutes)::int AS minutes, e.billable \
           FROM sd_time_entries e \
           JOIN wf_timesheets s ON s.user_id = e.user_id AND s.status = 'approved' \
                AND e.worked_on BETWEEN s.period_start AND s.period_end \
           JOIN users u ON u.id = e.user_id JOIN sd_work_types w ON w.id = e.work_type_id \
          WHERE e.worked_on BETWEEN $1 AND $2 \
          GROUP BY u.display_name, u.email, e.worked_on, w.name, e.billable",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&mut *conn)
    .await?;
    let off: Vec<(String, String, Uuid, Date, Date, String)> = sqlx::query_as(
        "SELECT u.display_name, u.email, u.id, t.starts_on, t.ends_on, t.kind FROM wf_time_off t \
           JOIN users u ON u.id = t.user_id \
          WHERE t.status = 'approved' AND t.starts_on <= $2 AND t.ends_on >= $1",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&mut *conn)
    .await?;
    for (person, email, user, starts, ends, kind) in off {
        let schedule: Option<Json> =
            sqlx::query_scalar("SELECT hours FROM wf_schedules WHERE user_id = $1")
                .bind(user)
                .fetch_optional(&mut *conn)
                .await?;
        let mut day = starts.max(from);
        while day <= ends.min(to) {
            let minutes = schedule.as_ref().map_or_else(
                || {
                    if day.weekday().number_from_monday() <= 5 {
                        480
                    } else {
                        0
                    }
                },
                |h| scheduled_minutes(h, weekday_key(day)),
            );
            if minutes > 0 {
                rows.push(PayrollRow {
                    person: person.clone(),
                    email: email.clone(),
                    date: ymd(day),
                    kind: format!("Time off: {kind}"),
                    minutes,
                    billable: false,
                });
            }
            day += Duration::days(1);
        }
    }
    rows.sort_by(|a, b| (&a.person, &a.date, &a.kind).cmp(&(&b.person, &b.date, &b.kind)));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn periods_follow_the_anchor() {
        let monday = date!(2024 - 01 - 01);
        assert_eq!(
            period_of(7, monday, date!(2026 - 11 - 26)),
            (date!(2026 - 11 - 23), date!(2026 - 11 - 29))
        );
        assert_eq!(
            period_of(7, monday, date!(2026 - 11 - 23)),
            (date!(2026 - 11 - 23), date!(2026 - 11 - 29))
        );
        // Before the anchor too.
        assert_eq!(
            period_of(7, monday, date!(2023 - 12 - 31)),
            (date!(2023 - 12 - 25), date!(2023 - 12 - 31))
        );
        let (start, end) = period_of(14, monday, date!(2026 - 11 - 26));
        assert_eq!((end - start).whole_days(), 13);
        assert!(start <= date!(2026 - 11 - 26) && date!(2026 - 11 - 26) <= end);
    }

    #[test]
    fn schedules_give_expected_minutes() {
        let hours = json!({ "mon": [["09:00", "12:00"], ["13:00", "17:30"]], "sat": [] });
        assert_eq!(scheduled_minutes(&hours, "mon"), 450);
        assert_eq!(scheduled_minutes(&hours, "sat"), 0);
        assert_eq!(scheduled_minutes(&hours, "sun"), 0);
    }
}
