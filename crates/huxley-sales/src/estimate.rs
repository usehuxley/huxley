//! Presales estimates (ADR-073): a scope's Work Breakdown Structure and
//! Bill of Materials, and the quote they assemble.
//!
//! - **WBS**: phases and tasks, each with a role (a labour service from the
//!   catalog, priced per hour) and low / likely / high hours. A task's
//!   expected hours are the three-point estimate (low + 4 × likely + high)
//!   ÷ 6; the scope's risk adds contingency (10, 20 or 35%).
//! - **BOM**: catalog lines sized from the client's environment, each with
//!   how its quantity was reached; alternates become quote options.
//! - The Presales scoping pack drafts both ([`context`], [`write`]) from the
//!   Scope of Work, project templates and the hours logged on past
//!   projects' tasks. Rows people edited are kept.
//! - [`assemble`] builds a draft quote: labour lines per role, then the
//!   materials, carrying the latest approved Scope of Work, with checks.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{SalesError, SalesResult, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = ScopeRisk)]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    /// The contingency it adds, in percent.
    pub fn contingency(self) -> Decimal {
        match self {
            Self::Low => Decimal::from(10),
            Self::Medium => Decimal::from(20),
            Self::High => Decimal::from(35),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeTask)]
pub struct Task {
    pub id: Uuid,
    pub phase: String,
    pub name: String,
    pub role_product_id: Option<Uuid>,
    pub role: Option<String>,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub hours_low: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub hours_likely: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub hours_high: Decimal,
    /// (low + 4 × likely + high) ÷ 6.
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub expected: Decimal,
    pub milestone: bool,
    pub deliverable: Option<String>,
    pub drafted: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeItem)]
pub struct Item {
    pub id: Uuid,
    pub product_id: Option<Uuid>,
    pub sku: Option<String>,
    pub name: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub basis: Option<String>,
    pub alternate: bool,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub unit_price: Option<Decimal>,
    pub drafted: bool,
}

/// Hours and cost per role, with contingency.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeRoleTotal)]
pub struct RoleTotal {
    pub role_product_id: Option<Uuid>,
    pub role: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub hours: Decimal,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub rate: Option<Decimal>,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub cost: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeEstimate)]
pub struct Estimate {
    pub risk: Risk,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub contingency_pct: Decimal,
    pub state: crate::scoping::DraftState,
    pub error: Option<String>,
    pub tasks: Vec<Task>,
    pub items: Vec<Item>,
    pub roles: Vec<RoleTotal>,
    /// Expected hours with contingency.
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub hours: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub labour: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub materials: Decimal,
    /// Things to settle before quoting.
    pub warnings: Vec<String>,
}

