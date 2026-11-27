//! Pipeline stages and win/loss reasons, instance-wide.
//!
//! - Stages are ordered; each is `open`, `won` or `lost`, with the chance of
//!   winning at it (won is 100, lost is 0). There is always at least one of
//!   each kind.
//! - A stage in use is archived rather than deleted; opportunities keep it.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{SalesError, SalesResult, required};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = SalesStageKind)]
pub enum StageKind {
    Open,
    Won,
    Lost,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesStage)]
pub struct Stage {
    pub id: Uuid,
    pub name: String,
    pub kind: StageKind,
    /// The chance of winning at this stage, 0 to 100.
    pub probability: i16,
    pub position: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SalesStageInput)]
pub struct StageInput {
    pub name: String,
    pub kind: StageKind,
    pub probability: i16,
}

/// The stages in order (archived ones left out).
pub async fn list(conn: &mut PgConnection) -> SalesResult<Vec<Stage>> {
    Ok(sqlx::query_as(
        "SELECT id, name, kind, probability, position FROM sales_stages \
          WHERE archived_at IS NULL ORDER BY position, name",
    )
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> SalesResult<Stage> {
    sqlx::query_as(
        "SELECT id, name, kind, probability, position FROM sales_stages \
          WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(SalesError::NotFound)
}

/// The first stage of a kind, by position.
pub async fn first_of(conn: &mut PgConnection, kind: StageKind) -> SalesResult<Stage> {
    sqlx::query_as(
        "SELECT id, name, kind, probability, position FROM sales_stages \
          WHERE kind = $1 AND archived_at IS NULL ORDER BY position LIMIT 1",
    )
    .bind(kind)
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| SalesError::Conflict(format!("no {kind:?} stage is set up").to_lowercase()))
}

fn check(input: &StageInput) -> SalesResult<(String, i16)> {
    let name = required("name", &input.name, 60)?;
    let probability = match input.kind {
        StageKind::Won => 100,
        StageKind::Lost => 0,
        StageKind::Open if (0..=100).contains(&input.probability) => input.probability,
        StageKind::Open => return Err(SalesError::invalid("probability", "0 to 100")),
    };
    Ok((name, probability))
}

fn taken(name: &str) -> impl FnOnce(sqlx::Error) -> SalesError + '_ {
    move |e| match huxley_db::violated_constraint(&e) {
        Some("sales_stages_name") => SalesError::invalid("name", format!("{name} is taken")),
        _ => e.into(),
    }
}

/// Adds a stage at the end of its kind's place in the order.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &StageInput,
) -> SalesResult<Stage> {
    let (name, probability) = check(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_stages (id, name, kind, probability, position) \
         VALUES ($1, $2, $3, $4, (SELECT coalesce(max(position), 0) + 1 FROM sales_stages))",
    )
    .bind(id)
    .bind(&name)
    .bind(input.kind)
    .bind(probability)
    .execute(&mut *conn)
    .await
    .map_err(taken(&name))?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_stage.created")
            .target("sales_stage", id)
            .details(json!({ "name": name, "kind": input.kind })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &StageInput,
) -> SalesResult<Stage> {
    let (name, probability) = check(input)?;
    let current = get(&mut *conn, id).await?;
    if current.kind != input.kind {
        last_of_kind(&mut *conn, &current).await?;
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sales_opportunities WHERE stage_id = $1)",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
        if used {
            return Err(SalesError::Conflict(
                "opportunities are at this stage, so its kind can't change".into(),
            ));
        }
    }
    sqlx::query("UPDATE sales_stages SET name = $2, kind = $3, probability = $4 WHERE id = $1")
        .bind(id)
        .bind(&name)
        .bind(input.kind)
        .bind(probability)
        .execute(&mut *conn)
        .await
        .map_err(taken(&name))?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_stage.updated")
            .target("sales_stage", id)
            .details(json!({ "name": name, "kind": input.kind, "probability": probability })),
    )
    .await?;
    get(conn, id).await
}

