//! Opportunities: a possible sale to a client, moving through the pipeline.
//!
//! - It starts at the first open stage unless told otherwise, and every move
//!   is kept in its stage history.
//! - Moving to a won or lost stage closes it (with an optional reason);
//!   moving back to an open stage reopens it.
//! - Its probability is the stage's unless overridden; weighted value is
//!   one-time amount plus a year of recurring, times the probability.
//! - Raises `sales.opportunity.created`, `.updated`, `.stage_changed`,
//!   `.won` and `.lost`.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::stages::{self, StageKind};
use crate::{SalesError, SalesResult, currency, money, plain, required, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = OpportunityStatus)]
pub enum Status {
    Open,
    Won,
    Lost,
}

/// Which forecast an open opportunity counts in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = ForecastCategory)]
pub enum Forecast {
    #[default]
    Pipeline,
    BestCase,
    Commit,
    Omitted,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Opportunity {
    pub id: Uuid,
    /// The client it would sell to.
    pub org_id: Uuid,
    pub org_name: String,
    /// `OPP-` and this number.
    pub number: i64,
    pub name: String,
    pub description: String,
    pub stage_id: Uuid,
    pub stage_name: String,
    pub stage_kind: StageKind,
    pub status: Status,
    /// Expected one-time value.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub amount: Decimal,
    /// Expected monthly recurring value.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub recurring: Decimal,
    pub currency: String,
    /// The chance of winning: the override, else the stage's.
    pub probability: i16,
    /// Set when the stage's probability is overridden.
    pub probability_override: Option<i16>,
    /// (One-time + 12 × recurring) × probability.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub weighted: Decimal,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub close_date: Option<Date>,
    pub forecast: Forecast,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub contact_id: Option<Uuid>,
    pub contact_name: Option<String>,
    pub lead_id: Option<Uuid>,
    pub source: Option<String>,
    pub reason_id: Option<Uuid>,
    pub reason_name: Option<String>,
    pub reason_note: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpportunityInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The stage to start at (new ones only; move it to change it).
    #[serde(default)]
    pub stage_id: Option<Uuid>,
    #[serde(default, deserialize_with = "money")]
    #[schema(value_type = Option<String>)]
    pub amount: Decimal,
    #[serde(default, deserialize_with = "money")]
    #[schema(value_type = Option<String>)]
    pub recurring: Decimal,
    #[serde(default = "usd")]
    pub currency: String,
    /// Overrides the stage's probability.
    #[serde(default)]
    pub probability: Option<i16>,
    #[serde(default, with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub close_date: Option<Date>,
    #[serde(default)]
    pub forecast: Forecast,
    #[serde(default)]
    pub owner_id: Option<Uuid>,
    #[serde(default)]
    pub contact_id: Option<Uuid>,
    #[serde(default)]
    pub source: Option<String>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

fn usd() -> String {
    "USD".into()
}

/// A move to another stage. Closing it may say why.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveInput {
    pub stage_id: Uuid,
    #[serde(default)]
    pub reason_id: Option<Uuid>,
    #[serde(default)]
    pub reason_note: Option<String>,
    pub version: i32,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    pub status: Option<Status>,
    pub stage_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    /// One client's.
    pub org_id: Option<Uuid>,
    /// In the name, the client's name or the number.
    pub q: Option<String>,
    /// Closing on or before (`YYYY-MM-DD`).
    pub close_before: Option<String>,
    /// At most this many (default 500).
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct StageChange {
    pub from_stage_id: Option<Uuid>,
    pub from_stage_name: Option<String>,
    pub to_stage_id: Uuid,
    pub to_stage_name: String,
    pub actor_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

const SELECT: &str = "SELECT o.id, o.org_id, g.name AS org_name, o.number, o.name, o.description, \
        o.stage_id, s.name AS stage_name, s.kind AS stage_kind, o.status, o.amount, o.recurring, \
        o.currency, coalesce(o.probability, s.probability) AS probability, \
        o.probability AS probability_override, \
        round((o.amount + 12 * o.recurring) * coalesce(o.probability, s.probability) / 100, 2) AS weighted, \
        o.close_date, o.forecast, o.owner_id, u.display_name AS owner_name, o.contact_id, \
        nullif(btrim(coalesce(c.first_name, '') || ' ' || coalesce(c.last_name, '')), '') AS contact_name, \
        o.lead_id, o.source, o.reason_id, r.name AS reason_name, o.reason_note, o.closed_at, \
        o.version, o.created_at, o.updated_at \
   FROM sales_opportunities o \
   JOIN organizations g ON g.id = o.org_id \
   JOIN sales_stages s ON s.id = o.stage_id \
   LEFT JOIN users u ON u.id = o.owner_id \
   LEFT JOIN crm_contacts c ON c.id = o.contact_id \
   LEFT JOIN sales_reasons r ON r.id = o.reason_id";

pub async fn get(conn: &mut PgConnection, id: Uuid) -> SalesResult<Opportunity> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE o.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(SalesError::NotFound)
}

/// Opportunities you can see: open ones by close date, then the rest newest first.
pub async fn list(conn: &mut PgConnection, filter: &Filter) -> SalesResult<Vec<Opportunity>> {
    let q = filter
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(|q| q.trim_start_matches(['O', 'o', 'P', 'p', '-']).to_owned());
    let before = filter
        .close_before
        .as_deref()
        .map(|d| {
            Date::parse(d, time::macros::format_description!("[year]-[month]-[day]"))
                .map_err(|_| SalesError::invalid("closeBefore", "a date, YYYY-MM-DD"))
        })
        .transpose()?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::text IS NULL OR o.status = $1) \
            AND ($2::uuid IS NULL OR o.stage_id = $2) \
            AND ($3::uuid IS NULL OR o.owner_id = $3) \
            AND ($4::uuid IS NULL OR o.org_id = $4) \
            AND ($5::text IS NULL OR o.name ILIKE '%' || $5 || '%' OR g.name ILIKE '%' || $5 || '%' \
                 OR o.number::text = $5) \
            AND ($6::date IS NULL OR o.close_date <= $6) \
          ORDER BY o.status <> 'open', o.close_date NULLS LAST, o.updated_at DESC \
          LIMIT $7"
    )))
    .bind(filter.status)
    .bind(filter.stage_id)
    .bind(filter.owner_id)
    .bind(filter.org_id)
    .bind(q)
    .bind(before)
    .bind(filter.limit.unwrap_or(500).clamp(1, 2000))
    .fetch_all(conn)
    .await?)
}

