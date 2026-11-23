//! Custom fields: definitions per entity type, and validated values.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{EntityError, EntityResult, entity_type, owning_org};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "custom_field_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    LongText,
    Number,
    Boolean,
    Date,
    Select,
    MultiSelect,
    Url,
    Email,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FieldDef {
    pub id: Uuid,
    pub entity_type: String,
    pub key: String,
    pub label: String,
    pub help: String,
    pub field_type: FieldType,
    #[schema(value_type = Vec<SelectOption>)]
    pub options: sqlx::types::Json<Vec<SelectOption>>,
    pub required: bool,
    pub scope_org_id: Option<Uuid>,
    pub position: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<OffsetDateTime>,
}

const SELECT_DEFS: &str = "SELECT id, entity_type, key, label, help, field_type, options, required, \
    scope_org_id, position, archived_at FROM custom_field_defs";

/// Settings for a new field. `key` is optional (derived from the label).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewField {
    pub entity_type: String,
    pub key: Option<String>,
    pub label: String,
    #[serde(default)]
    pub help: String,
    pub field_type: FieldType,
    #[serde(default)]
    pub options: Vec<SelectOption>,
    #[serde(default)]
    pub required: bool,
    pub scope_org_id: Option<Uuid>,
    #[serde(default)]
    pub position: i32,
}

/// Changes to a field. The key and type never change (stored values depend
/// on them).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldChanges {
    pub label: Option<String>,
    pub help: Option<String>,
    pub options: Option<Vec<SelectOption>>,
    pub required: Option<bool>,
    /// Limit the field to this org's subtree.
    pub scope_org_id: Option<Uuid>,
    /// Apply the field to every org again (removes the scope).
    #[serde(default)]
    pub clear_scope: bool,
    pub position: Option<i32>,
}

/// A field that applies to an entity, with its value (null when unset).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldValue {
    #[serde(flatten)]
    pub def: FieldDef,
    #[schema(value_type = Object)]
    pub value: Value,
}

fn slugify(label: &str) -> String {
    let mut key = String::new();
    for c in label.trim().to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c);
        } else if !key.ends_with('_') && !key.is_empty() {
            key.push('_');
        }
    }
    let key = key.trim_end_matches('_').to_owned();
    match key.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => key.chars().take(63).collect(),
        _ => format!("f_{key}").chars().take(63).collect(),
    }
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && key.len() <= 63
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn check_label(label: &str) -> EntityResult<String> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 100 {
        return Err(EntityError::invalid(
            "label",
            "Enter a label of 1 to 100 characters.",
        ));
    }
    Ok(label.to_owned())
}

fn check_options(
    field_type: FieldType,
    options: &[SelectOption],
) -> EntityResult<Vec<SelectOption>> {
    let is_select = matches!(field_type, FieldType::Select | FieldType::MultiSelect);
    if !is_select {
        return Ok(Vec::new());
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for o in options {
        let value = o.value.trim().to_owned();
        let label = if o.label.trim().is_empty() {
            value.clone()
        } else {
            o.label.trim().to_owned()
        };
        if value.is_empty() || value.len() > 100 || label.chars().count() > 100 {
            return Err(EntityError::invalid(
                "options",
                "Each option needs a value of 1 to 100 characters.",
            ));
        }
        if !seen.insert(value.clone()) {
            return Err(EntityError::invalid(
                "options",
                format!("Option `{value}` is listed twice."),
            ));
        }
        out.push(SelectOption { value, label });
    }
    if out.is_empty() {
        return Err(EntityError::invalid(
            "options",
            "A select field needs at least one option.",
        ));
    }
    Ok(out)
}

pub async fn list_defs(
    conn: &mut PgConnection,
    entity_type: &str,
    include_archived: bool,
) -> EntityResult<Vec<FieldDef>> {
    self::entity_type(entity_type)?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_DEFS} WHERE entity_type = $1 AND ($2 OR archived_at IS NULL) ORDER BY position, lower(label)"
    )))
    .bind(entity_type)
    .bind(include_archived)
    .fetch_all(conn)
    .await?)
}

pub async fn get_def(conn: &mut PgConnection, id: Uuid) -> EntityResult<FieldDef> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT_DEFS} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(EntityError::NotFound)
}