async fn last_of_kind(conn: &mut PgConnection, stage: &Stage) -> SalesResult<()> {
    let others: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sales_stages WHERE kind = $1 AND id <> $2 AND archived_at IS NULL",
    )
    .bind(stage.kind)
    .bind(stage.id)
    .fetch_one(conn)
    .await?;
    if others == 0 {
        return Err(SalesError::Conflict(format!(
            "it's the only {} stage",
            match stage.kind {
                StageKind::Open => "open",
                StageKind::Won => "won",
                StageKind::Lost => "lost",
            }
        )));
    }
    Ok(())
}

/// Archives a stage. Open opportunities at it must be moved first.
pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> SalesResult<()> {
    let stage = get(&mut *conn, id).await?;
    last_of_kind(&mut *conn, &stage).await?;
    let open: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sales_opportunities WHERE stage_id = $1 AND status = 'open')",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if open {
        return Err(SalesError::Conflict(
            "open opportunities are at this stage; move them first".into(),
        ));
    }
    sqlx::query("UPDATE sales_stages SET archived_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sales_stage.archived")
            .target("sales_stage", id)
            .details(json!({ "name": stage.name })),
    )
    .await?;
    Ok(())
}

/// Puts the stages in this order (every current stage, once).
pub async fn reorder(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ids: &[Uuid],
) -> SalesResult<Vec<Stage>> {
    let current = list(&mut *conn).await?;
    let mut given = ids.to_vec();
    given.sort_unstable();
    let mut have: Vec<Uuid> = current.iter().map(|s| s.id).collect();
    have.sort_unstable();
    if given != have {
        return Err(SalesError::invalid("ids", "list every stage once"));
    }
    for (i, id) in ids.iter().enumerate() {
        sqlx::query("UPDATE sales_stages SET position = $2 WHERE id = $1")
            .bind(id)
            .bind(i32::try_from(i).unwrap_or(i32::MAX))
            .execute(&mut *conn)
            .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_stage.reordered").details(json!({ "ids": ids })),
    )
    .await?;
    list(conn).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = SalesReasonKind)]
pub enum ReasonKind {
    Won,
    Lost,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesReason)]
pub struct Reason {
    pub id: Uuid,
    pub kind: ReasonKind,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SalesReasonInput)]
pub struct ReasonInput {
    pub kind: ReasonKind,
    pub name: String,
}

pub async fn reasons(conn: &mut PgConnection) -> SalesResult<Vec<Reason>> {
    Ok(sqlx::query_as(
        "SELECT id, kind, name FROM sales_reasons WHERE archived_at IS NULL ORDER BY kind, position, name",
    )
    .fetch_all(conn)
    .await?)
}

pub async fn add_reason(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &ReasonInput,
) -> SalesResult<Reason> {
    let name = required("name", &input.name, 100)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_reasons (id, kind, name, position) \
         VALUES ($1, $2, $3, (SELECT coalesce(max(position), 0) + 1 FROM sales_reasons WHERE kind = $2))",
    )
    .bind(id)
    .bind(input.kind)
    .bind(&name)
    .execute(&mut *conn)
    .await
    .map_err(|e| match huxley_db::violated_constraint(&e) {
        Some("sales_reasons_name") => SalesError::invalid("name", format!("{name} is taken")),
        _ => e.into(),
    })?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_reason.created")
            .target("sales_reason", id)
            .details(json!({ "name": name, "kind": input.kind })),
    )
    .await?;
    Ok(Reason {
        id,
        kind: input.kind,
        name,
    })
}

/// Archives a reason; closed opportunities keep it.
pub async fn archive_reason(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> SalesResult<()> {
    let done = sqlx::query(
        "UPDATE sales_reasons SET archived_at = now() WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sales_reason.archived").target("sales_reason", id),
    )
    .await?;
    Ok(())
}
