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
    /// Shown in the client portal (shared dashboards only).
    pub portal: bool,
    /// Who sees a shared dashboard: members of these teams or holders of
    /// these roles (both empty: everyone who can see dashboards there).
    pub audience_teams: Vec<Uuid>,
    pub audience_roles: Vec<Uuid>,
    pub created_by: Option<Uuid>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    /// A prebuilt dashboard's module: it's listed only while that's on.
    #[serde(skip)]
    pub module: Option<&'static str>,
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
    /// Show it in the client portal (a shared dashboard only).
    #[serde(default)]
    pub portal: bool,
    /// Share a personal dashboard with its organization (changing it then
    /// needs `dashboard:manage`).
    #[serde(default)]
    pub shared: Option<bool>,
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
        // Datasets' specs are checked when they run (their columns live in the database).
        let checked = if crate::catalog::Source::is_dataset(&w.spec.source) {
            Ok(())
        } else {
            compile(&w.spec, &Scope::All).map(|_| ())
        };
        checked.map_err(|e| match e {
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
    portal: bool,
    audience_teams: Vec<Uuid>,
    audience_roles: Vec<Uuid>,
    created_by: Option<Uuid>,
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
            portal: r.portal,
            audience_teams: r.audience_teams,
            audience_roles: r.audience_roles,
            created_by: r.created_by,
            version: r.version,
            updated_at: Some(r.updated_at),
            module: None,
        }
    }
}

const COLUMNS: &str = "id, org_id, owner_user_id, name, description, range, widgets, portal, audience_teams, \
     audience_roles, created_by, version, updated_at";
const SELECT: &str = "SELECT id, org_id, owner_user_id, name, description, range, widgets, portal, audience_teams, \
     audience_roles, created_by, version, updated_at FROM dashboards";

/// Who is looking: to show them shared dashboards meant for them.
#[derive(Debug, Clone, Default)]
pub struct Viewer {
    pub user: Uuid,
    /// Holds `dashboard:manage`: sees every shared dashboard in scope.
    pub manager: bool,
    pub teams: Vec<Uuid>,
    pub roles: Vec<Uuid>,
}

impl Viewer {
    /// `user`'s teams and roles (read across organizations).
    pub async fn load(conn: &mut PgConnection, user: Uuid, manager: bool) -> AnalyticsResult<Self> {
        let teams = sqlx::query_scalar("SELECT team_id FROM team_members WHERE user_id = $1")
            .bind(user)
            .fetch_all(&mut *conn)
            .await?;
        let roles =
            sqlx::query_scalar("SELECT DISTINCT role_id FROM role_assignments WHERE user_id = $1")
                .bind(user)
                .fetch_all(&mut *conn)
                .await?;
        Ok(Self {
            user,
            manager,
            teams,
            roles,
        })
    }
}

/// Your own, and shared ones meant for you (`$1` user, `$2` manager, `$3`
/// teams, `$4` roles).
const VISIBLE: &str = "(owner_user_id = $1 OR (owner_user_id IS NULL AND ($2 OR created_by = $1 \
     OR (cardinality(audience_teams) = 0 AND cardinality(audience_roles) = 0) \
     OR audience_teams && $3 OR audience_roles && $4)))";

/// Shared dashboards in the transaction's scope meant for `viewer`, and
/// their own, by name.
pub async fn list(conn: &mut PgConnection, viewer: &Viewer) -> AnalyticsResult<Vec<Dashboard>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE {VISIBLE} ORDER BY lower(name), id"
    )))
    .bind(viewer.user)
    .bind(viewer.manager)
    .bind(&viewer.teams)
    .bind(&viewer.roles)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(Dashboard::from).collect())
}

/// One stored dashboard, if `viewer` may see it.
pub async fn get(
    conn: &mut PgConnection,
    viewer: &Viewer,
    id: Uuid,
) -> AnalyticsResult<Option<Dashboard>> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE id = $5 AND {VISIBLE}"
    )))
    .bind(viewer.user)
    .bind(viewer.manager)
    .bind(&viewer.teams)
    .bind(&viewer.roles)
    .bind(id)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(Dashboard::from))
}

