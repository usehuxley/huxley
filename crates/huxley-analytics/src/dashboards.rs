//! Dashboards (design doc §8.12): a grid of widgets, each a query spec and
//! how to show it. Shared dashboards belong to an organization (for it and
//! those below); personal ones to their owner. Prebuilt dashboards are
//! defined here, in code, and can be copied to change them.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::query::{QuerySpec, Scope, compile, ranges};
use crate::{AnalyticsError, AnalyticsResult};

pub const MAX_WIDGETS: usize = 30;

/// How a widget shows its numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Viz {
    /// One number.
    Number,
    Table,
    Bar,
    Line,
    Area,
    Pie,
}

fn half() -> u8 {
    6
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Widget {
    /// Unique within the dashboard.
    pub id: String,
    pub title: String,
    pub viz: Viz,
    pub spec: QuerySpec,
    /// Grid columns, 1 to 12.
    #[serde(default = "half")]
    pub width: u8,
    /// Ignore the dashboard's range (e.g. "open now").
    #[serde(default)]
    pub fixed_range: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    /// A uuid, or a prebuilt dashboard's key.
    pub id: String,
    pub org_id: Option<Uuid>,
    pub owner_user_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub range: String,
    pub widgets: Vec<Widget>,
    /// Defined in code; copy it to change it.
    pub builtin: bool,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub widgets: Vec<Widget>,
    #[serde(default)]
    pub version: Option<i32>,
}

fn bad(at: impl Into<String>, message: impl Into<String>) -> AnalyticsError {
    AnalyticsError::Invalid(at.into(), message.into())
}

/// Checks a dashboard: its range, and each widget's spec and shape.
pub fn check(input: &DashboardInput) -> AnalyticsResult<String> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(bad("name", "1 to 100 characters"));
    }
    let range = input.range.clone().unwrap_or_else(|| "last_30_days".into());
    if !ranges().any(|r| r == range) {
        return Err(bad(
            "range",
            format!("one of {}", ranges().collect::<Vec<_>>().join(", ")),
        ));
    }
    if input.widgets.len() > MAX_WIDGETS {
        return Err(bad("widgets", format!("at most {MAX_WIDGETS}")));
    }
    let mut ids: Vec<&str> = Vec::new();
    for (i, w) in input.widgets.iter().enumerate() {
        let at = format!("widgets/{i}");
        if w.id.is_empty() || w.id.len() > 40 || ids.contains(&w.id.as_str()) {
            return Err(bad(
                format!("{at}/id"),
                "a unique id of up to 40 characters",
            ));
        }
        ids.push(&w.id);
        if w.title.trim().is_empty() || w.title.chars().count() > 100 {
            return Err(bad(format!("{at}/title"), "1 to 100 characters"));
        }
        if !(1..=12).contains(&w.width) {
            return Err(bad(format!("{at}/width"), "1 to 12 columns"));
        }
        compile(&w.spec, &Scope::All).map_err(|e| match e {
            AnalyticsError::Invalid(field, message) => bad(format!("{at}/spec/{field}"), message),
            other => other,
        })?;
        let (dims, measures) = (w.spec.dimensions.len(), w.spec.measures.len());
        let ok = match w.viz {
            Viz::Number => dims == 0 && measures == 1,
            Viz::Pie => dims == 1 && measures == 1,
            Viz::Bar | Viz::Line | Viz::Area => (1..=2).contains(&dims) && measures >= 1,
            Viz::Table => true,
        };
        if !ok {
            return Err(bad(
                format!("{at}/viz"),
                match w.viz {
                    Viz::Number => "a number shows one measure, not grouped",
                    Viz::Pie => "a pie shows one measure by one grouping",
                    _ => "a chart needs one or two groupings and a measure",
                },
            ));
        }
    }
    Ok(range)
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    org_id: Uuid,
    owner_user_id: Option<Uuid>,
    name: String,
    description: Option<String>,
    range: String,
    widgets: sqlx::types::Json<Vec<Widget>>,
    version: i32,
    updated_at: OffsetDateTime,
}