/// The estimate as it stands. Run in system scope (rates are the catalog's).
pub async fn get(conn: &mut PgConnection, scope: Uuid) -> SalesResult<Estimate> {
    let (risk, state, error): (Risk, crate::scoping::DraftState, Option<String>) = sqlx::query_as(
        "SELECT risk, estimate_state, estimate_error FROM sales_scopes WHERE id = $1",
    )
    .bind(scope)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(SalesError::NotFound)?;
    let tasks: Vec<Task> = sqlx::query_as(
        "SELECT t.id, t.phase, t.name, t.role_product_id, coalesce(t.role, p.name) AS role, t.hours_low, \
                t.hours_likely, t.hours_high, round((t.hours_low + 4 * t.hours_likely + t.hours_high) / 6, 2) AS expected, \
                t.milestone, t.deliverable, t.drafted \
           FROM sales_scope_tasks t LEFT JOIN catalog_products p ON p.id = t.role_product_id \
          WHERE t.scope_id = $1 ORDER BY t.position",
    )
    .bind(scope)
    .fetch_all(&mut *conn)
    .await?;
    let items: Vec<Item> = sqlx::query_as(
        "SELECT i.id, i.product_id, p.sku, i.name, i.quantity, i.basis, i.alternate, p.price AS unit_price, i.drafted \
           FROM sales_scope_items i LEFT JOIN catalog_products p ON p.id = i.product_id \
          WHERE i.scope_id = $1 ORDER BY i.position",
    )
    .bind(scope)
    .fetch_all(&mut *conn)
    .await?;
    let factor = Decimal::ONE + risk.contingency() / Decimal::ONE_HUNDRED;
    let mut roles: Vec<RoleTotal> = Vec::new();
    for t in &tasks {
        let label = t.role.clone().unwrap_or_else(|| "Unassigned".into());
        let hours = (t.expected * factor).round_dp(2);
        match roles
            .iter_mut()
            .find(|r| r.role_product_id == t.role_product_id && r.role == label)
        {
            Some(r) => r.hours += hours,
            None => roles.push(RoleTotal {
                role_product_id: t.role_product_id,
                role: label,
                hours,
                rate: None,
                cost: None,
            }),
        }
    }
    for r in &mut roles {
        if let Some(p) = r.role_product_id {
            r.rate = sqlx::query_scalar("SELECT price FROM catalog_products WHERE id = $1")
                .bind(p)
                .fetch_optional(&mut *conn)
                .await?
                .flatten();
        }
        r.cost = r.rate.map(|rate| (rate * r.hours).round_dp(2));
    }
    let labour = roles.iter().filter_map(|r| r.cost).sum();
    let materials = items
        .iter()
        .filter(|i| !i.alternate)
        .filter_map(|i| i.unit_price.map(|p| p * i.quantity))
        .sum::<Decimal>()
        .round_dp(2);
    let warnings = checks(&mut *conn, scope, &tasks, &items, &roles).await?;
    Ok(Estimate {
        risk,
        contingency_pct: risk.contingency(),
        state,
        error,
        hours: roles.iter().map(|r| r.hours).sum(),
        tasks,
        items,
        roles,
        labour,
        materials,
        warnings,
    })
}

/// Consistency checks before quoting.
async fn checks(
    conn: &mut PgConnection,
    scope: Uuid,
    tasks: &[Task],
    items: &[Item],
    roles: &[RoleTotal],
) -> SalesResult<Vec<String>> {
    let mut out = Vec::new();
    let approved: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sales_scope_versions WHERE scope_id = $1)",
    )
    .bind(scope)
    .fetch_one(&mut *conn)
    .await?;
    if !approved {
        out.push("The Scope of Work isn't approved yet.".to_owned());
    }
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sales_scope_questions WHERE scope_id = $1 AND resolved_at IS NULL",
    )
    .bind(scope)
    .fetch_one(&mut *conn)
    .await?;
    if open > 0 {
        out.push(format!("{open} open question(s) still to answer."));
    }
    let unassigned = tasks.iter().filter(|t| t.role_product_id.is_none()).count();
    if unassigned > 0 {
        out.push(format!(
            "{unassigned} task(s) have no priced role, so their hours won't be quoted."
        ));
    }
    for r in roles
        .iter()
        .filter(|r| r.role_product_id.is_some() && r.rate.is_none())
    {
        out.push(format!("{} has no hourly price in the catalog.", r.role));
    }
    let loose = items.iter().filter(|i| i.product_id.is_none()).count();
    if loose > 0 {
        out.push(format!(
            "{loose} materials line(s) aren't matched to the catalog."
        ));
    }
    // Every deliverable in the Scope of Work has a task serving it.
    let deliverables: Option<String> = sqlx::query_scalar(
        "SELECT body FROM sales_scope_sections WHERE scope_id = $1 AND key = 'deliverables'",
    )
    .bind(scope)
    .fetch_optional(&mut *conn)
    .await?;
    for line in deliverables.unwrap_or_default().lines() {
        let d = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if d.len() < 3 {
            continue;
        }
        let served = tasks.iter().any(|t| {
            t.deliverable.as_deref().is_some_and(|x| {
                let (x, d) = (x.to_lowercase(), d.to_lowercase());
                x.contains(&d) || d.contains(&x)
            })
        });
        if !served {
            out.push(format!("No task delivers \"{d}\"."));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ScopeTaskInput)]