/// One stored dashboard, whoever it's for.
pub(crate) async fn get_any(
    conn: &mut PgConnection,
    id: Uuid,
) -> AnalyticsResult<Option<Dashboard>> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
        .bind(id)
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
    if input.portal && owner.is_some() {
        return Err(bad(
            "portal",
            "only a shared dashboard can be shown in the client portal",
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dashboards (id, org_id, owner_user_id, name, description, range, widgets, created_by, portal) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(org_id)
    .bind(owner)
    .bind(input.name.trim())
    .bind(input.description.as_deref().map(str::trim).filter(|d| !d.is_empty()))
    .bind(&range)
    .bind(sqlx::types::Json(&input.widgets))
    .bind(ctx.actor.id())
    .bind(input.portal)
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
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE dashboards SET name = $2, description = $3, range = $4, widgets = $5, portal = $7, \
                version = version + 1, updated_at = now() WHERE id = $1 AND ($6::integer IS NULL OR version = $6) \
         RETURNING {COLUMNS}",
    )))
    .bind(id)
    .bind(input.name.trim())
    .bind(input.description.as_deref().map(str::trim).filter(|d| !d.is_empty()))
    .bind(&range)
    .bind(sqlx::types::Json(&input.widgets))
    .bind(input.version)
    .bind(input.portal)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("dashboards_portal_shared") => {
            bad("portal", "only a shared dashboard can be shown in the client portal")
        }
        _ => AnalyticsError::Db(e),
    })?;
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

/// Makes a personal dashboard shared at its organization.
pub async fn share(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> AnalyticsResult<()> {
    let org: Option<Uuid> = sqlx::query_scalar(
        "UPDATE dashboards SET owner_user_id = NULL, updated_at = now() WHERE id = $1 AND owner_user_id IS NOT NULL \
         RETURNING org_id",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(org) = org {
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("dashboard.shared")
                .org(org)
                .target("dashboard", id),
        )
        .await
        .map_err(|e| AnalyticsError::Db(sqlx::Error::Protocol(e.to_string())))?;
    }
    Ok(())
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
        portal: false,
        audience_teams: Vec::new(),
        audience_roles: Vec::new(),
        created_by: None,
        version: 1,
        updated_at: None,
        module: None,
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
            "csat",
            "Customer satisfaction",
            "How requesters rate resolved tickets, and who's rated well.",
            vec![
                widget(
                    "score",
                    "Satisfaction %",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "csat",
                "measures": [{ "fn": "avg", "field": "satisfaction" }] }),
                ),
                widget(
                    "answers",
                    "Answers",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "csat", "measures": count,
                "filters": [{ "field": "answered", "op": "eq", "value": true }] }),
                ),
                widget(
                    "sent",
                    "Surveys sent",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "csat", "measures": count }),
                ),
                widget(
                    "by-week",
                    "Satisfaction % by week",
                    Viz::Line,
                    12,
                    false,
                    json!({ "source": "csat",
                "measures": [{ "fn": "avg", "field": "satisfaction" }],
                "dimensions": [{ "field": "responded_at", "granularity": "week" }],
                "filters": [{ "field": "answered", "op": "eq", "value": true }], "limit": 60 }),
                ),
                widget(
                    "by-tech",
                    "Satisfaction % by technician",
                    Viz::Bar,
                    6,
                    false,
                    json!({ "source": "csat",
                "measures": [{ "fn": "avg", "field": "satisfaction" }, { "fn": "count" }],
                "dimensions": [{ "field": "technician" }],
                "filters": [{ "field": "answered", "op": "eq", "value": true }], "limit": 20 }),
                ),
                widget(
                    "by-client",
                    "Satisfaction % by client",
                    Viz::Table,
                    6,
                    false,
                    json!({ "source": "csat",
                "measures": [{ "fn": "avg", "field": "satisfaction" }, { "fn": "count" }],
                "dimensions": [{ "field": "client" }],
                "filters": [{ "field": "answered", "op": "eq", "value": true }], "limit": 50 }),
                ),
            ],
        ),
        prebuilt(
            "nps",
            "Net promoter score",
            "How likely your clients' people are to recommend you, and who's unhappy.",
            vec![
                widget(
                    "score",
                    "NPS",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "nps", "measures": [{ "fn": "avg", "field": "nps" }] }),
                ),
                widget(
                    "answers",
                    "Answers",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "nps", "measures": count,
                "filters": [{ "field": "answered", "op": "eq", "value": true }] }),
                ),
                widget(
                    "asked",
                    "Asked",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "nps", "measures": count }),
                ),
                widget(
                    "mix",
                    "Promoters, passives and detractors",
                    Viz::Pie,
                    6,
                    false,
                    json!({ "source": "nps", "measures": count,
                "dimensions": [{ "field": "category" }], "filters": [{ "field": "answered", "op": "eq", "value": true }] }),
                ),
                widget(
                    "by-client",
                    "NPS by client",
                    Viz::Table,
                    6,
                    false,
                    json!({ "source": "nps",
                "measures": [{ "fn": "avg", "field": "nps" }, { "fn": "count" }], "dimensions": [{ "field": "client" }],
                "filters": [{ "field": "answered", "op": "eq", "value": true }], "limit": 50 }),
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
        sales(),
        finance(),
        expenses(),
    ]
}

