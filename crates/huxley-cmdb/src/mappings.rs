//! Dataset → asset mapping rules (design doc §8.5 "Discovery and sync").
//!
//! A rule turns a dataset's rows (an RMM's devices, say) into assets of
//! each row's organization:
//!
//! - **Matching:** keys in order (external id, serial number, hostname,
//!   name), within the row's organization; the first key with exactly one
//!   match wins. More than one match is skipped as ambiguous.
//! - **Fields:** columns map to the asset's fields and its type's
//!   attributes. Empty values never clear anything.
//! - **Conflicts:** `manual_wins` keeps values someone set by hand;
//!   `source_wins` replaces them. Each field remembers its source.
//! - **New assets:** made (of the rule's type) for rows that match nothing,
//!   if the rule says so.
//!
//! A run is one transaction; a preview is a run that's rolled back.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::assets::{self, Asset};
use crate::{CmdbError, CmdbResult, text, types};

/// Asset fields a rule can fill, besides attributes.
pub const FIELDS: &[&str] = &["name", "serial_number", "hostname", "manufacturer", "model"];
/// Keys a rule can match on, in the order they're usually tried.
pub const MATCH_FIELDS: &[&str] = &["external_id", "serial_number", "hostname", "name"];
/// Most rows one run reads.
pub const MAX_ROWS: i64 = 20_000;
/// Most problems a result lists.
const MAX_PROBLEMS: usize = 20;

/// One match key: an asset field and the column holding its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AssetMatchKey)]
pub struct MatchKey {
    /// `external_id`, `serial_number`, `hostname` or `name`.
    pub field: String,
    pub column: String,
}

/// Which column fills which asset field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AssetFieldMap)]
pub struct FieldMap {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The asset's id in the source, kept under `externalIds[source]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// Type attributes: `{ attribute key: column }`.
    #[serde(default)]
    pub attributes: std::collections::BTreeMap<String, String>,
}

impl FieldMap {
    /// `(field, column)` for the plain fields.
    fn plain(&self) -> Vec<(&'static str, &str)> {
        [
            ("name", &self.name),
            ("serial_number", &self.serial_number),
            ("hostname", &self.hostname),
            ("manufacturer", &self.manufacturer),
            ("model", &self.model),
        ]
        .into_iter()
        .filter_map(|(f, c)| c.as_deref().map(|c| (f, c)))
        .collect()
    }
}

/// Which organization a row's asset belongs to: the one (at or below the
/// dataset's) whose variable `variable` equals the row's `column`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AssetOrgMatch)]
pub struct OrgMatch {
    pub column: String,
    /// An organization variable's key, e.g. `automate_client_id`.
    pub variable: String,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssetMapping)]
pub struct Mapping {
    pub id: Uuid,
    pub dataset_id: Uuid,
    pub dataset_name: String,
    pub name: String,
    pub source: String,
    pub type_id: Uuid,
    pub type_name: String,
    #[schema(value_type = Vec<MatchKey>)]
    pub match_keys: sqlx::types::Json<Vec<MatchKey>>,
    #[schema(value_type = FieldMap)]
    pub fields: sqlx::types::Json<FieldMap>,
    /// `manual_wins` or `source_wins`.
    pub conflict: String,
    pub create_missing: bool,
    pub enabled: bool,
    /// Which organization each row's asset belongs to (none: the
    /// organization the row was written for).
    #[schema(value_type = Option<OrgMatch>)]
    pub org_match: Option<sqlx::types::Json<OrgMatch>>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_run_at: Option<OffsetDateTime>,
    #[schema(value_type = Option<SyncResult>)]
    pub last_result: Option<sqlx::types::Json<SyncResult>>,
    pub version: i32,
}

const SELECT: &str = "SELECT m.id, m.dataset_id, d.name AS dataset_name, m.name, m.source, m.type_id, \
    t.name AS type_name, m.match_keys, m.fields, m.conflict, m.create_missing, m.enabled, m.org_match, m.last_run_at, \
    m.last_result, m.version \
    FROM cmdb_mappings m JOIN datasets d ON d.id = m.dataset_id JOIN cmdb_asset_types t ON t.id = m.type_id";