/// Creates a field. Run in system scope after checking `custom_field:manage`.
pub async fn create_def(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    new: &NewField,
) -> EntityResult<FieldDef> {
    entity_type(&new.entity_type)?;
    let label = check_label(&new.label)?;
    let key = new
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map_or_else(|| slugify(&label), str::to_owned);
    if !valid_key(&key) {
        return Err(EntityError::invalid(
            "key",
            "Use a lowercase letter, then letters, digits, or underscores.",
        ));
    }
    if new.help.chars().count() > 500 {
        return Err(EntityError::invalid(
            "help",
            "Keep help text under 500 characters.",
        ));
    }
    let options = check_options(new.field_type, &new.options)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO custom_field_defs (id, entity_type, key, label, help, field_type, options, required, scope_org_id, position) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(&new.entity_type)
    .bind(&key)
    .bind(&label)
    .bind(new.help.trim())
    .bind(new.field_type)
    .bind(sqlx::types::Json(&options))
    .bind(new.required)
    .bind(new.scope_org_id)
    .bind(new.position)
    .execute(&mut *conn)
    .await
    .map_err(|e| match huxley_db::violated_constraint(&e) {
        Some("custom_field_defs_key") => EntityError::KeyTaken,
        Some("custom_field_defs_scope_org_id_fkey") => EntityError::invalid("scopeOrgId", "Unknown organization."),
        _ => e.into(),
    })?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("custom_field.created")
            .target("custom_field", id)
            .details(json!({ "entityType": new.entity_type, "key": key, "type": new.field_type })),
    )
    .await
    .map_err(|e| EntityError::Db(db_err(e)))?;
    get_def(conn, id).await
}

fn db_err(e: huxley_db::DbError) -> sqlx::Error {
    match e {
        huxley_db::DbError::Sqlx(e) => e,
        other => sqlx::Error::Protocol(other.to_string()),
    }
}

pub async fn update_def(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    changes: &FieldChanges,
) -> EntityResult<FieldDef> {
    let before = get_def(conn, id).await?;
    let label = changes.label.as_deref().map(check_label).transpose()?;
    if changes
        .help
        .as_ref()
        .is_some_and(|h| h.chars().count() > 500)
    {
        return Err(EntityError::invalid(
            "help",
            "Keep help text under 500 characters.",
        ));
    }
    let options = changes
        .options
        .as_ref()
        .map(|o| check_options(before.field_type, o))
        .transpose()?;
    let scope = if changes.clear_scope {
        None
    } else {
        changes.scope_org_id.or(before.scope_org_id)
    };
    sqlx::query(
        "UPDATE custom_field_defs SET label = coalesce($2, label), help = coalesce($3, help), \
                options = coalesce($4, options), required = coalesce($5, required), scope_org_id = $6, \
                position = coalesce($7, position), version = version + 1 \
          WHERE id = $1",
    )
    .bind(id)
    .bind(label)
    .bind(changes.help.as_deref().map(str::trim))
    .bind(options.map(sqlx::types::Json))
    .bind(changes.required)
    .bind(scope)
    .bind(changes.position)
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("custom_field.updated")
            .target("custom_field", id)
            .details(json!({ "key": before.key })),
    )
    .await
    .map_err(|e| EntityError::Db(db_err(e)))?;
    get_def(conn, id).await
}

/// Archives a field: it disappears from entities, and its stored values are
/// kept (unarchiving brings them back). Its key is never reused.
pub async fn set_archived(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    archived: bool,
) -> EntityResult<FieldDef> {
    let def = get_def(conn, id).await?;
    sqlx::query(
        "UPDATE custom_field_defs SET archived_at = CASE WHEN $2 THEN now() END WHERE id = $1",
    )
    .bind(id)
    .bind(archived)
    .execute(&mut *conn)
    .await?;
    let action = if archived {
        "custom_field.archived"
    } else {
        "custom_field.restored"
    };
    audit::record(
        conn,
        ctx,
        AuditRecord::new(action)
            .target("custom_field", id)
            .details(json!({ "key": def.key })),
    )
    .await
    .map_err(|e| EntityError::Db(db_err(e)))?;
    get_def(conn, id).await
}

/// Active fields that apply to an entity owned by the org at `org_path`.
async fn applicable(
    conn: &mut PgConnection,
    entity_type: &str,
    org_path: &str,
) -> EntityResult<Vec<FieldDef>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_DEFS} d WHERE d.entity_type = $1 AND d.archived_at IS NULL \
           AND (d.scope_org_id IS NULL OR $2::ltree <@ (SELECT path FROM organizations WHERE id = d.scope_org_id)) \
         ORDER BY d.position, lower(d.label)"
    )))
    .bind(entity_type)
    .bind(org_path)
    .fetch_all(conn)
    .await?)
}