/// The prebuilt expenses dashboard (ADR-084): spent in the range, by
/// person, client, project and category, and what's to be billed and paid
/// back. Drafts are left out.
fn expenses() -> Dashboard {
    let filed = json!([{ "field": "status", "op": "neq", "value": "draft" }]);
    let amount = json!([{ "fn": "sum", "field": "amount" }]);
    Dashboard {
        module: Some("expenses"),
        ..prebuilt(
            "expenses",
            "Expenses",
            "Expenses submitted in the range: by person, client, project and category, and what's billable and paid back.",
            vec![
                widget(
                    "spent",
                    "Spent",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "expenses", "measures": amount, "filters": filed }),
                ),
                widget(
                    "billable",
                    "To bill (marked up)",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "expenses", "measures": [{ "fn": "sum", "field": "billed_amount" }],
                        "filters": filed }),
                ),
                widget(
                    "pay-back",
                    "To pay back",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "expenses", "measures": amount,
                        "filters": [{ "field": "reimbursable", "op": "eq", "value": true },
                                    { "field": "status", "op": "in", "value": ["submitted", "approved"] }] }),
                ),
                widget(
                    "by-person",
                    "By person",
                    Viz::Bar,
                    6,
                    false,
                    json!({ "source": "expenses", "measures": amount, "dimensions": [{ "field": "person" }],
                        "filters": filed, "limit": 20 }),
                ),
                widget(
                    "by-category",
                    "By category",
                    Viz::Bar,
                    6,
                    false,
                    json!({ "source": "expenses", "measures": amount, "dimensions": [{ "field": "category" }],
                        "filters": filed }),
                ),
                widget(
                    "by-client",
                    "By client",
                    Viz::Table,
                    6,
                    false,
                    json!({ "source": "expenses", "measures": amount, "dimensions": [{ "field": "client" }],
                        "filters": filed, "sort": [{ "by": "sum_amount", "dir": "desc" }], "limit": 25 }),
                ),
                widget(
                    "by-project",
                    "By project",
                    Viz::Table,
                    6,
                    false,
                    json!({ "source": "expenses", "measures": amount, "dimensions": [{ "field": "project" }],
                        "filters": filed, "sort": [{ "by": "sum_amount", "dir": "desc" }], "limit": 25 }),
                ),
            ],
        )
    }
}

