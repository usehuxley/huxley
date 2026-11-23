//! Forms (design doc §6.8): a JSON Schema of answers, with UI hints, that
//! people fill in. Submitting one starts the published workflows in the
//! form's organization that have a `form` trigger for it; the answers are
//! their inputs, and the runs happen in the organization the form was
//! submitted for.
//!
//! Supported fields (the renderer's subset): strings (with `enum` for a
//! choice, `format: date | date-time | email`, `maxLength`), integers and
//! numbers, booleans, and arrays of `enum` strings (several choices). UI
//! hints per field: `widget: textarea`, `help`, `placeholder`,
//! `show_if` (shown only for certain answers) and `options_from` (choices
//! looked up through a read-only integration action); and `order`, the
//! fields' order.

mod hints;

pub use hints::{Choice, OptionsFrom, ShowIf, choices_of, options, shown};

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_engine::{Engine, StartRun};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

/// Most fields one form may have.
pub const MAX_FIELDS: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum FormError {
    #[error("form not found")]
    NotFound,
    #[error("another form in this organization already has the key {0:?}")]
    KeyTaken(String),
    /// The definition is wrong: `(where, what)`.
    #[error("{1}")]
    InvalidDefinition(String, String),
    /// The answers don't fit: one problem per field, as `(pointer, message)`.
    #[error("the answers don't fit the form")]
    InvalidAnswers(Vec<(String, String)>),
    #[error("the form is archived")]
    Archived,
    /// Looking up a field's choices failed.
    #[error("couldn't look up the choices: {0}")]
    Lookup(String),
    #[error(transparent)]
    Engine(#[from] huxley_engine::EngineError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type FormResult<T> = Result<T, FormError>;

fn db(e: huxley_db::DbError) -> FormError {
    match e {
        huxley_db::DbError::Sqlx(e) => FormError::Db(e),
        other => FormError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Form {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema of the answers.
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub schema: Json,
    /// UI hints: `order`, and per field `widget`, `help`, `placeholder`.
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub ui: Json,
    pub archived: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

const COLUMNS: &str =
    "id, org_id, key, name, description, schema, ui, archived, created_at, updated_at";

/// A form's definition, to create or replace it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormInput {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub schema: Json,
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub ui: Json,
}

fn invalid(at: impl Into<String>, message: impl Into<String>) -> FormError {
    FormError::InvalidDefinition(at.into(), message.into())
}

fn valid_key(key: &str) -> bool {
    key.len() <= 64
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Checks a definition: an object schema of supported fields that compiles.
pub fn check(input: &FormInput) -> FormResult<()> {
    if !valid_key(&input.key) {
        return Err(invalid(
            "key",
            "lowercase letters, digits and hyphens, starting with a letter",
        ));
    }
    if input.name.trim().is_empty() || input.name.chars().count() > 200 {
        return Err(invalid("name", "1 to 200 characters"));
    }
    let schema = &input.schema;
    if schema["type"] != "object" {
        return Err(invalid(
            "schema.type",
            "a form's schema is an object (type: object)",
        ));
    }
    let Some(properties) = schema["properties"].as_object() else {
        return Err(invalid(
            "schema.properties",
            "list the form's fields under properties",
        ));
    };
    if properties.len() > MAX_FIELDS {
        return Err(invalid(
            "schema.properties",
            format!("at most {MAX_FIELDS} fields"),
        ));
    }
    for (name, field) in properties {
        let at = format!("schema.properties.{name}");
        let supported = match field["type"].as_str() {
            Some("string" | "integer" | "number" | "boolean") => true,
            Some("array") => {
                field["items"]["type"] == "string"
                    && (field["items"]["enum"].is_array()
                        || hints::options_from(&input.ui, name).is_some())
            }
            _ => false,
        };
        if !supported {
            return Err(invalid(
                at,
                "fields are strings, numbers, integers, booleans, or arrays of choices",
            ));
        }
    }
    if let Err(e) = jsonschema::validator_for(schema) {
        return Err(invalid("schema", format!("not a valid JSON Schema: {e}")));
    }
    if !input.ui.is_null() && !input.ui.is_object() {
        return Err(invalid("ui", "an object of hints"));
    }
    hints::check(schema, &input.ui)
}

fn ui(input: &FormInput) -> Json {
    if input.ui.is_null() {
        json!({})
    } else {
        input.ui.clone()
    }
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &FormInput,
) -> FormResult<Form> {
    check(input)?;
    let id = huxley_core::new_id();
    let inserted = sqlx::query(
        "INSERT INTO forms (id, org_id, key, name, description, schema, ui, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (org_id, key) DO NOTHING",
    )
    .bind(id)
    .bind(org_id)
    .bind(&input.key)
    .bind(input.name.trim())
    .bind(&input.description)
    .bind(&input.schema)
    .bind(ui(input))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(FormError::KeyTaken(input.key.clone()));
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("form.created")
            .org(org_id)
            .target("form", id)
            .details(json!({ "key": input.key })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

/// Replaces a form's definition. Its key can't change (workflows name it).
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &FormInput,
) -> FormResult<Form> {
    let current = get(&mut *conn, id).await?;
    if input.key != current.key {
        return Err(invalid("key", "a form's key can't change"));
    }
    check(input)?;
    sqlx::query(
        "UPDATE forms SET name = $2, description = $3, schema = $4, ui = $5, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(input.name.trim())
    .bind(&input.description)
    .bind(&input.schema)
    .bind(ui(input))
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("form.updated")
            .org(current.org_id)
            .target("form", id),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

pub async fn set_archived(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    archived: bool,
) -> FormResult<Form> {
    let form = get(&mut *conn, id).await?;
    sqlx::query("UPDATE forms SET archived = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(archived)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(if archived {
            "form.archived"
        } else {
            "form.restored"
        })
        .org(form.org_id)
        .target("form", id),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> FormResult<Form> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM forms WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(FormError::NotFound)
}

/// Forms in the caller's scope, by name.
pub async fn list(conn: &mut PgConnection) -> FormResult<Vec<Form>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM forms ORDER BY archived, lower(name), id"
    )))
    .fetch_all(conn)
    .await?)
}

/// Checks answers against a form: one problem per field, with a pointer.
/// Fields that aren't shown (`show_if`) aren't required, and their answers
/// are dropped: returns the answers that count.
pub fn check_answers(form: &Form, answers: &Json) -> FormResult<Json> {
    let (kept, hidden) = hints::shown(form, answers);
    let schema = hints::schema_for(form, &hidden);
    let Ok(validator) = jsonschema::validator_for(&schema) else {
        return Err(invalid("schema", "the form's schema doesn't compile"));
    };
    let problems: Vec<(String, String)> = validator
        .iter_errors(&kept)
        .map(|e| (e.instance_path().to_string(), e.to_string()))
        .collect();
    if problems.is_empty() {
        Ok(kept)
    } else {
        Err(FormError::InvalidAnswers(problems))
    }
}

/// Checks that answers to fields with looked-up choices are among them
/// (looking them up again, for `org_id`). Call before [`submit`], outside
/// a transaction.
pub async fn check_choices(
    engine: &Engine,
    form: &Form,
    answers: &Json,
    org_id: Uuid,
) -> FormResult<()> {
    let (kept, _) = hints::shown(form, answers);
    let mut problems = Vec::new();
    for (name, answer) in kept.as_object().into_iter().flatten() {
        if hints::options_from(&form.ui, name).is_none() {
            continue;
        }
        let choices = hints::options(engine, form, name, org_id).await?;
        let given: Vec<&Json> = match answer {
            Json::Array(items) => items.iter().collect(),
            one => vec![one],
        };
        let all_known = given.iter().all(|g| {
            g.as_str()
                .is_some_and(|g| choices.iter().any(|c| c.value == g))
        });
        if !all_known {
            problems.push((format!("/{name}"), "not one of the choices".to_owned()));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(FormError::InvalidAnswers(problems))
    }
}

/// What a submission started.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Submitted {
    pub submission_id: Uuid,
    /// The runs it started, one per workflow with a trigger for the form.
    pub runs: Vec<Uuid>,
    /// Workflows that didn't start because the answers don't fit their
    /// inputs (a mismatch between the form and the workflow).
    pub skipped: Vec<String>,
}

/// Submits answers to a form for `org_id` (the form's organization or one
/// below it): starts the runs of the published workflows in the form's
/// organization that have a `form` trigger for it, acting as `user_id`.
/// The caller checks the submitter may submit there; this runs as the
/// system, since the workflows usually live above the submitter's scope.
pub async fn submit(
    conn: &mut PgConnection,
    engine: &Engine,
    ctx: &RequestContext,
    form_id: Uuid,
    org_id: Uuid,
    user_id: Option<Uuid>,
    answers: Json,
) -> FormResult<Submitted> {
    let form = get(&mut *conn, form_id).await?;
    if form.archived {
        return Err(FormError::Archived);
    }
    let answers = check_answers(&form, &answers)?;
    let workflows: Vec<(Uuid, String, i32, Json)> = sqlx::query_as(
        "SELECT w.id, w.name, w.current_version, v.document FROM workflow_triggers t \
           JOIN workflows w ON w.id = t.workflow_id \
           JOIN workflow_versions v ON v.workflow_id = w.id AND v.version = w.current_version \
          WHERE t.type = 'form' AND t.config->>'form' = $1 AND w.org_id = $2 \
            AND w.status = 'active' ORDER BY w.name",
    )
    .bind(&form.key)
    .bind(form.org_id)
    .fetch_all(&mut *conn)
    .await?;
    let submission_id = huxley_core::new_id();
    let mut runs = Vec::new();
    let mut skipped = Vec::new();
    for (workflow_id, name, version, document) in workflows {
        if let Err(e) = huxley_workflows::check_inputs(&document, &answers) {
            tracing::warn!(%workflow_id, error = %e, "form submission doesn't fit the workflow's inputs");
            skipped.push(name);
            continue;
        }
        let run = engine
            .start_run(
                &mut *conn,
                StartRun {
                    org_id,
                    workflow_id: Some(workflow_id),
                    version: Some(version),
                    document: None,
                    trigger_type: "form".into(),
                    trigger: json!({ "form": form.key, "submission": submission_id }),
                    inputs: answers.clone(),
                    // Form runs act as whoever filled the form in.
                    actor_id: user_id,
                },
            )
            .await?;
        runs.push(run);
    }
    sqlx::query(
        "INSERT INTO form_submissions (id, form_id, org_id, submitted_by, data, run_ids) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(submission_id)
    .bind(form_id)
    .bind(org_id)
    .bind(user_id)
    .bind(&answers)
    .bind(&runs)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("form.submitted")
            .org(org_id)
            .target("form", form_id)
            .details(json!({ "submission": submission_id, "runs": runs })),
    )
    .await
    .map_err(db)?;
    Ok(Submitted {
        submission_id,
        runs,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(schema: Json) -> FormInput {
        FormInput {
            key: "new-user".into(),
            name: "New user".into(),
            description: None,
            schema,
            ui: json!({}),
        }
    }

    #[test]
    fn definitions_are_checked() {
        let good = json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string", "maxLength": 100 },
                "start": { "type": "string", "format": "date" },
                "seats": { "type": "integer", "minimum": 1 },
                "licenses": { "type": "array", "items": { "type": "string", "enum": ["E3", "E5"] } },
                "manager": { "type": "boolean" }
            }
        });
        assert!(check(&input(good)).is_ok());
        let field = |e: FormError| match e {
            FormError::InvalidDefinition(at, _) => at,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            field(check(&input(json!({ "type": "string" }))).unwrap_err()),
            "schema.type"
        );
        assert_eq!(
            field(
                check(&input(
                    json!({ "type": "object", "properties": { "x": { "type": "object" } } })
                ))
                .unwrap_err()
            ),
            "schema.properties.x"
        );
        let mut bad_key = input(json!({ "type": "object", "properties": {} }));
        bad_key.key = "Bad".into();
        assert_eq!(field(check(&bad_key).unwrap_err()), "key");
    }
}