async fn stored(
    conn: &mut PgConnection,
    entity_type: &str,
    id: Uuid,
) -> EntityResult<Map<String, Value>> {
    let table = crate::entity_type(entity_type)?.table;
    let (value,): (Value,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT custom_fields FROM {table} WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(EntityError::NotFound)?;
    Ok(value.as_object().cloned().unwrap_or_default())
}

/// The fields that apply to an entity, with values.
pub async fn values(
    conn: &mut PgConnection,
    entity_type: &str,
    id: Uuid,
) -> EntityResult<Vec<FieldValue>> {
    let (_, path) = owning_org(conn, entity_type, id).await?;
    let defs = applicable(conn, entity_type, &path).await?;
    let stored = stored(conn, entity_type, id).await?;
    Ok(defs
        .into_iter()
        .map(|def| FieldValue {
            value: stored.get(&def.key).cloned().unwrap_or(Value::Null),
            def,
        })
        .collect())
}

/// Checks and normalizes a value for a field. `Null` clears it.
pub fn validate(def: &FieldDef, value: &Value) -> EntityResult<Value> {
    let bad = |m: &str| EntityError::invalid(def.key.clone(), m.to_owned());
    if value.is_null() {
        return if def.required {
            Err(bad("This field is required."))
        } else {
            Ok(Value::Null)
        };
    }
    let text = |max: usize| -> EntityResult<Value> {
        let s = value.as_str().ok_or_else(|| bad("Expected text."))?.trim();
        if s.chars().count() > max {
            return Err(bad(&format!("Keep it under {max} characters.")));
        }
        if s.is_empty() {
            return if def.required {
                Err(bad("This field is required."))
            } else {
                Ok(Value::Null)
            };
        }
        Ok(Value::String(s.to_owned()))
    };
    match def.field_type {
        FieldType::Text => text(1_000),
        FieldType::LongText => text(20_000),
        FieldType::Number => value
            .as_f64()
            .filter(|n| n.is_finite())
            .map(|_| value.clone())
            .ok_or_else(|| bad("Expected a number.")),
        FieldType::Boolean => value
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| bad("Expected true or false.")),
        FieldType::Date => {
            let s = value
                .as_str()
                .ok_or_else(|| bad("Expected a date (YYYY-MM-DD)."))?;
            let format = time::macros::format_description!("[year]-[month]-[day]");
            time::Date::parse(s, &format).map_err(|_| bad("Expected a date (YYYY-MM-DD)."))?;
            Ok(Value::String(s.to_owned()))
        }
        FieldType::Select => {
            let s = value
                .as_str()
                .ok_or_else(|| bad("Expected one of the options."))?;
            if def.options.iter().any(|o| o.value == s) {
                Ok(Value::String(s.to_owned()))
            } else {
                Err(bad("Choose one of the options."))
            }
        }
        FieldType::MultiSelect => validate_multi(def, value),
        FieldType::Url => {
            let s = value
                .as_str()
                .ok_or_else(|| bad("Expected a web address."))?
                .trim();
            match url::Url::parse(s) {
                Ok(u) if matches!(u.scheme(), "http" | "https") && s.len() <= 2_000 => {
                    Ok(Value::String(s.to_owned()))
                }
                _ => Err(bad("Enter an http or https address.")),
            }
        }
        FieldType::Email => {
            let s = value
                .as_str()
                .ok_or_else(|| bad("Expected an email address."))?
                .trim();
            let ok = s.len() <= 320
                && !s.chars().any(char::is_whitespace)
                && s.split_once('@')
                    .is_some_and(|(l, d)| !l.is_empty() && d.contains('.'));
            if ok {
                Ok(Value::String(s.to_owned()))
            } else {
                Err(bad("Enter a valid email address."))
            }
        }
    }
}

fn validate_multi(def: &FieldDef, value: &Value) -> EntityResult<Value> {
    let bad = |m: &str| EntityError::invalid(def.key.clone(), m.to_owned());
    let items = value
        .as_array()
        .ok_or_else(|| bad("Expected a list of options."))?;
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let s = item
            .as_str()
            .ok_or_else(|| bad("Expected a list of options."))?;
        if !def.options.iter().any(|o| o.value == s) {
            return Err(bad(&format!("`{s}` isn't one of the options.")));
        }
        if !out.iter().any(|v| v == s) {
            out.push(s.to_owned());
        }
    }
    if out.is_empty() {
        return if def.required {
            Err(bad("This field is required."))
        } else {
            Ok(Value::Null)
        };
    }
    Ok(json!(out))
}