pub async fn list(conn: &mut PgConnection) -> CmdbResult<Vec<Mapping>> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} ORDER BY m.name")))
            .fetch_all(conn)
            .await?,
    )
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> CmdbResult<Mapping> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE m.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CmdbError::NotFound)
}

/// A rule's settings, to create or change it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AssetMappingInput)]
pub struct MappingInput {
    pub dataset_id: Uuid,
    pub name: String,
    /// The source's name, e.g. `rmm`: lowercase letters, digits and `_`.
    pub source: String,
    /// The type of assets it creates.
    pub type_id: Uuid,
    pub match_keys: Vec<MatchKey>,
    pub fields: FieldMap,
    /// `manual_wins` (the default) or `source_wins`.
    #[serde(default)]
    pub conflict: Option<String>,
    #[serde(default = "yes")]
    pub create_missing: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Place each row's asset with the organization whose variable matches
    /// a column; leave out to use the organization the row was written for.
    #[serde(default)]
    pub org_match: Option<OrgMatch>,
    /// The version you read (changes only).
    #[serde(default)]
    pub version: Option<i32>,
}

const fn yes() -> bool {
    true
}

/// Checks a rule against its dataset's columns and type's attributes.
async fn check(conn: &mut PgConnection, input: &MappingInput) -> CmdbResult<(String, String)> {
    let name = text("name", Some(&input.name), 100)?
        .ok_or_else(|| CmdbError::invalid("name", "a mapping needs a name"))?;
    let source = input.source.trim().to_owned();
    let mut chars = source.chars();
    if source.len() > 32
        || !chars.next().is_some_and(|c| c.is_ascii_lowercase())
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(CmdbError::invalid(
            "source",
            "lowercase letters, digits and _, e.g. rmm",
        ));
    }
    let conflict = input
        .conflict
        .clone()
        .unwrap_or_else(|| "manual_wins".into());
    if !matches!(conflict.as_str(), "manual_wins" | "source_wins") {
        return Err(CmdbError::invalid("conflict", "manual_wins or source_wins"));
    }
    let columns: Option<sqlx::types::Json<Vec<Json>>> =
        sqlx::query_scalar("SELECT columns FROM datasets WHERE id = $1")
            .bind(input.dataset_id)
            .fetch_optional(&mut *conn)
            .await?;
    let columns: Vec<String> = columns
        .ok_or_else(|| CmdbError::invalid("datasetId", "there's no such dataset"))?
        .0
        .iter()
        .filter_map(|c| c["name"].as_str().map(str::to_owned))
        .collect();
    let column = |at: &str, c: &str| {
        if columns.iter().any(|x| x == c) {
            Ok(())
        } else {
            Err(CmdbError::invalid(
                at,
                format!("{c:?} isn't a column of the dataset"),
            ))
        }
    };
    if input.match_keys.is_empty() {
        return Err(CmdbError::invalid(
            "matchKeys",
            "at least one key to match assets by",
        ));
    }
    for (i, k) in input.match_keys.iter().enumerate() {
        if !MATCH_FIELDS.contains(&k.field.as_str()) {
            return Err(CmdbError::invalid(
                &format!("matchKeys/{i}/field"),
                MATCH_FIELDS.join(", "),
            ));
        }
        column(&format!("matchKeys/{i}/column"), &k.column)?;
    }
    for (field, c) in input.fields.plain() {
        column(&format!("fields/{field}"), c)?;
    }
    if let Some(c) = &input.fields.external_id {
        column("fields/externalId", c)?;
    }
    let ty = types::get(conn, input.type_id)
        .await
        .map_err(|_| CmdbError::invalid("typeId", "there's no such asset type"))?;
    for (key, c) in &input.fields.attributes {
        if !ty.fields.iter().any(|f| &f.key == key) {
            return Err(CmdbError::invalid(
                &format!("fields/attributes/{key}"),
                format!("{} has no such field", ty.name),
            ));
        }
        column(&format!("fields/attributes/{key}"), c)?;
    }
    if let Some(om) = &input.org_match {
        column("orgMatch/column", &om.column)?;
        let mut chars = om.variable.chars();
        if om.variable.len() > 64
            || !chars.next().is_some_and(|c| c.is_ascii_lowercase())
            || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(CmdbError::invalid(
                "orgMatch/variable",
                "an organization variable's key, e.g. automate_client_id",
            ));
        }
    }
    if input.create_missing
        && input.fields.name.is_none()
        && input.fields.hostname.is_none()
        && input.fields.serial_number.is_none()
    {
        return Err(CmdbError::invalid(
            "fields/name",
            "to create assets, map a name, hostname or serial number",
        ));
    }
    Ok((name, conflict))
}

