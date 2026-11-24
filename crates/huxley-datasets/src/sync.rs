//! Syncing a dataset from an integration (design doc §9): each sync is an
//! ordinary workflow run, built from the dataset's sync settings, in each
//! target organization: an `integration.action` that follows every page,
//! then `dataset.upsert` with the items mapped to columns. Runs show up
//! with the rest, retry like the rest, and use the integration's
//! pagination, rate limits and per-organization connections.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_hwf::ScheduleTrigger;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{Dataset, DatasetError, DatasetResult, invalid};

/// How a sync writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Replace each organization's rows with what was fetched.
    #[default]
    Replace,
    /// Add and update rows by key; nothing is removed.
    Upsert,
}

/// Which organizations a sync runs for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncOrgs {
    /// The dataset's own organization.
    #[default]
    Own,
    /// Every client organization below it (each with its own connection).
    Clients,
    /// The dataset's organization and every one below it.
    All,
}

/// When a sync runs on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncSchedule {
    /// Five-field cron expression. Set this or `interval`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// e.g. `6h`. Set this or `cron`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// IANA time zone for `cron` (default: the organization's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl SyncSchedule {
    /// As a workflow schedule trigger, to compute next runs with.
    pub fn trigger(&self) -> ScheduleTrigger {
        serde_json::from_value(
            json!({ "cron": self.cron, "interval": self.interval, "timezone": self.timezone }),
        )
        .expect("schedule fields match")
    }
}

/// A dataset's sync settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncConfig {
    /// The list action, `<integration>/<action>` (with pagination set in
    /// the integration's overlay, all pages are fetched).
    pub action: String,
    /// A connection key (default: the nearest the organization can use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// The action's inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub with: Option<Json>,
    /// The most pages fetched (default 20, at most 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u32>,
    /// Column → dot path into each fetched item.
    pub map: BTreeMap<String, String>,
    #[serde(default)]
    pub mode: SyncMode,
    #[serde(default)]
    pub orgs: SyncOrgs,
    /// None: only when started by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<SyncSchedule>,
}

/// Checks sync settings against `dataset`.
pub fn check(dataset: &Dataset, config: &SyncConfig) -> DatasetResult<()> {
    let mut parts = config.action.split('/');
    let ok = matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if !a.is_empty() && !b.is_empty());
    if !ok {
        return Err(invalid(
            "sync.action",
            "<integration>/<action>, e.g. msgraph/listUsers",
        ));
    }
    if config.map.is_empty() {
        return Err(invalid("sync.map", "map at least one column"));
    }
    for (column, path) in &config.map {
        if dataset.column(column).is_none() {
            return Err(invalid(
                format!("sync.map.{column}"),
                "not a column of this dataset",
            ));
        }
        if path.trim().is_empty() {
            return Err(invalid(
                format!("sync.map.{column}"),
                "a path into each item, e.g. userPrincipalName",
            ));
        }
    }
    for key in &dataset.primary_key {
        if !config.map.contains_key(key) {
            return Err(invalid("sync.map", format!("map the key column {key:?}")));
        }
    }
    if config.mode == SyncMode::Upsert && dataset.primary_key.is_empty() {
        return Err(invalid(
            "sync.mode",
            "upserting needs the dataset to have a primary key",
        ));
    }
    if config.max_pages.is_some_and(|p| p == 0 || p > 100) {
        return Err(invalid("sync.maxPages", "1 to 100"));
    }
    if let Some(schedule) = &config.schedule
        && schedule.cron.is_some() == schedule.interval.is_some()
    {
        return Err(invalid("sync.schedule", "set cron or interval"));
    }
    Ok(())
}