/// The prebuilt finance dashboard (ADR-078): billed in the range, what's
/// owed and overdue now, aging, billed by kind and client, and billable
/// time billed or not.
fn finance() -> Dashboard {
    let sent = json!([{ "field": "sent", "op": "eq", "value": true }]);
    let total = json!([{ "fn": "sum", "field": "total" }]);
    let owed = json!([{ "fn": "sum", "field": "owed" }]);
    let amount = json!([{ "fn": "sum", "field": "amount" }]);
    Dashboard {
        module: Some("billing"),
        ..prebuilt(
            "finance",
            "Finance",
            "What was billed in the range, by kind and client; what's owed and overdue now, by age; and billable time, billed or not.",
            vec![
                widget(
                    "billed",
                    "Billed",
                    Viz::Number,
                    4,
                    false,
                    json!({ "source": "invoices", "measures": total, "filters": sent }),
                ),
                widget(
                    "owed",
                    "Still owed",
                    Viz::Number,
                    4,
                    true,
                    json!({ "source": "invoices", "measures": owed }),
                ),
                widget(
                    "overdue",
                    "Overdue",
                    Viz::Number,
                    4,
                    true,
                    json!({ "source": "invoices", "measures": owed,
                        "filters": [{ "field": "overdue", "op": "eq", "value": true }] }),
                ),
                widget(
                    "per-month",
                    "Billed per month",
                    Viz::Line,
                    12,
                    false,
                    json!({ "source": "invoices", "measures": total, "filters": sent,
                        "dimensions": [{ "field": "issued_on", "granularity": "month" }], "limit": 24 }),
                ),
                widget(
                    "aging",
                    "Owed by age",
                    Viz::Bar,
                    6,
                    true,
                    json!({ "source": "invoices", "measures": owed, "dimensions": [{ "field": "aging" }],
                        "filters": [{ "field": "aging", "op": "neq", "value": "Settled" }] }),
                ),
                widget(
                    "by-kind",
                    "Billed by kind",
                    Viz::Bar,
                    6,
                    false,
                    json!({ "source": "invoice_lines", "measures": amount, "dimensions": [{ "field": "kind" }],
                        "filters": sent }),
                ),
                widget(
                    "by-client",
                    "Billed by client",
                    Viz::Table,
                    6,
                    false,
                    json!({ "source": "invoices", "measures": total, "dimensions": [{ "field": "client" }],
                        "filters": sent, "sort": [{ "by": "sum_total", "dir": "desc" }], "limit": 25 }),
                ),
                widget(
                    "time-billed",
                    "Billable hours, billed or not",
                    Viz::Bar,
                    6,
                    false,
                    json!({ "source": "time_entries", "measures": [{ "fn": "sum", "field": "hours" }],
                        "dimensions": [{ "field": "billed" }],
                        "filters": [{ "field": "billable", "op": "eq", "value": true }] }),
                ),
            ],
        )
    }
}

/// The prebuilt sales dashboard: the open pipeline as it stands, and what
/// was won (and lost) in the range.
fn sales() -> Dashboard {
    Dashboard {
        module: Some("sales"),
        ..sales_widgets()
    }
}