/// The moves between stages, oldest first.
pub async fn history(conn: &mut PgConnection, id: Uuid) -> SalesResult<Vec<StageChange>> {
    Ok(sqlx::query_as(
        "SELECT h.from_stage_id, f.name AS from_stage_name, h.to_stage_id, t.name AS to_stage_name, \
                u.display_name AS actor_name, h.at \
           FROM sales_stage_history h \
           LEFT JOIN sales_stages f ON f.id = h.from_stage_id \
           JOIN sales_stages t ON t.id = h.to_stage_id \
           LEFT JOIN users u ON u.id = h.actor_id \
          WHERE h.opportunity_id = $1 ORDER BY h.at, h.id",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

struct Checked {
    name: String,
    currency: String,
    source: Option<String>,
}

fn check(input: &OpportunityInput) -> SalesResult<Checked> {
    if input.amount < Decimal::ZERO {
        return Err(SalesError::invalid("amount", "can't be negative"));
    }
    if input.recurring < Decimal::ZERO {
        return Err(SalesError::invalid("recurring", "can't be negative"));
    }
    if input.probability.is_some_and(|p| !(0..=100).contains(&p)) {
        return Err(SalesError::invalid("probability", "0 to 100"));
    }
    if input.description.chars().count() > 8000 {
        return Err(SalesError::invalid(
            "description",
            "at most 8000 characters",
        ));
    }
    Ok(Checked {
        name: required("name", &input.name, 200)?,
        currency: currency(&input.currency)?,
        source: text("source", input.source.as_deref(), 60)?,
    })
}

/// The contact must belong to the client (as any of its companies).
async fn check_contact(
    conn: &mut PgConnection,
    org_id: Uuid,
    contact: Option<Uuid>,
) -> SalesResult<()> {
    let Some(contact) = contact else {
        return Ok(());
    };
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM crm_contact_companies WHERE contact_id = $1 AND org_id = $2)",
    )
    .bind(contact)
    .bind(org_id)
    .fetch_one(conn)
    .await?;
    if ok {
        Ok(())
    } else {
        Err(SalesError::invalid(
            "contactId",
            "not a contact of this client",
        ))
    }
}

pub(crate) async fn emit(
    conn: &mut PgConnection,
    event: &'static str,
    o: &Opportunity,
    extra: Json,
) -> SalesResult<()> {
    let mut payload = json!({
        "opportunityId": o.id,
        "number": o.number,
        "name": o.name,
        "orgId": o.org_id,
        "stage": o.stage_name,
        "status": o.status,
        "amount": o.amount.normalize().to_string(),
        "recurring": o.recurring.normalize().to_string(),
        "currency": o.currency,
        "ownerId": o.owner_id,
    });
    if let (Some(p), Json::Object(extra)) = (payload.as_object_mut(), extra) {
        p.extend(extra);
    }
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(o.org_id),
            subject_type: "opportunity",
            subject_id: o.id,
            payload,
        },
    )
    .await?;
    Ok(())
}