/// The workflow document a sync of `dataset` runs.
pub fn document(dataset: &Dataset, config: &SyncConfig) -> Json {
    let mut fetch = json!({
        "id": "fetch", "type": "integration.action", "action": config.action, "all_pages": true
    });
    if let Some(connection) = &config.connection {
        fetch["connection"] = json!(connection);
    }
    if let Some(with) = &config.with {
        fetch["with"] = with.clone();
    }
    if let Some(pages) = config.max_pages {
        fetch["max_pages"] = json!(pages);
    }
    let mut steps = vec![fetch];
    let replace = config.mode == SyncMode::Replace;
    if replace {
        // A partial fetch must not replace (and so delete) the rest.
        steps.push(json!({
            "id": "complete", "type": "if", "condition": "{{ steps.fetch.output.truncated }}",
            "then": [{
                "id": "incomplete", "type": "fail",
                "message": "There were more pages than the sync's page limit, so nothing was replaced. Raise maxPages."
            }]
        }));
    }
    steps.push(json!({
        "id": "save", "type": "dataset.upsert",
        "with": {
            // By id: the dataset itself, even if an organization below has
            // one with the same key.
            "dataset": dataset.id.to_string(),
            "rows": "{{ steps.fetch.output.items }}",
            "map": config.map,
            "replace": replace
        }
    }));
    json!({
        "apiVersion": "usehuxley.io/v1",
        "kind": "Workflow",
        "metadata": { "key": format!("sync-{}", dataset.key), "name": format!("Sync {}", dataset.name) },
        "spec": {
            "steps": steps,
            "outputs": { "fetched": "{{ steps.fetch.output.items | length }}", "written": "{{ steps.save.output.written }}" }
        }
    })
}

impl Dataset {
    /// Its sync settings, if it syncs.
    pub async fn sync(&self, conn: &mut PgConnection) -> DatasetResult<Option<SyncConfig>> {
        let raw: Option<Json> = sqlx::query_scalar("SELECT sync FROM datasets WHERE id = $1")
            .bind(self.id)
            .fetch_optional(conn)
            .await?
            .flatten();
        Ok(raw.and_then(|v| serde_json::from_value(v).ok()))
    }
}

/// Sets (or with `None` removes) a dataset's sync, and when it next runs.
pub async fn set(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    dataset: &Dataset,
    config: Option<&SyncConfig>,
    next_at: Option<OffsetDateTime>,
) -> DatasetResult<()> {
    if let Some(config) = config {
        check(dataset, config)?;
    }
    sqlx::query(
        "UPDATE datasets SET sync = $2, sync_next_at = $3, updated_at = now() WHERE id = $1",
    )
    .bind(dataset.id)
    .bind(config.map(|c| serde_json::to_value(c).unwrap_or_default()))
    .bind(next_at)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("dataset.sync_changed")
            .org(dataset.org_id)
            .target("dataset", dataset.id)
            .details(json!({ "key": dataset.key, "action": config.map(|c| &c.action), "scheduled": next_at.is_some() })),
    )
    .await
    .map_err(|e| DatasetError::Db(sqlx::Error::Protocol(e.to_string())))?;
    Ok(())
}

/// The organizations a sync of `dataset` runs for.
pub async fn targets(
    conn: &mut PgConnection,
    dataset: &Dataset,
    orgs: SyncOrgs,
) -> DatasetResult<Vec<Uuid>> {
    let filter = match orgs {
        SyncOrgs::Own => return Ok(vec![dataset.org_id]),
        SyncOrgs::Clients => "AND o.kind = 'client'",
        SyncOrgs::All => "",
    };
    let ids = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT o.id FROM organizations o \
          WHERE o.path <@ (SELECT path FROM organizations WHERE id = $1) AND o.status = 'active' {filter} \
          ORDER BY o.path"
    )))
    .bind(dataset.org_id)
    .fetch_all(conn)
    .await?;
    Ok(ids)
}

/// Datasets whose sync is due, locked for this transaction (skipping any
/// another scheduler holds).
pub async fn due(conn: &mut PgConnection, limit: i64) -> DatasetResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM datasets WHERE sync IS NOT NULL AND sync_next_at <= now() \
          ORDER BY sync_next_at LIMIT $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Records that a sync started, and when the next one is.
pub async fn started(
    conn: &mut PgConnection,
    id: Uuid,
    next_at: Option<OffsetDateTime>,
) -> DatasetResult<()> {
    sqlx::query("UPDATE datasets SET sync_last_at = now(), sync_next_at = $2 WHERE id = $1")
        .bind(id)
        .bind(next_at)
        .execute(conn)
        .await?;
    Ok(())
}
