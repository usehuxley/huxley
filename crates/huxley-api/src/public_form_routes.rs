//! `/public/forms/{token}`: forms filled in through a public link, without
//! signing in (design doc §6.8). Outside `/api/v1`: no session or CSRF;
//! the token is the credential, and requests are rate limited per client
//! address. Answers start runs that act as each workflow's run-as account.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use huxley_db::{OrgScope, begin_scoped};
use huxley_forms::{FormError, Submitter};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::ToSchema;

use crate::context::ClientInfo;
use crate::{ApiError, ApiResult, AppState};

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// What someone with the link sees.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicForm {
    pub name: String,
    pub description: Option<String>,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub schema: JsonValue,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub ui: JsonValue,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicSubmission {
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub answers: JsonValue,
}

/// Only that it was received: nothing about the runs.
#[derive(Debug, Serialize, ToSchema)]
pub struct Received {
    pub received: bool,
}

fn limited(state: &AppState, ctx: &huxley_core::RequestContext) -> ApiResult<()> {
    if let Some(ip) = ctx.ip
        && !state.public_limiter.check(ip)
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate-limited",
            "Too many requests",
        )
        .with_detail("Wait a minute and try again."));
    }
    Ok(())
}

fn map_err(e: FormError) -> ApiError {
    match e {
        // Unknown, revoked and expired links look the same.
        FormError::NotFound | FormError::Archived => ApiError::not_found(),
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

/// The form a public link opens.
#[utoipa::path(get, path = "/public/forms/{token}", operation_id = "get_public_form", tag = "forms",
    params(("token" = String, Path)),
    responses((status = 200, body = PublicForm), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn form(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Path(token): Path<String>,
) -> ApiResult<Json<PublicForm>> {
    limited(&state, &ctx)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (form, _) = huxley_forms::open(&mut tx, &token).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(PublicForm {
        name: form.name,
        description: form.description,
        schema: form.schema,
        ui: form.ui,
    }))
}

/// Submits answers through a public link.
#[utoipa::path(post, path = "/public/forms/{token}", operation_id = "submit_public_form", tag = "forms",
    params(("token" = String, Path)), request_body = PublicSubmission,
    responses((status = 201, body = Received),
              (status = 404, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json"),
              (status = 429, body = ApiError, content_type = "application/problem+json")))]
pub async fn submit(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Path(token): Path<String>,
    Json(body): Json<PublicSubmission>,
) -> ApiResult<(StatusCode, Json<Received>)> {
    limited(&state, &ctx)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (form, link) = huxley_forms::open(&mut tx, &token).await.map_err(map_err)?;
    huxley_forms::submit(
        &mut tx,
        &state.engine,
        &ctx,
        form.id,
        link.org_id,
        Submitter::Link(link.id),
        huxley_forms::Submission {
            answers: body.answers,
            context: None,
        },
    )
    .await
    .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(Received { received: true })))
}