pub struct TaskInput {
    pub phase: String,
    pub name: String,
    #[serde(default)]
    pub role_product_id: Option<Uuid>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default, deserialize_with = "crate::money")]
    #[schema(value_type = String)]
    pub hours_low: Decimal,
    #[serde(default, deserialize_with = "crate::money")]
    #[schema(value_type = String)]
    pub hours_likely: Decimal,
    #[serde(default, deserialize_with = "crate::money")]
    #[schema(value_type = String)]
    pub hours_high: Decimal,
    #[serde(default)]
    pub milestone: bool,
    #[serde(default)]
    pub deliverable: Option<String>,
    /// Kept as the AI's (when saving rows you didn't change).
    #[serde(default)]
    pub drafted: bool,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ScopeItemInput)]
pub struct ItemInput {
    #[serde(default)]
    pub product_id: Option<Uuid>,
    pub name: String,
    #[serde(deserialize_with = "crate::money")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    #[serde(default)]
    pub basis: Option<String>,
    #[serde(default)]
    pub alternate: bool,
    #[serde(default)]
    pub drafted: bool,
}

async fn org(conn: &mut PgConnection, scope: Uuid) -> SalesResult<Uuid> {
    sqlx::query_scalar("SELECT org_id FROM sales_scopes WHERE id = $1")
        .bind(scope)
        .fetch_optional(conn)
        .await?
        .ok_or(SalesError::NotFound)
}

/// Replaces the WBS.
pub async fn set_tasks(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    scope: Uuid,
    tasks: &[TaskInput],
) -> SalesResult<Estimate> {
    let org = org(&mut *conn, scope).await?;
    if tasks.len() > 300 {
        return Err(SalesError::invalid("tasks", "at most 300 tasks"));
    }
    sqlx::query("DELETE FROM sales_scope_tasks WHERE scope_id = $1")
        .bind(scope)
        .execute(&mut *conn)
        .await?;
    for (i, t) in tasks.iter().enumerate() {
        let f = |x: &str| format!("tasks/{i}/{x}");
        let phase = crate::required(&f("phase"), &t.phase, 100)?;
        let name = crate::required(&f("name"), &t.name, 300)?;
        let (lo, li, hi) = (
            t.hours_low,
            t.hours_likely.max(t.hours_low),
            t.hours_high.max(t.hours_likely.max(t.hours_low)),
        );
        if lo < Decimal::ZERO {
            return Err(SalesError::invalid(&f("hoursLow"), "can't be negative"));
        }
        sqlx::query(
            "INSERT INTO sales_scope_tasks (id, scope_id, org_id, position, phase, name, role_product_id, role, \
                hours_low, hours_likely, hours_high, milestone, deliverable, drafted) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(huxley_core::new_id())
        .bind(scope)
        .bind(org)
        .bind(i32::try_from(i).unwrap_or(0))
        .bind(&phase)
        .bind(&name)
        .bind(t.role_product_id)
        .bind(text(&f("role"), t.role.as_deref(), 100)?)
        .bind(lo)
        .bind(li)
        .bind(hi)
        .bind(t.milestone)
        .bind(text(&f("deliverable"), t.deliverable.as_deref(), 300)?)
        .bind(t.drafted)
        .execute(&mut *conn)
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("scope.wbs_saved")
            .org(org)
            .target("scope", scope)
            .details(json!({ "tasks": tasks.len() })),
    )
    .await?;
    get(conn, scope).await
}

/// Replaces the BOM.
pub async fn set_items(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    scope: Uuid,
    items: &[ItemInput],
) -> SalesResult<Estimate> {
    let org = org(&mut *conn, scope).await?;
    if items.len() > 300 {
        return Err(SalesError::invalid("items", "at most 300 lines"));
    }
    sqlx::query("DELETE FROM sales_scope_items WHERE scope_id = $1")
        .bind(scope)
        .execute(&mut *conn)
        .await?;
    for (i, x) in items.iter().enumerate() {
        let f = |k: &str| format!("items/{i}/{k}");
        let name = crate::required(&f("name"), &x.name, 300)?;
        if x.quantity <= Decimal::ZERO {
            return Err(SalesError::invalid(
                &f("quantity"),
                "must be more than zero",
            ));
        }
        sqlx::query(
            "INSERT INTO sales_scope_items (id, scope_id, org_id, position, product_id, name, quantity, basis, alternate, drafted) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(huxley_core::new_id())
        .bind(scope)
        .bind(org)
        .bind(i32::try_from(i).unwrap_or(0))
        .bind(x.product_id)
        .bind(&name)
        .bind(x.quantity)
        .bind(text(&f("basis"), x.basis.as_deref(), 300)?)
        .bind(x.alternate)
        .bind(x.drafted)
        .execute(&mut *conn)
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("scope.bom_saved")
            .org(org)
            .target("scope", scope)
            .details(json!({ "items": items.len() })),
    )
    .await?;
    get(conn, scope).await
}

pub async fn set_risk(conn: &mut PgConnection, scope: Uuid, risk: Risk) -> SalesResult<Estimate> {
    sqlx::query("UPDATE sales_scopes SET risk = $2 WHERE id = $1")
        .bind(scope)
        .bind(risk)
        .execute(&mut *conn)
        .await?;
    get(conn, scope).await
}

// ---- drafting ----

/// Asks the pack for an estimate: raises `scope.estimate_requested`.
pub async fn request(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    scope: Uuid,
) -> SalesResult<Estimate> {
    let org = org(&mut *conn, scope).await?;
    let written: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sales_scope_sections WHERE scope_id = $1 AND btrim(body) <> '')",
    )
    .bind(scope)
    .fetch_one(&mut *conn)
    .await?;
    if !written {
        return Err(SalesError::invalid(
            "sections",
            "write the Scope of Work first: the estimate follows it",
        ));
    }
    sqlx::query(
        "UPDATE sales_scopes SET estimate_state = 'requested', estimate_error = NULL, estimate_requested_at = now() \
          WHERE id = $1",
    )
    .bind(scope)
    .execute(&mut *conn)
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "scope.estimate_requested",
            org_id: Some(org),
            subject_type: "scope",
            subject_id: scope,
            payload: json!({ "scopeId": scope, "requestedBy": ctx.actor.id() }),
        },
    )
    .await?;
    get(conn, scope).await
}