impl From<Row> for Dashboard {
    fn from(r: Row) -> Self {
        Self {
            id: r.id.to_string(),
            org_id: Some(r.org_id),
            owner_user_id: r.owner_user_id,
            name: r.name,
            description: r.description,
            range: r.range,
            widgets: r.widgets.0,
            builtin: false,
            version: r.version,
            updated_at: Some(r.updated_at),
        }
    }
}

const SELECT: &str = "SELECT id, org_id, owner_user_id, name, description, range, widgets, version, updated_at FROM dashboards";

/// Shared dashboards in the transaction's scope and `user`'s own, by name.
pub async fn list(conn: &mut PgConnection, user: Uuid) -> AnalyticsResult<Vec<Dashboard>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE owner_user_id IS NULL OR owner_user_id = $1 ORDER BY lower(name), id"
    )))
    .bind(user)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(Dashboard::from).collect())
}

/// One stored dashboard, if it's shared or `user`'s.
pub async fn get(
    conn: &mut PgConnection,
    user: Uuid,
    id: Uuid,
) -> AnalyticsResult<Option<Dashboard>> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE id = $1 AND (owner_user_id IS NULL OR owner_user_id = $2)"
    )))
    .bind(id)
    .bind(user)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(Dashboard::from))
}

/// Adds a dashboard at `org_id`: personal to `owner`, or shared.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    owner: Option<Uuid>,
    input: &DashboardInput,
) -> AnalyticsResult<Dashboard> {
    let range = check(input)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dashboards (id, org_id, owner_user_id, name, description, range, widgets, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(org_id)
    .bind(owner)
    .bind(input.name.trim())
    .bind(input.description.as_deref().map(str::trim).filter(|d| !d.is_empty()))
    .bind(&range)
    .bind(sqlx::types::Json(&input.widgets))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("dashboard.created")
            .org(org_id)
            .target("dashboard", id)
            .details(json!({ "name": input.name.trim(), "personal": owner.is_some() })),
    )
    .await
    .map_err(|e| AnalyticsError::Db(sqlx::Error::Protocol(e.to_string())))?;
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
        .bind(id)
        .fetch_one(conn)
        .await?;
    Ok(row.into())
}

/// Replaces a dashboard (send the version you read). `Ok(None)`: no such
/// dashboard, or someone changed it meanwhile.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &DashboardInput,
) -> AnalyticsResult<Option<Dashboard>> {
    let range = check(input)?;
    let row: Option<Row> = sqlx::query_as(
        "UPDATE dashboards SET name = $2, description = $3, range = $4, widgets = $5, version = version + 1, \
                updated_at = now() WHERE id = $1 AND ($6::integer IS NULL OR version = $6) \
         RETURNING id, org_id, owner_user_id, name, description, range, widgets, version, updated_at",
    )
    .bind(id)
    .bind(input.name.trim())
    .bind(input.description.as_deref().map(str::trim).filter(|d| !d.is_empty()))
    .bind(&range)
    .bind(sqlx::types::Json(&input.widgets))
    .bind(input.version)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(r) = &row {
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("dashboard.updated")
                .org(r.org_id)
                .target("dashboard", id),
        )
        .await
        .map_err(|e| AnalyticsError::Db(sqlx::Error::Protocol(e.to_string())))?;
    }
    Ok(row.map(Dashboard::from))
}

pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> AnalyticsResult<bool> {
    let org: Option<Uuid> =
        sqlx::query_scalar("DELETE FROM dashboards WHERE id = $1 RETURNING org_id")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(org) = org {
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("dashboard.deleted")
                .org(org)
                .target("dashboard", id),
        )
        .await
        .map_err(|e| AnalyticsError::Db(sqlx::Error::Protocol(e.to_string())))?;
    }
    Ok(org.is_some())
}

fn widget(
    id: &str,
    title: &str,
    viz: Viz,
    width: u8,
    fixed_range: bool,
    spec: serde_json::Value,
) -> Widget {
    Widget {
        id: id.into(),
        title: title.into(),
        viz,
        spec: serde_json::from_value(spec).expect("a valid prebuilt widget"),
        width,
        fixed_range,
    }
}