/// Sets values on an entity by field key (`null` clears). Unknown, archived,
/// or out-of-scope keys are refused. Run in system scope after checking the
/// entity's update permission. Returns the updated values.
pub async fn set_values(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    entity_type: &str,
    id: Uuid,
    changes: &Map<String, Value>,
) -> EntityResult<Vec<FieldValue>> {
    let ty = crate::entity_type(entity_type)?;
    let (org_id, path) = owning_org(conn, entity_type, id).await?;
    let defs = applicable(conn, entity_type, &path).await?;
    let mut stored = stored(conn, entity_type, id).await?;
    let mut changed_keys = Vec::new();
    for (key, value) in changes {
        let def = defs
            .iter()
            .find(|d| &d.key == key)
            .ok_or_else(|| EntityError::invalid(key.clone(), "No such field for this record."))?;
        let value = validate(def, value)?;
        // A missing key and null are the same thing.
        if stored.get(key).unwrap_or(&Value::Null) != &value {
            changed_keys.push(key.clone());
        }
        if value.is_null() {
            stored.remove(key);
        } else {
            stored.insert(key.clone(), value);
        }
    }
    if !changed_keys.is_empty() {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {} SET custom_fields = $2 WHERE id = $1",
            ty.table
        )))
        .bind(id)
        .bind(Value::Object(stored))
        .execute(&mut *conn)
        .await?;
        audit::record(
            conn,
            ctx,
            AuditRecord::new("entity.custom_fields_updated")
                .org(org_id)
                .target(ty.key, id)
                .details(json!({ "fields": changed_keys })),
        )
        .await
        .map_err(|e| EntityError::Db(db_err(e)))?;
        huxley_db::outbox::emit(
            conn,
            huxley_db::outbox::DomainEvent {
                event_type: &format!("{}.custom_fields_changed", ty.key),
                org_id: Some(org_id),
                subject_type: ty.key,
                subject_id: id,
                payload: json!({ "fields": changed_keys }),
            },
        )
        .await
        .map_err(|e| EntityError::Db(db_err(e)))?;
    }
    values(conn, entity_type, id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(field_type: FieldType, required: bool) -> FieldDef {
        FieldDef {
            id: Uuid::nil(),
            entity_type: "organization".into(),
            key: "f".into(),
            label: "F".into(),
            help: String::new(),
            field_type,
            options: sqlx::types::Json(vec![
                SelectOption {
                    value: "a".into(),
                    label: "A".into(),
                },
                SelectOption {
                    value: "b".into(),
                    label: "B".into(),
                },
            ]),
            required,
            scope_org_id: None,
            position: 0,
            archived_at: None,
        }
    }

    #[test]
    fn values_are_validated_by_type() {
        assert_eq!(
            validate(&def(FieldType::Text, false), &json!("  hi ")).unwrap(),
            json!("hi")
        );
        assert_eq!(
            validate(&def(FieldType::Text, false), &json!("  ")).unwrap(),
            Value::Null
        );
        assert!(validate(&def(FieldType::Text, true), &json!(" ")).is_err());
        assert!(validate(&def(FieldType::Text, true), &Value::Null).is_err());
        assert!(validate(&def(FieldType::Number, false), &json!("3")).is_err());
        assert_eq!(
            validate(&def(FieldType::Number, false), &json!(3.5)).unwrap(),
            json!(3.5)
        );
        assert!(validate(&def(FieldType::Boolean, false), &json!(1)).is_err());
        assert!(validate(&def(FieldType::Date, false), &json!("2026-02-30")).is_err());
        assert_eq!(
            validate(&def(FieldType::Date, false), &json!("2026-02-28")).unwrap(),
            json!("2026-02-28")
        );
        assert!(validate(&def(FieldType::Select, false), &json!("z")).is_err());
        assert_eq!(
            validate(&def(FieldType::MultiSelect, false), &json!(["b", "a", "b"])).unwrap(),
            json!(["b", "a"])
        );
        assert!(validate(&def(FieldType::MultiSelect, false), &json!(["z"])).is_err());
        assert!(validate(&def(FieldType::Url, false), &json!("javascript:alert(1)")).is_err());
        assert!(validate(&def(FieldType::Url, false), &json!("https://example.com/x")).is_ok());
        assert!(validate(&def(FieldType::Email, false), &json!("not an email")).is_err());
    }

    #[test]
    fn keys_are_derived_from_labels() {
        assert_eq!(slugify("Contract Renewal Date"), "contract_renewal_date");
        assert_eq!(slugify("  2FA enforced? "), "f_2fa_enforced");
        assert!(valid_key(&slugify("Ünïcode Name")));
        assert!(!valid_key("Bad-Key"));
    }
}