pub async fn cancel(conn: &mut PgConnection, scope: Uuid) -> SalesResult<Estimate> {
    sqlx::query("UPDATE sales_scopes SET estimate_state = 'idle' WHERE id = $1")
        .bind(scope)
        .execute(&mut *conn)
        .await?;
    get(conn, scope).await
}

/// What drafting an estimate reads.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeEstimateContext)]
pub struct Context {
    /// The Scope of Work, the client's environment, project templates and
    /// past actuals, as text for the AI.
    pub text: String,
    /// Labour roles to pick from, by number: `[n] name (rate an hour)`.
    pub roles: Vec<String>,
    /// Catalog products to pick from, by number: `[n] SKU name (category, price)`.
    pub products: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct Choice {
    id: Uuid,
    sku: String,
    name: String,
    category: Option<String>,
    price: Option<Decimal>,
}

/// Labour roles (hourly services) and products the client's providers sell.
async fn choices(conn: &mut PgConnection, scope: Uuid) -> SalesResult<(Vec<Choice>, Vec<Choice>)> {
    let rows: Vec<(String, Choice)> = sqlx::query_as::<
        _,
        (
            String,
            Uuid,
            String,
            String,
            Option<String>,
            Option<Decimal>,
        ),
    >(
        "SELECT p.kind, p.id, p.sku, p.name, p.category, p.price FROM catalog_products p \
           JOIN organizations po ON po.id = p.org_id \
           JOIN sales_scopes s ON s.id = $1 JOIN organizations c ON c.id = s.org_id \
          WHERE p.active AND po.path @> c.path AND p.kind IN ('product', 'service') \
          ORDER BY p.kind, p.category NULLS LAST, p.name LIMIT 400",
    )
    .bind(scope)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(kind, id, sku, name, category, price)| {
        (
            kind,
            Choice {
                id,
                sku,
                name,
                category,
                price,
            },
        )
    })
    .collect();
    let units: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, unit FROM catalog_products WHERE id = ANY($1)")
            .bind(rows.iter().map(|(_, c)| c.id).collect::<Vec<_>>())
            .fetch_all(&mut *conn)
            .await?;
    let hourly = |id: Uuid| {
        units
            .iter()
            .any(|(i, u)| *i == id && u.to_lowercase().starts_with("hour"))
    };
    let (mut roles, mut products) = (Vec::new(), Vec::new());
    for (kind, c) in rows {
        if kind == "service" && hourly(c.id) {
            roles.push(c);
        } else if kind == "product" {
            products.push(c);
        }
    }
    products.truncate(200);
    Ok((roles, products))
}