fn prebuilt(key: &str, name: &str, description: &str, widgets: Vec<Widget>) -> Dashboard {
    Dashboard {
        id: key.into(),
        org_id: None,
        owner_user_id: None,
        name: name.into(),
        description: Some(description.into()),
        range: "last_30_days".into(),
        widgets,
        builtin: true,
        version: 1,
        updated_at: None,
    }
}

/// The prebuilt dashboards.
#[allow(clippy::too_many_lines)]
pub fn builtin() -> Vec<Dashboard> {
    let count = json!([{ "fn": "count" }]);
    let hours = json!([{ "fn": "sum", "field": "hours" }]);
    let open = json!([{ "field": "open", "op": "eq", "value": true }]);
    vec![
        prebuilt(
            "service-desk",
            "Service desk overview",
            "What's open, where it sits, and what's coming in.",
            vec![
                widget(
                    "open",
                    "Open tickets",
                    Viz::Number,
                    3,
                    true,
                    json!({ "source": "tickets", "measures": count, "filters": open }),
                ),
                widget(
                    "new",
                    "Tickets raised",
                    Viz::Number,
                    3,
                    false,
                    json!({ "source": "tickets", "measures": count }),
                ),
                widget(
                    "resolved",
                    "Tickets resolved",
                    Viz::Number,
                    3,
                    false,
                    json!({ "source": "tickets", "measures": count,
                "filters": [{ "field": "resolved_at", "op": "not_null" }] }),
                ),
                widget(
                    "response",
                    "Average minutes to first response",
                    Viz::Number,
                    3,
                    false,
                    json!({ "source": "tickets",
                "measures": [{ "fn": "avg", "field": "response_minutes" }] }),
                ),
                widget(
                    "per-day",
                    "Tickets raised per day",
                    Viz::Line,
                    12,
                    false,
                    json!({ "source": "tickets", "measures": count,
                "dimensions": [{ "field": "created_at", "granularity": "day" }], "limit": 400 }),
                ),
                widget(
                    "by-queue",
                    "Open by queue",
                    Viz::Bar,
                    6,
                    true,
                    json!({ "source": "tickets", "measures": count,
                "dimensions": [{ "field": "queue" }], "filters": open, "limit": 12 }),
                ),
                widget(
                    "by-priority",
                    "Open by priority",
                    Viz::Bar,
                    6,
                    true,
                    json!({ "source": "tickets", "measures": count,
                "dimensions": [{ "field": "priority" }], "filters": open, "sort": [{ "by": "priority" }] }),
                ),
                widget(
                    "by-assignee",
                    "Open by assignee",
                    Viz::Table,
                    12,
                    true,
                    json!({ "source": "tickets", "measures": count,
                "dimensions": [{ "field": "assignee" }], "filters": open, "limit": 25 }),
                ),
            ],
        ),
        prebuilt(
            "sla",
            "SLA performance",
            "Whether first responses and resolutions met their targets.",
            vec![
                widget(
                    "response-met",
                    "First responses on time",
                    Viz::Pie,
                    6,
                    false,
                    json!({ "source": "tickets", "measures": count,
                "dimensions": [{ "field": "response_met" }], "filters": [{ "field": "response_met", "op": "not_null" }] }),
                ),
                widget(
                    "resolution-met",
                    "Resolutions on time",
                    Viz::Pie,
                    6,
                    false,
                    json!({ "source": "tickets", "measures": count,
                "dimensions": [{ "field": "resolution_met" }], "filters": [{ "field": "resolution_met", "op": "not_null" }] }),
                ),
                widget(
                    "hours-by-priority",
                    "Average hours to resolve, by priority",
                    Viz::Bar,
                    6,
                    false,
                    json!({ "source": "tickets",
                "measures": [{ "fn": "avg", "field": "resolution_hours" }], "dimensions": [{ "field": "priority" }],
                "filters": [{ "field": "resolved_at", "op": "not_null" }], "sort": [{ "by": "priority" }] }),
                ),
                widget(
                    "missed-by-queue",
                    "Missed resolutions by queue",
                    Viz::Bar,
                    6,
                    false,
                    json!({ "source": "tickets", "measures": count,
                "dimensions": [{ "field": "queue" }], "filters": [{ "field": "resolution_met", "op": "eq", "value": false }], "limit": 12 }),
                ),
            ],
        ),
        prebuilt(
            "utilization",
            "Technician utilization",
            "Hours logged by each technician, and how much is billable.",
            vec![
                widget(
                    "hours",
                    "Hours logged",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "time_entries", "measures": hours }),
                ),
                widget(
                    "billable",
                    "Billable hours",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "time_entries", "measures": hours,
                "filters": [{ "field": "billable", "op": "eq", "value": true }] }),
                ),
                widget(
                    "people",
                    "Technicians logging time",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "time_entries",
                "measures": [{ "fn": "count_distinct", "field": "technician" }] }),
                ),
                widget(
                    "by-tech",
                    "Hours by technician, billable or not",
                    Viz::Bar,
                    12,
                    false,
                    json!({ "source": "time_entries", "measures": hours,
                "dimensions": [{ "field": "technician" }, { "field": "billable" }], "limit": 60 }),
                ),
                widget(
                    "per-week",
                    "Hours per week",
                    Viz::Area,
                    12,
                    false,
                    json!({ "source": "time_entries", "measures": hours,
                "dimensions": [{ "field": "worked_on", "granularity": "week" }], "limit": 60 }),
                ),
            ],
        ),
        prebuilt(
            "clients",
            "Client overview",
            "Tickets, time and assets across your clients.",
            vec![
                widget(
                    "open-by-client",
                    "Open tickets by client",
                    Viz::Bar,
                    6,
                    true,
                    json!({ "source": "tickets", "measures": count,
                "dimensions": [{ "field": "client" }], "filters": open, "limit": 15 }),
                ),
                widget(
                    "hours-by-client",
                    "Hours by client",
                    Viz::Bar,
                    6,
                    false,
                    json!({ "source": "time_entries", "measures": hours,
                "dimensions": [{ "field": "client" }], "limit": 15 }),
                ),
                widget(
                    "assets-by-client",
                    "Assets by client",
                    Viz::Table,
                    12,
                    true,
                    json!({ "source": "assets", "measures": count,
                "dimensions": [{ "field": "client" }], "filters": [{ "field": "status", "op": "not_in", "value": ["retired", "disposed"] }], "limit": 50 }),
                ),
            ],
        ),
        prebuilt(
            "time",
            "Time and utilization",
            "Billable and non-billable hours by client, work type and technician.",
            vec![
                widget(
                    "billable-split",
                    "Billable or not",
                    Viz::Pie,
                    4,
                    false,
                    json!({ "source": "time_entries", "measures": hours,
                "dimensions": [{ "field": "billable" }] }),
                ),
                widget(
                    "by-work-type",
                    "Hours by work type",
                    Viz::Bar,
                    8,
                    false,
                    json!({ "source": "time_entries", "measures": hours,
                "dimensions": [{ "field": "work_type" }], "limit": 12 }),
                ),
                widget(
                    "by-client",
                    "Hours by client, billable or not",
                    Viz::Bar,
                    12,
                    false,
                    json!({ "source": "time_entries", "measures": hours,
                "dimensions": [{ "field": "client" }, { "field": "billable" }], "limit": 60 }),
                ),
                widget(
                    "tech-week",
                    "Hours by technician and week",
                    Viz::Table,
                    12,
                    false,
                    json!({ "source": "time_entries", "measures": hours,
                "dimensions": [{ "field": "technician" }, { "field": "worked_on", "granularity": "week" }],
                "sort": [{ "by": "technician" }, { "by": "worked_on" }], "limit": 200 }),
                ),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prebuilt_dashboards_are_valid() {
        for d in builtin() {
            let input = DashboardInput {
                name: d.name.clone(),
                description: d.description.clone(),
                range: Some(d.range.clone()),
                widgets: d.widgets.clone(),
                version: None,
            };
            check(&input).unwrap_or_else(|e| panic!("{}: {e}", d.id));
        }
    }
}
