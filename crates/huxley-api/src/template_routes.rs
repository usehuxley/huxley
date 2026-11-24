//! `/api/v1/templates`: built-in workflow (and form) templates (design doc
//! §6.6). Using one creates a draft to review and publish.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_templates::{TemplateError, TemplateInfo};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/{key}/use", post(use_template))
        .route("/{id}", axum::routing::delete(delete_template))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    /// Also list the templates saved at this organization and above it.
    pub org_id: Option<Uuid>,
}

fn store_err(e: huxley_templates::store::StoreError) -> ApiError {
    match e {
        huxley_templates::store::StoreError::NotFound => ApiError::not_found(),
        huxley_templates::store::StoreError::Invalid(at, message) => {
            ApiError::validation().with_field_error(at, message)
        }
        huxley_templates::store::StoreError::Db(e) => ApiError::internal(&e),
    }
}

/// Your organization's templates offered to `org_id` (as the system: the
/// caller checked access to `org_id`).
async fn offered(state: &AppState, org_id: Uuid) -> ApiResult<Vec<huxley_templates::Template>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let offered = huxley_templates::store::offered(&mut tx, org_id)
        .await
        .map_err(store_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(offered)
}

/// The built-in templates, and (with `orgId`) those saved at that
/// organization and above it.
#[utoipa::path(get, path = "/api/v1/templates", operation_id = "list_templates", tag = "templates",
    params(ListQuery), responses((status = 200, body = Vec<TemplateInfo>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<TemplateInfo>>> {
    let mut all = huxley_templates::list();
    if let Some(org_id) = query.org_id {
        authed
            .require_at(&state, permissions::WORKFLOW_WRITE, org_id)
            .await?;
        all.extend(
            offered(&state, org_id)
                .await?
                .into_iter()
                .map(|t| t.template),
        );
    }
    Ok(Json(all))
}

/// Deletes one of your organization's templates.
#[utoipa::path(delete, path = "/api/v1/templates/{id}", operation_id = "delete_template", tag = "templates",
    params(("id" = Uuid, Path)), responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_template(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let template = huxley_templates::store::get(&mut tx, id)
        .await
        .map_err(store_err)?;
    tx.commit().await.map_err(db_err)?;
    let org_id = template.template.org_id.ok_or_else(ApiError::not_found)?;
    if authed
        .require_at(&state, permissions::WORKFLOW_PUBLISH, org_id)
        .await
        .is_err()
    {
        // Don't reveal templates the caller can't manage.
        return Err(ApiError::not_found());
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    huxley_templates::store::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(store_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UseTemplateRequest {
    pub org_id: Uuid,
    /// The new workflow's key and name.
    pub key: String,
    pub name: String,
    /// The new form's key and name, if the template has one (default: the
    /// workflow's key and the template's form name).
    #[serde(default)]
    pub form_key: Option<String>,
    #[serde(default)]
    pub form_name: Option<String>,
    /// Values for the template's bindings (missing ones take defaults).
    #[serde(default)]
    pub bindings: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsedTemplate {
    /// The new draft workflow.
    pub workflow_id: Uuid,
    pub form_id: Option<Uuid>,
}

/// Creates the template's form (if any) and a draft workflow from it.
#[utoipa::path(post, path = "/api/v1/templates/{key}/use", operation_id = "use_template", tag = "templates",
    params(("key" = String, Path)), request_body = UseTemplateRequest,
    responses((status = 201, body = UsedTemplate),
              (status = 404, body = ApiError, content_type = "application/problem+json"),
              (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn use_template(
    State(state): State<AppState>,
    authed: Authed,
    Path(key): Path<String>,
    Json(body): Json<UseTemplateRequest>,
) -> ApiResult<(StatusCode, Json<UsedTemplate>)> {
    authed
        .require_at(&state, permissions::WORKFLOW_WRITE, body.org_id)
        .await?;
    // A built-in by key, or one of your organization's by id (offered
    // to this organization: saved here or above).
    let source = match key.parse::<Uuid>() {
        Ok(id) => offered(&state, body.org_id)
            .await?
            .into_iter()
            .find(|t| t.template.key == id.to_string())
            .ok_or_else(ApiError::not_found)?,
        Err(_) => huxley_templates::builtin(&key).ok_or_else(ApiError::not_found)?,
    };
    let info = source.template.clone();
    if info.has_form {
        authed
            .require_at(&state, permissions::FORM_MANAGE, body.org_id)
            .await?;
    }
    let form_key = info
        .has_form
        .then(|| body.form_key.clone().unwrap_or_else(|| body.key.clone()));
    let resolved = huxley_templates::resolve_template(
        &source,
        &body.bindings,
        &body.key,
        &body.name,
        form_key.as_deref(),
    )
    .map_err(|e| match e {
        TemplateError::NotFound => ApiError::not_found(),
        TemplateError::Binding(at, message) => {
            ApiError::validation().with_field_error(format!("bindings.{at}"), message)
        }
    })?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let form_id = match (resolved.form, form_key) {
        (Some(form), Some(form_key)) => {
            let input = huxley_forms::FormInput {
                key: form_key,
                name: body.form_name.clone().unwrap_or(form.name),
                description: None,
                schema: form.schema,
                ui: form.ui,
            };
            let created = huxley_forms::create(&mut tx, &authed.user.ctx, body.org_id, &input)
                .await
                .map_err(crate::form_routes::map_err)?;
            Some(created.id)
        }
        _ => None,
    };
    let workflow = huxley_workflows::create(
        &mut tx,
        &authed.user.ctx,
        state.engine.expr(),
        body.org_id,
        resolved.workflow,
    )
    .await
    .map_err(crate::workflow_routes::map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        Json(UsedTemplate {
            workflow_id: workflow.summary.id,
            form_id,
        }),
    ))
}
