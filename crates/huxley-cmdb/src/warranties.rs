//! Warranties per asset: from the manufacturer, an extension or a support
//! contract, entered by hand or recorded by a vendor lookup (a workflow's
//! `core.asset.set_warranty`). An asset's `warranty_ends_on` follows its
//! latest warranty. A sweeper announces each warranty once when it's about
//! to end (`asset.warranty_expiring`, within [`EXPIRING_DAYS`]) and once
//! when it has (`asset.warranty_expired`); changing the end date re-arms
//! both. Retired and disposed assets aren't announced.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CmdbError, CmdbResult, assets, text};

/// Kinds of warranty.
pub const KINDS: &[&str] = &["manufacturer", "extended", "support", "other"];
/// How far ahead an ending warranty is announced.
pub const EXPIRING_DAYS: i32 = 60;

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Warranty {
    pub id: Uuid,
    pub asset_id: Uuid,
    pub kind: String,
    pub provider: String,
    pub reference: String,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub starts_on: Option<Date>,
    #[serde(with = "crate::date")]
    #[schema(value_type = String, format = Date)]
    pub ends_on: Date,
    pub notes: String,
    /// `manual`, or the lookup that recorded it.
    pub source: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WarrantyInput {
    /// `manufacturer` (the default), `extended`, `support` or `other`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    /// A contract number, service tag or order reference.
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default, with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub starts_on: Option<Date>,
    #[serde(with = "crate::date")]
    #[schema(value_type = String, format = Date)]
    pub ends_on: Date,
    #[serde(default)]
    pub notes: Option<String>,
}

const SELECT: &str = "SELECT id, asset_id, kind, provider, reference, starts_on, ends_on, notes, source, updated_at \
    FROM cmdb_warranties";

/// An asset's warranties, latest ending first.
pub async fn for_asset(conn: &mut PgConnection, asset: Uuid) -> CmdbResult<Vec<Warranty>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE asset_id = $1 ORDER BY ends_on DESC, created_at"
    )))
    .bind(asset)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> CmdbResult<Warranty> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CmdbError::NotFound)
}

struct Checked {
    kind: String,
    provider: String,
    reference: String,
    notes: String,
}

fn check(input: &WarrantyInput) -> CmdbResult<Checked> {
    let kind = input.kind.clone().unwrap_or_else(|| "manufacturer".into());
    if !KINDS.contains(&kind.as_str()) {
        return Err(CmdbError::invalid("kind", KINDS.join(", ")));
    }
    if input.starts_on.is_some_and(|s| s > input.ends_on) {
        return Err(CmdbError::invalid("endsOn", "on or after the start"));
    }
    Ok(Checked {
        kind,
        provider: text("provider", input.provider.as_deref(), 200)?.unwrap_or_default(),
        reference: text("reference", input.reference.as_deref(), 200)?.unwrap_or_default(),
        notes: text("notes", input.notes.as_deref(), 2000)?.unwrap_or_default(),
    })
}

/// Keeps the asset's `warranty_ends_on` at its latest warranty's end.
async fn follow(conn: &mut PgConnection, asset: Uuid) -> CmdbResult<()> {
    sqlx::query(
        "UPDATE assets SET warranty_ends_on = (SELECT max(ends_on) FROM cmdb_warranties WHERE asset_id = $1) WHERE id = $1",
    )
    .bind(asset)
    .execute(conn)
    .await?;
    Ok(())
}

async fn audited(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    event: &str,
    w: &Warranty,
    org: Uuid,
) -> CmdbResult<()> {
    audit::record(
        conn,
        ctx,
        AuditRecord::new(event).org(org).target("asset", w.asset_id).details(json!({
            "warrantyId": w.id, "kind": w.kind, "provider": w.provider, "endsOn": w.ends_on.to_string(), "source": w.source,
        })),
    )
    .await?;
    Ok(())
}

