//! Asset types: workstation, server, network device and the rest, each
//! with its own fields. System types can be extended (fields added), not
//! removed; custom types can be archived.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_entity::custom_fields::{FieldDef, FieldType, SelectOption};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CmdbError, CmdbResult, text};

/// A field of an asset type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AssetTypeField)]
pub struct TypeField {
    /// The key its value is kept under.
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    pub field_type: FieldType,
    /// For `select` and `multi_select`.
    #[serde(default)]
    pub options: Vec<SelectOption>,
    #[serde(default)]
    pub required: bool,
}

impl TypeField {
    fn def(&self) -> FieldDef {
        FieldDef {
            id: Uuid::nil(),
            entity_type: "asset".into(),
            key: self.key.clone(),
            label: self.label.clone(),
            help: self.help.clone(),
            field_type: self.field_type,
            options: sqlx::types::Json(self.options.clone()),
            required: self.required,
            scope_org_id: None,
            position: 0,
            archived_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AssetType {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    #[schema(value_type = Vec<TypeField>)]
    pub fields: sqlx::types::Json<Vec<TypeField>>,
    pub is_system: bool,
    pub position: i32,
    pub archived: bool,
    /// Assets of this type you can see.
    pub assets: i64,
    pub version: i32,
}

impl AssetType {
    /// Checks and normalizes an asset's attributes against this type's
    /// fields: unknown keys are refused, required ones must be set.
    pub fn check(&self, attributes: &Map<String, Json>) -> CmdbResult<Map<String, Json>> {
        let mut out = Map::new();
        for key in attributes.keys() {
            if !self.fields.iter().any(|f| &f.key == key) {
                return Err(CmdbError::invalid(
                    &format!("attributes/{key}"),
                    format!("{} has no such field", self.name),
                ));
            }
        }
        for field in self.fields.iter() {
            let value = attributes.get(&field.key).cloned().unwrap_or(Json::Null);
            let value = huxley_entity::custom_fields::validate(&field.def(), &value).map_err(
                |e| match e {
                    huxley_entity::EntityError::Invalid { message, .. } => {
                        CmdbError::invalid(&format!("attributes/{}", field.key), message)
                    }
                    other => CmdbError::from(other),
                },
            )?;
            if !value.is_null() {
                out.insert(field.key.clone(), value);
            }
        }
        Ok(out)
    }
}

const SELECT: &str = "SELECT t.id, t.key, t.name, t.fields, t.is_system, t.position, t.archived_at IS NOT NULL AS archived, \
    (SELECT count(*) FROM assets a WHERE a.type_id = t.id) AS assets, t.version FROM cmdb_asset_types t";

/// Every type, in order (archived ones last).
pub async fn list(conn: &mut PgConnection) -> CmdbResult<Vec<AssetType>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY t.archived_at IS NOT NULL, t.position, t.name"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> CmdbResult<AssetType> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CmdbError::NotFound)
}

/// A new type, or changes to one.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetTypeInput {
    /// Only for a new type; default: from the name.
    #[serde(default)]
    pub key: Option<String>,
    pub name: String,
    #[serde(default)]
    pub fields: Vec<TypeField>,
    #[serde(default)]
    pub position: Option<i32>,
    /// The version you read (changes only).
    #[serde(default)]
    pub version: Option<i32>,
}

fn key_from(name: &str) -> String {
    let mut key: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while key.contains("__") {
        key = key.replace("__", "_");
    }
    let key = key.trim_matches('_').to_owned();
    if key.starts_with(|c: char| c.is_ascii_lowercase()) {
        key.chars().take(63).collect()
    } else {
        format!("t_{key}").chars().take(63).collect()
    }
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    key.len() <= 63
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn check_fields(fields: &[TypeField]) -> CmdbResult<()> {
    if fields.len() > 50 {
        return Err(CmdbError::invalid("fields", "at most 50 fields"));
    }
    for (i, f) in fields.iter().enumerate() {
        if !valid_key(&f.key) {
            return Err(CmdbError::invalid(
                &format!("fields/{i}/key"),
                "lowercase letters, digits and _",
            ));
        }
        if fields[..i].iter().any(|g| g.key == f.key) {
            return Err(CmdbError::invalid(&format!("fields/{i}/key"), "used twice"));
        }
        if text("label", Some(&f.label), 100)?.is_none() {
            return Err(CmdbError::invalid(
                &format!("fields/{i}/label"),
                "a field needs a label",
            ));
        }
        if matches!(f.field_type, FieldType::Select | FieldType::MultiSelect)
            && f.options.is_empty()
        {
            return Err(CmdbError::invalid(
                &format!("fields/{i}/options"),
                "a choice needs options",
            ));
        }
    }
    Ok(())
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &AssetTypeInput,
) -> CmdbResult<AssetType> {
    let name = text("name", Some(&input.name), 100)?
        .ok_or_else(|| CmdbError::invalid("name", "a type needs a name"))?;
    let key = input
        .key
        .clone()
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| key_from(&name));
    if !valid_key(&key) {
        return Err(CmdbError::invalid(
            "key",
            "lowercase letters, digits and _, starting with a letter",
        ));
    }
    check_fields(&input.fields)?;
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO cmdb_asset_types (id, key, name, fields, position) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(&key)
        .bind(&name)
        .bind(sqlx::types::Json(&input.fields))
        .bind(input.position.unwrap_or(500))
        .execute(&mut *conn)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => CmdbError::invalid("key", "a type with this key exists"),
            _ => CmdbError::Sqlx(e),
        })?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("cmdb.type_created")
            .target("asset_type", id)
            .details(json!({ "key": key, "name": name })),
    )
    .await?;
    get(conn, id).await
}

/// Changes a type's name, fields and position. Fields of a system type
/// can be added and changed; its key never changes.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &AssetTypeInput,
) -> CmdbResult<AssetType> {
    let before = get(conn, id).await?;
    let name = text("name", Some(&input.name), 100)?
        .ok_or_else(|| CmdbError::invalid("name", "a type needs a name"))?;
    check_fields(&input.fields)?;
    let rows = sqlx::query(
        "UPDATE cmdb_asset_types SET name = $3, fields = $4, position = $5, version = version + 1 \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&name)
    .bind(sqlx::types::Json(&input.fields))
    .bind(input.position.unwrap_or(before.position))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(CmdbError::VersionConflict);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("cmdb.type_updated")
            .target("asset_type", id)
            .details(json!({ "key": before.key, "name": name })),
    )
    .await?;
    get(conn, id).await
}

/// Archives a custom type: no new assets of it. System types stay.
pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> CmdbResult<()> {
    let before = get(conn, id).await?;
    if before.is_system {
        return Err(CmdbError::invalid(
            "id",
            "a built-in type can't be archived",
        ));
    }
    sqlx::query(
        "UPDATE cmdb_asset_types SET archived_at = now() WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("cmdb.type_archived")
            .target("asset_type", id)
            .details(json!({ "key": before.key })),
    )
    .await?;
    Ok(())
}