/// Creates an opportunity for the client `org_id`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &OpportunityInput,
    lead_id: Option<Uuid>,
) -> SalesResult<Opportunity> {
    let c = check(input)?;
    check_contact(&mut *conn, org_id, input.contact_id).await?;
    let stage = match input.stage_id {
        Some(id) => stages::get(&mut *conn, id).await?,
        None => stages::first_of(&mut *conn, StageKind::Open).await?,
    };
    if stage.kind != StageKind::Open {
        return Err(SalesError::invalid("stageId", "start at an open stage"));
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_opportunities (id, org_id, name, description, stage_id, amount, recurring, \
            currency, probability, close_date, forecast, owner_id, contact_id, lead_id, source, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.name)
    .bind(&input.description)
    .bind(stage.id)
    .bind(input.amount)
    .bind(input.recurring)
    .bind(&c.currency)
    .bind(input.probability)
    .bind(input.close_date)
    .bind(input.forecast)
    .bind(input.owner_id.or_else(|| ctx.actor.id()))
    .bind(input.contact_id)
    .bind(lead_id)
    .bind(&c.source)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    record_move(&mut *conn, ctx, id, org_id, None, stage.id).await?;
    let o = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("opportunity.created")
            .org(org_id)
            .target("opportunity", id)
            .details(json!({ "name": o.name, "stage": o.stage_name })),
    )
    .await?;
    emit(&mut *conn, "sales.opportunity.created", &o, json!({})).await?;
    Ok(o)
}

/// Changes its details (not its stage: see [`move_to`]).
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &OpportunityInput,
) -> SalesResult<Opportunity> {
    let c = check(input)?;
    let current = get(&mut *conn, id).await?;
    check_contact(&mut *conn, current.org_id, input.contact_id).await?;
    let version = input
        .version
        .ok_or_else(|| SalesError::invalid("version", "send the version you read"))?;
    let done = sqlx::query(
        "UPDATE sales_opportunities SET name = $3, description = $4, amount = $5, recurring = $6, \
            currency = $7, probability = $8, close_date = $9, forecast = $10, owner_id = $11, \
            contact_id = $12, source = $13, version = version + 1, updated_at = now() \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .bind(&c.name)
    .bind(&input.description)
    .bind(input.amount)
    .bind(input.recurring)
    .bind(&c.currency)
    .bind(input.probability)
    .bind(input.close_date)
    .bind(input.forecast)
    .bind(input.owner_id)
    .bind(input.contact_id)
    .bind(&c.source)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::VersionConflict);
    }
    let o = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("opportunity.updated")
            .org(o.org_id)
            .target("opportunity", id)
            .details(json!({
                "before": { "name": current.name, "amount": current.amount.normalize().to_string(),
                            "closeDate": current.close_date.map(|d| d.to_string()) },
                "after": { "name": o.name, "amount": o.amount.normalize().to_string(),
                           "closeDate": o.close_date.map(|d| d.to_string()) },
            })),
    )
    .await?;
    emit(&mut *conn, "sales.opportunity.updated", &o, json!({})).await?;
    Ok(o)
}