/// What estimates are grounded in: the providers' project templates and
/// the hours logged on past projects' tasks.
async fn grounding(conn: &mut PgConnection, org: Uuid) -> SalesResult<Vec<String>> {
    let mut parts = Vec::new();
    let templates: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT t.name, t.structure FROM prj_templates t JOIN organizations tor ON tor.id = t.org_id \
           JOIN organizations c ON c.id = $1 WHERE tor.path @> c.path ORDER BY t.updated_at DESC LIMIT 5",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await?;
    for (name, structure) in templates {
        let phases: Vec<String> = structure["phases"]
            .as_array()
            .map(|ps| {
                ps.iter()
                    .map(|p| {
                        let tasks: Vec<String> = p["tasks"]
                            .as_array()
                            .map(|ts| {
                                ts.iter()
                                    .map(|t| {
                                        format!(
                                            "{} ({}h)",
                                            t["title"].as_str().unwrap_or(""),
                                            t["estimateHours"]
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        format!(
                            "- {}: {}",
                            p["name"].as_str().unwrap_or(""),
                            tasks.join("; ")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        parts.push(format!(
            "A project template, \"{name}\":\n{}",
            phases.join("\n")
        ));
    }
    let actuals: Vec<(String, Option<Decimal>, Option<Decimal>, i64)> = sqlx::query_as(
        "SELECT t.title, round(avg(pt.estimate_hours), 1), round(avg(coalesce(te.minutes, 0)) / 60.0, 1), count(*) \
           FROM prj_tasks pt JOIN sd_tickets t ON t.id = pt.ticket_id \
           JOIN prj_projects p ON p.id = pt.project_id \
           LEFT JOIN (SELECT ticket_id, sum(minutes) AS minutes FROM sd_time_entries GROUP BY ticket_id) te \
                  ON te.ticket_id = pt.ticket_id \
          WHERE p.status IN ('completed', 'closed') \
          GROUP BY t.title ORDER BY count(*) DESC LIMIT 40",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap_or_default();
    if !actuals.is_empty() {
        let lines: Vec<String> = actuals
            .iter()
            .map(|(title, est, act, n)| {
                format!(
                    "- {title}: estimated {}h, took {}h on average ({n} project(s))",
                    est.map_or_else(|| "?".into(), |e| e.normalize().to_string()),
                    act.map_or_else(|| "?".into(), |a| a.normalize().to_string())
                )
            })
            .collect();
        parts.push(format!(
            "Hours logged on past projects' tasks (ground estimates in these):\n{}",
            lines.join("\n")
        ));
    }
    Ok(parts)
}

/// Gathers what drafting the estimate reads. Run in system scope.
pub async fn context(conn: &mut PgConnection, scope: Uuid) -> SalesResult<Context> {
    let s = crate::scoping::get(&mut *conn, scope).await?;
    let mut parts = vec![format!(
        "Engagement: {} for {}.",
        s.opportunity_name, s.template
    )];
    let sow: Vec<String> = s
        .sections
        .iter()
        .filter(|x| !x.body.trim().is_empty())
        .map(|x| format!("## {}\n{}", x.title, x.body))
        .collect();
    parts.push(format!("The Scope of Work:\n{}", sow.join("\n")));
    let environment = crate::scoping::environment_text(&mut *conn, s.org_id).await?;
    if !environment.is_empty() {
        parts.push(format!(
            "The client's environment (for sizing materials):\n{environment}"
        ));
    }
    parts.extend(grounding(&mut *conn, s.org_id).await?);
    let (roles, products) = choices(&mut *conn, scope).await?;
    Ok(Context {
        text: parts.join("\n\n"),
        roles: roles
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "[{}] {} ({} an hour)",
                    i + 1,
                    r.name,
                    r.price
                        .map_or_else(|| "no price".into(), |p| p.normalize().to_string())
                )
            })
            .collect(),
        products: products
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "[{}] {} {} ({}, {})",
                    i + 1,
                    p.sku,
                    p.name,
                    p.category.as_deref().unwrap_or("uncategorized"),
                    p.price
                        .map_or_else(|| "no price".into(), |x| x.normalize().to_string())
                )
            })
            .collect(),
    })
}

/// What the AI wrote.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeEstimateDraft)]
pub struct Draft {
    #[serde(default)]
    pub tasks: Vec<DraftTask>,
    #[serde(default)]
    pub items: Vec<DraftItem>,
    /// low, medium or high.
    #[serde(default)]
    pub risk: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeEstimateDraftTask)]
pub struct DraftTask {
    pub phase: String,
    pub name: String,
    /// A role's number from the list (0: none fits).
    #[serde(default)]
    pub role: i64,
    pub low: f64,
    pub likely: f64,
    pub high: f64,
    #[serde(default)]
    pub milestone: bool,
    #[serde(default)]
    pub deliverable: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeEstimateDraftItem)]
pub struct DraftItem {
    /// A product's number from the list (0: none fits).
    #[serde(default)]
    pub product: i64,
    pub name: String,
    pub quantity: f64,
    #[serde(default)]
    pub basis: String,
    #[serde(default)]
    pub alternate: bool,
}

fn decimal(x: f64) -> Decimal {
    Decimal::from_f64_retain(x.max(0.0))
        .unwrap_or_default()
        .round_dp(2)
}

/// Keeps a drafted estimate: the AI's rows replace its earlier ones, and
/// rows people edited stay. Run in system scope.
pub async fn write(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    scope: Uuid,
    draft: &Draft,
) -> SalesResult<Estimate> {
    let current = get(&mut *conn, scope).await?;
    let (roles, products) = choices(&mut *conn, scope).await?;
    let pick = |list: &[Choice], n: i64| {
        usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| list.get(i))
            .map(|c| (c.id, c.name.clone()))
    };
    let mut tasks: Vec<TaskInput> = current
        .tasks
        .iter()
        .filter(|t| !t.drafted)
        .map(|t| TaskInput {
            phase: t.phase.clone(),
            name: t.name.clone(),
            role_product_id: t.role_product_id,
            role: t.role.clone(),
            hours_low: t.hours_low,
            hours_likely: t.hours_likely,
            hours_high: t.hours_high,
            milestone: t.milestone,
            deliverable: t.deliverable.clone(),
            drafted: false,
        })
        .collect();
    for t in draft.tasks.iter().take(200) {
        let role = pick(&roles, t.role);
        tasks.push(TaskInput {
            phase: t.phase.chars().take(100).collect(),
            name: t.name.chars().take(300).collect(),
            role_product_id: role.as_ref().map(|r| r.0),
            role: role.map(|r| r.1),
            hours_low: decimal(t.low),
            hours_likely: decimal(t.likely.max(t.low)),
            hours_high: decimal(t.high.max(t.likely).max(t.low)),
            milestone: t.milestone,
            deliverable: Some(t.deliverable.chars().take(300).collect::<String>())
                .filter(|d| !d.trim().is_empty()),
            drafted: true,
        });
    }
    let mut items: Vec<ItemInput> = current
        .items
        .iter()
        .filter(|i| !i.drafted)
        .map(|i| ItemInput {
            product_id: i.product_id,
            name: i.name.clone(),
            quantity: i.quantity,
            basis: i.basis.clone(),
            alternate: i.alternate,
            drafted: false,
        })
        .collect();
    for x in draft.items.iter().take(200).filter(|x| x.quantity > 0.0) {
        let product = pick(&products, x.product);
        items.push(ItemInput {
            product_id: product.as_ref().map(|p| p.0),
            name: product.map_or_else(|| x.name.chars().take(300).collect(), |p| p.1),
            quantity: decimal(x.quantity),
            basis: Some(x.basis.chars().take(300).collect::<String>())
                .filter(|b| !b.trim().is_empty()),
            alternate: x.alternate,
            drafted: true,
        });
    }
    set_tasks(&mut *conn, ctx, scope, &tasks).await?;
    set_items(&mut *conn, ctx, scope, &items).await?;
    if let Some(risk) = draft.risk.as_deref().and_then(|r| match r {
        "low" => Some(Risk::Low),
        "medium" => Some(Risk::Medium),
        "high" => Some(Risk::High),
        _ => None,
    }) {
        sqlx::query("UPDATE sales_scopes SET risk = $2 WHERE id = $1")
            .bind(scope)
            .bind(risk)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query(
        "UPDATE sales_scopes SET estimate_state = 'idle', estimated_at = now() WHERE id = $1",
    )
    .bind(scope)
    .execute(&mut *conn)
    .await?;
    get(conn, scope).await
}

pub async fn failed(conn: &mut PgConnection, scope: Uuid, error: &str) -> SalesResult<()> {
    sqlx::query(
        "UPDATE sales_scopes SET estimate_state = 'failed', estimate_error = $2 WHERE id = $1",
    )
    .bind(scope)
    .bind(error.chars().take(2000).collect::<String>())
    .execute(conn)
    .await?;
    Ok(())
}

// ---- assembling the quote ----

/// The quote made, and what to look at.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeAssembled)]
pub struct Assembled {
    pub quote_id: Uuid,
    pub warnings: Vec<String>,
}

/// Builds a draft quote on the opportunity from the estimate: labour lines
/// per priced role (hours with contingency, rounded up to the half hour),
/// then the materials (alternates as options), carrying the latest approved
/// Scope of Work. Run in system scope (the caller checked access).
pub async fn assemble(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    scope: Uuid,
) -> SalesResult<Assembled> {
    let e = get(&mut *conn, scope).await?;
    let s = crate::scoping::get(&mut *conn, scope).await?;
    if e.tasks.is_empty() && e.items.is_empty() {
        return Err(SalesError::invalid("tasks", "estimate the work first"));
    }
    let mut lines = Vec::new();
    let labour: Vec<&RoleTotal> = e
        .roles
        .iter()
        .filter(|r| r.role_product_id.is_some())
        .collect();
    if !labour.is_empty() {
        lines.push(json!({ "kind": "heading", "name": "Labour" }));
        for r in labour {
            let hours = (r.hours * Decimal::TWO).ceil() / Decimal::TWO;
            lines.push(json!({ "productId": r.role_product_id, "quantity": hours.normalize().to_string() }));
        }
    }
    if !e.items.is_empty() {
        lines.push(json!({ "kind": "heading", "name": "Hardware and software" }));
        for i in &e.items {
            let mut line = json!({ "name": i.name, "quantity": i.quantity.normalize().to_string(),
                                   "optional": i.alternate, "description": i.basis.clone().unwrap_or_default() });
            if let Some(p) = i.product_id {
                line["productId"] = json!(p);
            }
            lines.push(line);
        }
    }
    let input: crate::quotes::QuoteInput = serde_json::from_value(json!({
        "title": s.opportunity_name,
        "opportunityId": s.opportunity_id,
        "scopeVersionId": s.versions.first().map(|v| v.id),
        "lines": lines,
    }))
    .map_err(|err| SalesError::Conflict(err.to_string()))?;
    let q = crate::quotes::create(&mut *conn, ctx, s.org_id, &input).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("scope.quote_assembled")
            .org(s.org_id)
            .target("scope", scope)
            .details(json!({ "quote": q.id, "warnings": e.warnings })),
    )
    .await?;
    Ok(Assembled {
        quote_id: q.id,
        warnings: e.warnings,
    })
}