/// Adds a warranty to an asset by hand.
pub async fn add(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    asset: Uuid,
    input: &WarrantyInput,
) -> CmdbResult<Warranty> {
    let a = assets::get(conn, asset).await?;
    let c = check(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO cmdb_warranties (id, asset_id, org_id, kind, provider, reference, starts_on, ends_on, notes, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(asset)
    .bind(a.org_id)
    .bind(&c.kind)
    .bind(&c.provider)
    .bind(&c.reference)
    .bind(input.starts_on)
    .bind(input.ends_on)
    .bind(&c.notes)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    follow(conn, asset).await?;
    let w = get(conn, id).await?;
    audited(conn, ctx, "asset.warranty_added", &w, a.org_id).await?;
    Ok(w)
}

/// Changes a warranty. A new end date re-arms its announcements.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &WarrantyInput,
) -> CmdbResult<Warranty> {
    let before = get(conn, id).await?;
    let c = check(input)?;
    sqlx::query(
        "UPDATE cmdb_warranties SET kind = $2, provider = $3, reference = $4, starts_on = $5, ends_on = $6, notes = $7, \
                warned_at = CASE WHEN ends_on = $6 THEN warned_at END, \
                expired_at = CASE WHEN ends_on = $6 THEN expired_at END, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(&c.kind)
    .bind(&c.provider)
    .bind(&c.reference)
    .bind(input.starts_on)
    .bind(input.ends_on)
    .bind(&c.notes)
    .execute(&mut *conn)
    .await?;
    follow(conn, before.asset_id).await?;
    let w = get(conn, id).await?;
    let org = assets::get(conn, w.asset_id).await?.org_id;
    audited(conn, ctx, "asset.warranty_updated", &w, org).await?;
    Ok(w)
}

pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> CmdbResult<()> {
    let w = get(conn, id).await?;
    let org = assets::get(conn, w.asset_id).await?.org_id;
    sqlx::query("DELETE FROM cmdb_warranties WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    follow(conn, w.asset_id).await?;
    audited(conn, ctx, "asset.warranty_removed", &w, org).await
}

/// Records a warranty a lookup found (`source`, e.g. `dell`): adds it, or
/// updates the one with the same reference. Returns it.
pub async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    asset: Uuid,
    source: &str,
    input: &WarrantyInput,
) -> CmdbResult<Warranty> {
    let mut chars = source.chars();
    if source == "manual"
        || source.len() > 32
        || !chars.next().is_some_and(|c| c.is_ascii_lowercase())
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(CmdbError::invalid(
            "source",
            "the lookup's name: lowercase letters, digits and _",
        ));
    }
    let a = assets::get(conn, asset).await?;
    let c = check(input)?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO cmdb_warranties (id, asset_id, org_id, kind, provider, reference, starts_on, ends_on, notes, source, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         ON CONFLICT (asset_id, source, reference) WHERE source <> 'manual' DO UPDATE SET \
                kind = EXCLUDED.kind, provider = EXCLUDED.provider, starts_on = EXCLUDED.starts_on, notes = EXCLUDED.notes, \
                warned_at = CASE WHEN cmdb_warranties.ends_on = EXCLUDED.ends_on THEN cmdb_warranties.warned_at END, \
                expired_at = CASE WHEN cmdb_warranties.ends_on = EXCLUDED.ends_on THEN cmdb_warranties.expired_at END, \
                ends_on = EXCLUDED.ends_on, updated_at = now() \
         RETURNING id",
    )
    .bind(huxley_core::new_id())
    .bind(asset)
    .bind(a.org_id)
    .bind(&c.kind)
    .bind(&c.provider)
    .bind(&c.reference)
    .bind(input.starts_on)
    .bind(input.ends_on)
    .bind(&c.notes)
    .bind(source)
    .bind(ctx.actor.id())
    .fetch_one(&mut *conn)
    .await?;
    follow(conn, asset).await?;
    let w = get(conn, id).await?;
    audited(conn, ctx, "asset.warranty_recorded", &w, a.org_id).await?;
    Ok(w)
}

/// A warranty the sweep claimed.
#[derive(sqlx::FromRow)]
struct Ending {
    id: Uuid,
    asset: Uuid,
    org: Uuid,
    name: String,
    kind: String,
    provider: String,
    reference: String,
    ends_on: String,
}

/// Announces warranties of assets in use that are about to end, or have,
/// each once. Returns how many were announced. System scope.
pub async fn sweep(conn: &mut PgConnection) -> CmdbResult<usize> {
    let mut announced = 0;
    for (event, column, condition) in [
        (
            "asset.warranty_expiring",
            "warned_at",
            "w.ends_on >= current_date AND w.ends_on <= current_date + $1",
        ),
        (
            "asset.warranty_expired",
            "expired_at",
            "w.ends_on < current_date AND $1 IS NOT NULL",
        ),
    ] {
        let hits: Vec<Ending> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "UPDATE cmdb_warranties w SET {column} = now() FROM assets a \
                  WHERE a.id = w.asset_id AND w.{column} IS NULL AND {condition} \
                    AND a.status NOT IN ('retired', 'disposed') \
                  RETURNING w.id, a.id AS asset, a.org_id AS org, a.name, w.kind, w.provider, w.reference, \
                         to_char(w.ends_on, 'YYYY-MM-DD') AS ends_on"
            )))
            .bind(EXPIRING_DAYS)
            .fetch_all(&mut *conn)
            .await?;
        for Ending {
            id,
            asset,
            org,
            name,
            kind,
            provider,
            reference,
            ends_on,
        } in hits
        {
            outbox::emit(
                conn,
                DomainEvent {
                    event_type: event,
                    org_id: Some(org),
                    subject_type: "asset",
                    subject_id: asset,
                    payload: json!({
                        "name": name,
                        "warrantyId": id,
                        "kind": kind,
                        "provider": provider,
                        "reference": reference,
                        "endsOn": ends_on,
                    }),
                },
            )
            .await?;
            announced += 1;
        }
    }
    Ok(announced)
}

/// How often warranties are looked at.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Announces expiring and expired warranties until shutdown, while the
/// module is on. Safe in several processes: each is claimed once.
pub async fn run_sweeper(db: sqlx::PgPool, shutdown: tokio_util::sync::CancellationToken) {
    loop {
        if let Err(e) = sweep_once(&db).await {
            tracing::error!(error = %e, "warranty sweep failed");
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(SWEEP_EVERY) => {}
        }
    }
}

async fn sweep_once(db: &sqlx::PgPool) -> CmdbResult<()> {
    let mut tx = huxley_db::begin_scoped(db, &huxley_db::OrgScope::System).await?;
    if huxley_modules::is_active(&mut tx, crate::MODULE)
        .await
        .map_err(|e| CmdbError::Sqlx(sqlx::Error::Protocol(e.to_string())))?
    {
        let announced = sweep(&mut tx).await?;
        if announced > 0 {
            tracing::info!(announced, "announced expiring and expired warranties");
        }
    }
    tx.commit().await?;
    Ok(())
}