fn taken(e: sqlx::Error) -> CmdbError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => CmdbError::invalid(
            "source",
            "this dataset already has a mapping for this source",
        ),
        _ => CmdbError::Sqlx(e),
    }
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &MappingInput,
) -> CmdbResult<Mapping> {
    let (name, conflict) = check(conn, input).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO cmdb_mappings (id, dataset_id, name, source, type_id, match_keys, fields, conflict, \
                create_missing, enabled, created_by, org_match) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(input.dataset_id)
    .bind(&name)
    .bind(input.source.trim())
    .bind(input.type_id)
    .bind(sqlx::types::Json(&input.match_keys))
    .bind(sqlx::types::Json(&input.fields))
    .bind(&conflict)
    .bind(input.create_missing)
    .bind(input.enabled)
    .bind(ctx.actor.id())
    .bind(input.org_match.as_ref().map(sqlx::types::Json))
    .execute(&mut *conn)
    .await
    .map_err(taken)?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("cmdb.mapping_created")
            .target("asset_mapping", id)
            .details(json!({ "name": name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &MappingInput,
) -> CmdbResult<Mapping> {
    get(conn, id).await?;
    let (name, conflict) = check(conn, input).await?;
    let rows = sqlx::query(
        "UPDATE cmdb_mappings SET dataset_id = $3, name = $4, source = $5, type_id = $6, match_keys = $7, fields = $8, \
                conflict = $9, create_missing = $10, enabled = $11, org_match = $12, version = version + 1 \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(input.dataset_id)
    .bind(&name)
    .bind(input.source.trim())
    .bind(input.type_id)
    .bind(sqlx::types::Json(&input.match_keys))
    .bind(sqlx::types::Json(&input.fields))
    .bind(&conflict)
    .bind(input.create_missing)
    .bind(input.enabled)
    .bind(input.org_match.as_ref().map(sqlx::types::Json))
    .execute(&mut *conn)
    .await
    .map_err(taken)?
    .rows_affected();
    if rows == 0 {
        return Err(CmdbError::VersionConflict);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("cmdb.mapping_updated")
            .target("asset_mapping", id)
            .details(json!({ "name": name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> CmdbResult<()> {
    let before = get(conn, id).await?;
    sqlx::query("DELETE FROM cmdb_mappings WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("cmdb.mapping_deleted")
            .target("asset_mapping", id)
            .details(json!({ "name": before.name })),
    )
    .await?;
    Ok(())
}

/// A row that couldn't be synced, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssetSyncProblem)]
pub struct Problem {
    /// Its position in the dataset (from 1).
    pub row: usize,
    pub reason: String,
}

/// What a run did (or would do).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssetSyncResult)]
pub struct SyncResult {
    pub rows: usize,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
    /// The first rows skipped, and why.
    pub problems: Vec<Problem>,
    /// Whether this was a preview (nothing was kept).
    pub preview: bool,
}

impl SyncResult {
    fn skip(&mut self, row: usize, reason: impl Into<String>) {
        self.skipped += 1;
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push(Problem {
                row,
                reason: reason.into(),
            });
        }
    }
}

/// A value as text: strings trimmed, numbers and booleans written out,
/// nothing for empty or other values.
fn as_text(v: Option<&Json>) -> Option<String> {
    match v? {
        Json::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Json::Number(n) => Some(n.to_string()),
        Json::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The asset in `org` the row matches by the rule's keys: `Ok(None)` for
/// none, `Err` for more than one.
async fn find(
    conn: &mut PgConnection,
    m: &Mapping,
    org: Uuid,
    row: &Map<String, Json>,
) -> CmdbResult<Result<Option<Uuid>, String>> {
    for key in m.match_keys.iter() {
        let Some(value) = as_text(row.get(&key.column)) else {
            continue;
        };
        let condition = match key.field.as_str() {
            "external_id" => "external_ids ->> $3 = $2",
            "serial_number" => "lower(serial_number) = lower($2)",
            "hostname" => "lower(hostname) = lower($2)",
            _ => "lower(name) = lower($2)",
        };
        let ids: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT id FROM assets WHERE org_id = $1 AND {condition} LIMIT 2"
        )))
        .bind(org)
        .bind(&value)
        .bind(&m.source)
        .fetch_all(&mut *conn)
        .await?;
        match ids.as_slice() {
            [] => {}
            [id] => return Ok(Ok(Some(*id))),
            _ => {
                return Ok(Err(format!(
                    "more than one asset has {} {value:?}",
                    key.field.replace('_', " ")
                )));
            }
        }
    }
    Ok(Ok(None))
}

/// The row's values for the rule's fields: `field → value` (attributes as
/// `attributes.<key>`), leaving out empty ones.
fn values(m: &Mapping, row: &Map<String, Json>) -> Map<String, Json> {
    let mut out = Map::new();
    for (field, column) in m.fields.plain() {
        if let Some(v) = as_text(row.get(column)) {
            out.insert(field.to_owned(), json!(v));
        }
    }
    for (key, column) in &m.fields.attributes {
        match row.get(column) {
            None | Some(Json::Null) => {}
            Some(Json::String(s)) if s.trim().is_empty() => {}
            Some(v) => {
                out.insert(format!("attributes.{key}"), v.clone());
            }
        }
    }
    out
}

/// The external id column: the field map's, or the external-id match key's.
fn external_column(m: &Mapping) -> Option<&str> {
    m.fields.external_id.as_deref().or_else(|| {
        m.match_keys
            .iter()
            .find(|k| k.field == "external_id")
            .map(|k| k.column.as_str())
    })
}

/// Runs a rule over its dataset's rows (up to [`MAX_ROWS`]), in the
/// caller's transaction (system scope). With `preview`, the caller rolls
/// it back; the result says so and isn't kept on the rule.
pub async fn run(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    preview: bool,
) -> CmdbResult<SyncResult> {
    let m = get(conn, id).await?;
    let ty = types::get(conn, m.type_id).await?;
    let dataset = huxley_datasets::get(conn, m.dataset_id)
        .await
        .map_err(|e| CmdbError::invalid("datasetId", e.to_string()))?;
    let mut orgs: std::collections::HashMap<String, Result<Uuid, String>> =
        std::collections::HashMap::new();
    let mut result = SyncResult {
        preview,
        ..SyncResult::default()
    };
    let mut offset = 0;
    while i64::try_from(result.rows).unwrap_or(i64::MAX) < MAX_ROWS {
        let page = huxley_datasets::query(
            conn,
            &dataset,
            &huxley_datasets::Query {
                order_by: None,
                limit: Some(huxley_datasets::MAX_QUERY_ROWS),
                offset: Some(offset),
                ..huxley_datasets::Query::default()
            },
        )
        .await
        .map_err(|e| CmdbError::invalid("datasetId", e.to_string()))?;
        if page.rows.is_empty() {
            break;
        }
        offset += i64::try_from(page.rows.len()).unwrap_or(0);
        for row in page.rows {
            result.rows += 1;
            let n = result.rows;
            let Some(row) = row.as_object() else { continue };
            let Some(written_for) = row
                .get("_org_id")
                .and_then(Json::as_str)
                .and_then(|s| s.parse::<Uuid>().ok())
            else {
                result.skip(n, "the row has no organization");
                continue;
            };
            let org = match &m.org_match {
                None => written_for,
                Some(om) => match matched_org(conn, &dataset, om, row, &mut orgs).await? {
                    Ok(org) => org,
                    Err(why) => {
                        result.skip(n, why);
                        continue;
                    }
                },
            };
            sync_row(conn, ctx, &m, &ty, org, row, n, &mut result).await?;
        }
    }
    if !preview {
        sqlx::query("UPDATE cmdb_mappings SET last_run_at = now(), last_result = $2 WHERE id = $1")
            .bind(id)
            .bind(sqlx::types::Json(&result))
            .execute(&mut *conn)
            .await?;
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("cmdb.mapping_run").target("asset_mapping", id).details(json!({
                "name": m.name, "created": result.created, "updated": result.updated, "skipped": result.skipped,
            })),
        )
        .await?;
    }
    Ok(result)
}

/// The organization at or below the dataset's whose variable matches the
/// row's column (cached per value for the run).
async fn matched_org(
    conn: &mut PgConnection,
    dataset: &huxley_datasets::Dataset,
    om: &OrgMatch,
    row: &Map<String, Json>,
    cache: &mut std::collections::HashMap<String, Result<Uuid, String>>,
) -> CmdbResult<Result<Uuid, String>> {
    let Some(value) = as_text(row.get(&om.column)) else {
        return Ok(Err(format!("the row has no {}", om.column)));
    };
    if let Some(hit) = cache.get(&value) {
        return Ok(hit.clone());
    }
    let found: Vec<Uuid> = sqlx::query_scalar(
        "SELECT v.org_id FROM org_variables v JOIN organizations o ON o.id = v.org_id \
          WHERE v.key = $1 AND v.type <> 'secret' AND v.value #>> '{}' = $2 \
            AND o.path <@ (SELECT path FROM organizations WHERE id = $3) \
          LIMIT 2",
    )
    .bind(&om.variable)
    .bind(&value)
    .bind(dataset.org_id)
    .fetch_all(&mut *conn)
    .await?;
    let hit = match found.as_slice() {
        [org] => Ok(*org),
        [] => Err(format!("no organization has {} = {value}", om.variable)),
        _ => Err(format!(
            "more than one organization has {} = {value}",
            om.variable
        )),
    };
    cache.insert(value, hit.clone());
    Ok(hit)
}

#[allow(clippy::too_many_arguments)]
async fn sync_row(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    m: &Mapping,
    ty: &types::AssetType,
    org: Uuid,
    row: &Map<String, Json>,
    n: usize,
    result: &mut SyncResult,
) -> CmdbResult<()> {
    let incoming = values(m, row);
    let external = external_column(m).and_then(|c| as_text(row.get(c)));
    match find(conn, m, org, row).await? {
        Err(why) => result.skip(n, why),
        Ok(Some(id)) => {
            let asset = assets::get(conn, id).await?;
            match apply_row(conn, ctx, m, &asset, &incoming, external.as_deref()).await? {
                Ok(true) => result.updated += 1,
                Ok(false) => result.unchanged += 1,
                Err(why) => result.skip(n, why),
            }
        }
        Ok(None) if m.create_missing => {
            match create_asset(conn, ctx, m, ty, org, &incoming, external.as_deref()).await? {
                Ok(()) => result.created += 1,
                Err(why) => result.skip(n, why),
            }
        }
        Ok(None) => result.unchanged += 1,
    }
    Ok(())
}

/// Applies a row to an existing asset: `Ok(true)` if anything changed.
async fn apply_row(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    m: &Mapping,
    asset: &Asset,
    incoming: &Map<String, Json>,
    external: Option<&str>,
) -> CmdbResult<Result<bool, String>> {
    let current = assets::values_of(asset);
    let ty = types::get(conn, asset.type_id).await?;
    let mut sources = asset.field_sources.0.clone();
    let mut set = Map::new();
    for (field, value) in incoming {
        // Attributes the asset's own type doesn't have are left alone.
        if let Some(key) = field.strip_prefix("attributes.")
            && !ty.fields.iter().any(|f| f.key == key)
        {
            continue;
        }
        let manual = sources.get(field).and_then(Json::as_str) == Some("manual");
        let has_value = current
            .get(field)
            .is_some_and(|v| !(v.is_null() || v.as_str() == Some("")));
        if m.conflict == "manual_wins" && manual && has_value {
            continue;
        }
        if current.get(field) != Some(value) {
            set.insert(field.clone(), value.clone());
        }
        sources.insert(field.clone(), json!(m.source));
    }
    let mut external_ids = asset.external_ids.0.clone();
    let external_changed =
        external.is_some_and(|e| external_ids.get(&m.source).and_then(Json::as_str) != Some(e));
    if let Some(e) = external {
        external_ids.insert(m.source.clone(), json!(e));
    }
    if set.is_empty() && !external_changed {
        sqlx::query("UPDATE assets SET synced_at = now(), field_sources = $2 WHERE id = $1")
            .bind(asset.id)
            .bind(sqlx::types::Json(&sources))
            .execute(&mut *conn)
            .await?;
        return Ok(Ok(false));
    }
    let text_of = |field: &str, old: &str| {
        set.get(field)
            .and_then(Json::as_str)
            .map_or_else(|| old.to_owned(), str::to_owned)
    };
    let mut attributes = asset.attributes.0.clone();
    for (field, value) in &set {
        if let Some(key) = field.strip_prefix("attributes.") {
            attributes.insert(key.to_owned(), value.clone());
        }
    }
    let attributes = match ty.check(&attributes) {
        Ok(a) => a,
        Err(e) => return Ok(Err(e.to_string())),
    };
    sqlx::query(
        "UPDATE assets SET name = $2, serial_number = $3, hostname = $4, manufacturer = $5, model = $6, attributes = $7, \
                external_ids = $8, field_sources = $9, synced_at = now(), updated_at = now(), version = version + 1 \
          WHERE id = $1",
    )
    .bind(asset.id)
    .bind(text_of("name", &asset.name))
    .bind(text_of("serial_number", &asset.serial_number))
    .bind(text_of("hostname", &asset.hostname))
    .bind(text_of("manufacturer", &asset.manufacturer))
    .bind(text_of("model", &asset.model))
    .bind(sqlx::types::Json(&attributes))
    .bind(sqlx::types::Json(&external_ids))
    .bind(sqlx::types::Json(&sources))
    .execute(&mut *conn)
    .await?;
    let after = assets::get(conn, asset.id).await?;
    let fields: Vec<&String> = set.keys().collect();
    assets::record(
        conn,
        ctx,
        &after,
        "asset.updated",
        json!({ "source": m.source, "fields": fields }),
    )
    .await?;
    Ok(Ok(true))
}

/// Makes an asset of the rule's type from a row.
async fn create_asset(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    m: &Mapping,
    ty: &types::AssetType,
    org: Uuid,
    incoming: &Map<String, Json>,
    external: Option<&str>,
) -> CmdbResult<Result<(), String>> {
    let text_of = |f: &str| {
        incoming
            .get(f)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let name = ["name", "hostname", "serial_number"]
        .iter()
        .map(|f| text_of(f))
        .find(|v| !v.is_empty());
    let Some(name) = name else {
        return Ok(Err(
            "no name, hostname or serial number to name a new asset by".into(),
        ));
    };
    let attributes: Map<String, Json> = incoming
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("attributes.")
                .map(|k| (k.to_owned(), v.clone()))
        })
        .collect();
    let attributes = match ty.check(&attributes) {
        Ok(a) => a,
        Err(e) => return Ok(Err(e.to_string())),
    };
    let mut sources: Map<String, Json> = incoming
        .keys()
        .map(|k| (k.clone(), json!(m.source)))
        .collect();
    sources
        .entry("name".to_owned())
        .or_insert_with(|| json!(m.source));
    let mut external_ids = Map::new();
    if let Some(e) = external {
        external_ids.insert(m.source.clone(), json!(e));
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO assets (id, org_id, type_id, name, serial_number, hostname, manufacturer, model, attributes, \
                external_ids, field_sources, synced_at, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now(), $12, $12)",
    )
    .bind(id)
    .bind(org)
    .bind(m.type_id)
    .bind(&name)
    .bind(text_of("serial_number"))
    .bind(text_of("hostname"))
    .bind(text_of("manufacturer"))
    .bind(text_of("model"))
    .bind(sqlx::types::Json(&attributes))
    .bind(sqlx::types::Json(&external_ids))
    .bind(sqlx::types::Json(&sources))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    let asset = assets::get(conn, id).await?;
    assets::record(
        conn,
        ctx,
        &asset,
        "asset.created",
        json!({ "source": m.source }),
    )
    .await?;
    Ok(Ok(()))
}

/// Announces a run's result (for workflows), outside previews.
pub async fn announce(conn: &mut PgConnection, m: &Mapping, result: &SyncResult) -> CmdbResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "asset.sync_completed",
            org_id: None,
            subject_type: "asset_mapping",
            subject_id: m.id,
            payload: json!({ "name": m.name, "source": m.source, "result": result }),
        },
    )
    .await?;
    Ok(())
}