fn sales_widgets() -> Dashboard {
    let open = json!([{ "field": "open", "op": "eq", "value": true }]);
    let won = json!([{ "field": "won", "op": "eq", "value": true }]);
    let value = json!([{ "fn": "sum", "field": "first_year" }]);
    let weighted = json!([{ "fn": "sum", "field": "weighted" }]);
    let count = json!([{ "fn": "count" }]);
    prebuilt(
        "sales",
        "Sales",
        "The open pipeline, weighted and by stage, and what was won in the range (first-year values).",
        vec![
            widget(
                "pipeline",
                "Open pipeline",
                Viz::Number,
                3,
                true,
                json!({ "source": "opportunities", "measures": value, "filters": open }),
            ),
            widget(
                "weighted",
                "Weighted pipeline",
                Viz::Number,
                3,
                true,
                json!({ "source": "opportunities", "measures": weighted, "filters": open }),
            ),
            widget(
                "won",
                "Won",
                Viz::Number,
                3,
                false,
                json!({ "source": "opportunities", "measures": value, "filters": won }),
            ),
            widget(
                "won-count",
                "Deals won",
                Viz::Number,
                3,
                false,
                json!({ "source": "opportunities", "measures": count, "filters": won }),
            ),
            widget(
                "by-stage",
                "Open pipeline by stage",
                Viz::Bar,
                6,
                true,
                json!({ "source": "opportunities", "measures": value, "dimensions": [{ "field": "stage" }],
                    "filters": open, "limit": 12 }),
            ),
            widget(
                "by-close",
                "Weighted pipeline by expected close",
                Viz::Bar,
                6,
                true,
                json!({ "source": "opportunities", "measures": weighted,
                    "dimensions": [{ "field": "close_date", "granularity": "month" }], "filters": open, "limit": 12 }),
            ),
            widget(
                "won-month",
                "Won per month",
                Viz::Line,
                12,
                false,
                json!({ "source": "opportunities", "measures": value,
                    "dimensions": [{ "field": "closed_at", "granularity": "month" }], "filters": won, "limit": 24 }),
            ),
            widget(
                "by-owner",
                "Open pipeline by owner",
                Viz::Table,
                6,
                true,
                json!({ "source": "opportunities", "measures": [{ "fn": "sum", "field": "first_year" },
                    { "fn": "sum", "field": "weighted" }], "dimensions": [{ "field": "owner" }],
                    "filters": open, "limit": 25 }),
            ),
            widget(
                "why-lost",
                "Why deals were lost",
                Viz::Bar,
                6,
                false,
                json!({ "source": "opportunities", "measures": count, "dimensions": [{ "field": "reason" }],
                    "filters": [{ "field": "status", "op": "eq", "value": "lost" }], "limit": 12 }),
            ),
            widget(
                "activity",
                "Activity by kind",
                Viz::Bar,
                12,
                false,
                json!({ "source": "sales_activities", "measures": count, "dimensions": [{ "field": "kind" }] }),
            ),
        ],
    )
}

/// Sources a dashboard can show in the client portal.
pub const PORTAL_SOURCES: &[&str] = &["tickets", "time_entries", "assets"];

/// Checks a dashboard can be shown in the client portal: every widget's
/// source is one the portal shows.
pub fn check_portal(widgets: &[Widget]) -> AnalyticsResult<()> {
    for (i, w) in widgets.iter().enumerate() {
        if !PORTAL_SOURCES.contains(&w.spec.source.as_str()) {
            return Err(bad(
                format!("widgets/{i}/spec/source"),
                format!("the client portal shows only {}", PORTAL_SOURCES.join(", ")),
            ));
        }
    }
    Ok(())
}

/// The dashboards shown in a company's portal: published at it or above.
pub async fn for_portal(conn: &mut PgConnection, org_id: Uuid) -> AnalyticsResult<Vec<Dashboard>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE portal AND owner_user_id IS NULL \
           AND (SELECT path FROM organizations WHERE id = $1) <@ (SELECT path FROM organizations o WHERE o.id = dashboards.org_id) \
         ORDER BY lower(name), id"
    )))
    .bind(org_id)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(Dashboard::from).collect())
}

/// The spec a widget runs for `range`: the range on its source's time
/// field, unless it fixes its own or already filters that field.
pub fn with_range(widget: &Widget, range: &str) -> QuerySpec {
    let mut spec = widget.spec.clone();
    if let Some(source) = crate::catalog::source(&spec.source)
        && !widget.fixed_range
        && !spec.filters.iter().any(|f| f.field == source.time_field)
    {
        spec.filters.push(crate::query::Filter {
            field: source.time_field.to_string(),
            op: crate::query::Op::Within,
            value: json!(range),
        });
    }
    spec
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
                portal: false,
                shared: None,
                version: None,
            };
            check(&input).unwrap_or_else(|e| panic!("{}: {e}", d.id));
        }
    }
}