async fn record_move(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    org_id: Uuid,
    from: Option<Uuid>,
    to: Uuid,
) -> SalesResult<()> {
    sqlx::query(
        "INSERT INTO sales_stage_history (id, opportunity_id, org_id, from_stage_id, to_stage_id, actor_id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(huxley_core::new_id())
    .bind(id)
    .bind(org_id)
    .bind(from)
    .bind(to)
    .bind(ctx.actor.id())
    .execute(conn)
    .await?;
    Ok(())
}

/// Moves it to a stage. A won or lost stage closes it, with the reason if
/// given (of the matching kind); an open stage reopens it.
pub async fn move_to(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &MoveInput,
) -> SalesResult<Opportunity> {
    let current = get(&mut *conn, id).await?;
    if current.version != input.version {
        return Err(SalesError::VersionConflict);
    }
    let stage = stages::get(&mut *conn, input.stage_id).await?;
    if stage.id == current.stage_id {
        return Ok(current);
    }
    let status = match stage.kind {
        StageKind::Open => Status::Open,
        StageKind::Won => Status::Won,
        StageKind::Lost => Status::Lost,
    };
    let reason = match (status, input.reason_id) {
        (Status::Open, _) | (_, None) => None,
        (_, Some(reason)) => {
            let kind: Option<String> =
                sqlx::query_scalar("SELECT kind FROM sales_reasons WHERE id = $1")
                    .bind(reason)
                    .fetch_optional(&mut *conn)
                    .await?;
            let wanted = if status == Status::Won { "won" } else { "lost" };
            if kind.as_deref() != Some(wanted) {
                return Err(SalesError::invalid(
                    "reasonId",
                    format!("not a reason for a {wanted} opportunity"),
                ));
            }
            Some(reason)
        }
    };
    let note = if status == Status::Open {
        None
    } else {
        text("reasonNote", input.reason_note.as_deref(), 2000)?
    };
    let done = sqlx::query(
        "UPDATE sales_opportunities SET stage_id = $3, status = $4, reason_id = $5, reason_note = $6, \
            closed_at = CASE WHEN $4 = 'open' THEN NULL ELSE coalesce(closed_at, now()) END, \
            version = version + 1, updated_at = now() \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(input.version)
    .bind(stage.id)
    .bind(status)
    .bind(reason)
    .bind(&note)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::VersionConflict);
    }
    record_move(
        &mut *conn,
        ctx,
        id,
        current.org_id,
        Some(current.stage_id),
        stage.id,
    )
    .await?;
    let o = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("opportunity.stage_changed")
            .org(o.org_id)
            .target("opportunity", id)
            .details(
                json!({ "from": current.stage_name, "to": o.stage_name, "reason": o.reason_name }),
            ),
    )
    .await?;
    let extra = json!({ "fromStage": current.stage_name, "reason": o.reason_name });
    emit(
        &mut *conn,
        "sales.opportunity.stage_changed",
        &o,
        extra.clone(),
    )
    .await?;
    if status != current.status {
        match status {
            Status::Won => emit(&mut *conn, "sales.opportunity.won", &o, extra).await?,
            Status::Lost => emit(&mut *conn, "sales.opportunity.lost", &o, extra).await?,
            Status::Open => {}
        }
    }
    Ok(o)
}

/// Deletes an opportunity (its history goes with it).
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> SalesResult<()> {
    let o = get(&mut *conn, id).await?;
    sqlx::query("DELETE FROM sales_opportunities WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("opportunity.deleted")
            .org(o.org_id)
            .target("opportunity", id)
            .details(json!({ "name": o.name, "number": o.number })),
    )
    .await?;
    Ok(())
}

/// Who owns it and where it is, for access checks.
pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> SalesResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM sales_opportunities WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}
