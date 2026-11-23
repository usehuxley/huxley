//! `/api/v1/forms`: forms people fill in to start workflows (design doc §6.8).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_forms::{Form, FormError, FormInput, Submitted};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update))
        .route("/{id}/archive", post(archive))
        .route("/{id}/restore", post(restore))
        .route("/{id}/submissions", get(submissions).post(submit))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map_err(e: FormError) -> ApiError {
    match e {
        FormError::NotFound => ApiError::not_found(),
        FormError::KeyTaken(_) | FormError::Archived => ApiError::conflict(e.to_string()),
        FormError::InvalidDefinition(at, message) => {
            ApiError::validation().with_field_error(at, message)
        }
        FormError::InvalidAnswers(problems) => problems.into_iter().fold(
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "answers-invalid",
                "Some answers don't fit the form",
            ),
            |error, (at, message)| error.with_field_error(format!("answers{at}"), message),
        ),
        other => ApiError::internal(&other),
    }
}

async fn org_path(state: &AppState, org: Uuid) -> ApiResult<Option<String>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let path: Option<String> =
        sqlx::query_scalar("SELECT path::text FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(path)
}

async fn load(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<Form> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let form = huxley_forms::get(&mut tx, id).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(form)
}

/// Forms you can fill in or manage.
#[utoipa::path(get, path = "/api/v1/forms", operation_id = "list_forms", tag = "forms",
    responses((status = 200, body = Vec<Form>)))]
pub async fn list(State(state): State<AppState>, authed: Authed) -> ApiResult<Json<Vec<Form>>> {
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let all = huxley_forms::list(&mut tx).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    let mut visible = Vec::new();
    for form in all {
        if let Some(path) = org_path(&state, form.org_id).await?
            && (authed
                .principal
                .can_at_path(permissions::FORM_SUBMIT, &path)
                || authed
                    .principal
                    .can_at_path(permissions::FORM_MANAGE, &path))
        {
            visible.push(form);
        }
    }
    Ok(Json(visible))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateFormRequest {
    pub org_id: Uuid,
    pub form: FormInput,
}

#[utoipa::path(post, path = "/api/v1/forms", operation_id = "create_form", tag = "forms",
    request_body = CreateFormRequest,
    responses((status = 201, body = Form), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateFormRequest>,
) -> ApiResult<(StatusCode, Json<Form>)> {
    authed
        .require_at(&state, permissions::FORM_MANAGE, body.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let form = huxley_forms::create(&mut tx, &authed.user.ctx, body.org_id, &body.form)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(form)))
}

#[utoipa::path(get, path = "/api/v1/forms/{id}", operation_id = "get_form", tag = "forms",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Form), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn detail(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Form>> {
    let form = load(&state, &authed, id).await?;
    if authed
        .require_at(&state, permissions::FORM_SUBMIT, form.org_id)
        .await
        .is_err()
    {
        authed
            .require_at(&state, permissions::FORM_MANAGE, form.org_id)
            .await?;
    }
    Ok(Json(form))
}

/// Replaces a form's definition (its key stays).
#[utoipa::path(put, path = "/api/v1/forms/{id}", operation_id = "update_form", tag = "forms",
    params(("id" = Uuid, Path)), request_body = FormInput,
    responses((status = 200, body = Form), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<FormInput>,
) -> ApiResult<Json<Form>> {
    let form = load(&state, &authed, id).await?;
    authed
        .require_at(&state, permissions::FORM_MANAGE, form.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let form = huxley_forms::update(&mut tx, &authed.user.ctx, id, &body)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(form))
}

async fn set_archived(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    archived: bool,
) -> ApiResult<Json<Form>> {
    let form = load(state, authed, id).await?;
    authed
        .require_at(state, permissions::FORM_MANAGE, form.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let form = huxley_forms::set_archived(&mut tx, &authed.user.ctx, id, archived)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(form))
}

/// Archives a form: it can't be submitted until restored.
#[utoipa::path(post, path = "/api/v1/forms/{id}/archive", operation_id = "archive_form", tag = "forms",
    params(("id" = Uuid, Path)), responses((status = 200, body = Form)))]
pub async fn archive(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Form>> {
    set_archived(&state, &authed, id, true).await
}

#[utoipa::path(post, path = "/api/v1/forms/{id}/restore", operation_id = "restore_form", tag = "forms",
    params(("id" = Uuid, Path)), responses((status = 200, body = Form)))]
pub async fn restore(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Form>> {
    set_archived(&state, &authed, id, false).await
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitRequest {
    /// The organization it's for: the form's (default) or one below it.
    pub org_id: Option<Uuid>,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub answers: JsonValue,
}

/// Submits answers. Starts the published workflows with a form trigger
/// for this form, as you; answers that don't fit are a 422 per field.
#[utoipa::path(post, path = "/api/v1/forms/{id}/submissions", operation_id = "submit_form", tag = "forms",
    params(("id" = Uuid, Path)), request_body = SubmitRequest,
    responses((status = 201, body = Submitted),
              (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn submit(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SubmitRequest>,
) -> ApiResult<(StatusCode, Json<Submitted>)> {
    let form = load(&state, &authed, id).await?;
    let org_id = body.org_id.unwrap_or(form.org_id);
    authed
        .require_at(&state, permissions::FORM_SUBMIT, org_id)
        .await?;
    let (Some(form_path), Some(org_path)) = (
        org_path(&state, form.org_id).await?,
        org_path(&state, org_id).await?,
    ) else {
        return Err(ApiError::not_found());
    };
    if org_path != form_path && !org_path.starts_with(&format!("{form_path}.")) {
        return Err(ApiError::validation()
            .with_field_error("orgId", "the form's organization or one below it"));
    }
    // The workflows usually live above the submitter's scope.
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let submitted = huxley_forms::submit(
        &mut tx,
        &state.engine,
        &authed.user.ctx,
        id,
        org_id,
        Some(authed.user.auth.user.id),
        body.answers,
    )
    .await
    .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(submitted)))
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionView {
    pub id: Uuid,
    pub org_id: Uuid,
    pub submitted_by: Option<Uuid>,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub data: JsonValue,
    pub run_ids: Vec<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// The latest submissions, for people who manage the form.
#[utoipa::path(get, path = "/api/v1/forms/{id}/submissions", operation_id = "list_form_submissions", tag = "forms",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<SubmissionView>)))]
pub async fn submissions(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<SubmissionView>>> {
    let form = load(&state, &authed, id).await?;
    authed
        .require_at(&state, permissions::FORM_MANAGE, form.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let rows: Vec<SubmissionView> = sqlx::query_as(
        "SELECT id, org_id, submitted_by, data, run_ids, created_at FROM form_submissions \
          WHERE form_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(rows))
}
