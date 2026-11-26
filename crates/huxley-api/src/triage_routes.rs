//! AI triage (ADR-047): the labeled sample set, accuracy evaluations (the
//! AI triage pack's workflow run over each sample), and how often live
//! suggestions matched where tickets ended up. Set up with
//! `servicedesk:configure`; agreement needs `ticket:read`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::StartRun;
use huxley_servicedesk::triage::{self, Agreement, Evaluation, QueueOption, Sample, SampleInput};
use huxley_servicedesk::{DeskError, MODULE};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module_at;
use crate::{ApiError, ApiResult, AppState};

/// The workflow evaluations run (the AI triage pack's).
pub const WORKFLOW_KEY: &str = "triage-ticket";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/triage/options", get(options))
        .route("/triage/samples", get(list_samples).post(add_sample))
        .route("/triage/samples/from-tickets", post(samples_from_tickets))
        .route("/triage/samples/{id}", delete(delete_sample))
        .route(
            "/triage/evaluations",
            get(list_evaluations).post(start_evaluation),
        )
        .route("/triage/evaluations/{id}", get(get_evaluation))
        .route("/triage/agreement", get(agreement))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map(e: DeskError) -> ApiError {
    match e {
        DeskError::NotFound => ApiError::not_found(),
        DeskError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        DeskError::VersionConflict => ApiError::conflict("It changed meanwhile; try again."),
        DeskError::Db(e) => ApiError::internal(&e),
        DeskError::Sqlx(e) => ApiError::internal(&e),
    }
}

async fn configure(state: &AppState, authed: &Authed, org_id: Uuid) -> ApiResult<()> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    require_module_at(state, MODULE, org_id).await
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct OrgParam {
    pub org_id: Uuid,
}

/// The queues samples can name (at the organization or above).
#[utoipa::path(get, path = "/api/v1/triage/options", operation_id = "triage_options", tag = "servicedesk",
    params(OrgParam), responses((status = 200, body = Vec<QueueOption>)))]
pub async fn options(
    State(state): State<AppState>,
    authed: Authed,
    Query(q): Query<OrgParam>,
) -> ApiResult<Json<Vec<QueueOption>>> {
    configure(&state, &authed, q.org_id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let o = triage::options(&mut tx, q.org_id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(o.queues))
}

/// The organization's labeled samples, newest first.
#[utoipa::path(get, path = "/api/v1/triage/samples", operation_id = "list_triage_samples", tag = "servicedesk",
    params(OrgParam), responses((status = 200, body = Vec<Sample>)))]
pub async fn list_samples(
    State(state): State<AppState>,
    authed: Authed,
    Query(q): Query<OrgParam>,
) -> ApiResult<Json<Vec<Sample>>> {
    configure(&state, &authed, q.org_id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let all = triage::samples(&mut tx, q.org_id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddSample {
    pub org_id: Uuid,
    #[serde(flatten)]
    pub sample: SampleInput,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Added {
    pub added: u64,
}

/// Adds a labeled sample by hand: text and the right queue, priority or both.
#[utoipa::path(post, path = "/api/v1/triage/samples", operation_id = "add_triage_sample", tag = "servicedesk",
    request_body = AddSample,
    responses((status = 201, body = Added), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_sample(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<AddSample>,
) -> ApiResult<(StatusCode, Json<Added>)> {
    configure(&state, &authed, body.org_id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    triage::add_sample(&mut tx, body.org_id, &body.sample)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(Added { added: 1 })))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FromTickets {
    pub org_id: Uuid,
    /// How many (default 50).
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Adds recently resolved tickets (with a queue) as samples, labeled with
/// where they ended up.
#[utoipa::path(post, path = "/api/v1/triage/samples/from-tickets", operation_id = "add_triage_samples_from_tickets", tag = "servicedesk",
    request_body = FromTickets, responses((status = 201, body = Added)))]
pub async fn samples_from_tickets(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<FromTickets>,
) -> ApiResult<(StatusCode, Json<Added>)> {
    configure(&state, &authed, body.org_id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let added = triage::add_from_tickets(&mut tx, body.org_id, body.limit.unwrap_or(50))
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(Added { added })))
}

#[utoipa::path(delete, path = "/api/v1/triage/samples/{id}", operation_id = "delete_triage_sample", tag = "servicedesk",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_sample(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let gone = triage::delete_sample(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    if gone {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

/// The published triage workflow at `org_id` or nearest above it.
async fn triage_workflow(
    tx: &mut sqlx::PgConnection,
    org_id: Uuid,
) -> ApiResult<(Uuid, Uuid, i32)> {
    let found: Option<(Uuid, Uuid, Option<i32>)> = sqlx::query_as(
        "SELECT w.id, w.org_id, w.current_version FROM workflows w JOIN organizations o ON o.id = w.org_id \
          WHERE w.key = $2 AND w.status <> 'archived' \
            AND o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path) DESC LIMIT 1",
    )
    .bind(org_id)
    .bind(WORKFLOW_KEY)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    match found {
        Some((id, org, Some(version))) => Ok((id, org, version)),
        Some(_) => Err(ApiError::conflict(
            "Publish the “Triage new tickets with AI” workflow first.",
        )),
        None => Err(ApiError::conflict(
            "Install the AI triage pack (and publish its workflow) here or above first.",
        )),
    }
}

/// Runs the AI triage workflow over every sample: each run records its
/// answer in the evaluation. Read it back for the score as runs finish.
#[utoipa::path(post, path = "/api/v1/triage/evaluations", operation_id = "start_triage_evaluation", tag = "servicedesk",
    request_body = OrgParamBody,
    responses((status = 201, body = Evaluation), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn start_evaluation(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<OrgParamBody>,
) -> ApiResult<(StatusCode, Json<Evaluation>)> {
    configure(&state, &authed, body.org_id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (workflow, _, version) = triage_workflow(&mut tx, body.org_id).await?;
    let target =
        huxley_orgs::settings::value(&mut tx, body.org_id, huxley_orgs::settings::TRIAGE_TARGET)
            .await
            .map_err(db_err)?
            .and_then(|v| v.parse::<i16>().ok())
            .unwrap_or(80)
            .clamp(0, 100);
    let started = triage::start_evaluation(
        &mut tx,
        &authed.user.ctx,
        body.org_id,
        (workflow, version),
        target,
    )
    .await
    .map_err(map)?;
    for (position, sample, text) in &started.samples {
        let run = state
            .engine
            .start_run(
                &mut tx,
                StartRun {
                    // At the organization evaluated (its queues and AI
                    // provider), with the workflow from there or above.
                    org_id: body.org_id,
                    workflow_id: Some(workflow),
                    version: Some(version),
                    document: None,
                    trigger_type: "manual".into(),
                    trigger: json!({}),
                    inputs: json!({
                        "text": text,
                        "evaluation_id": started.evaluation_id,
                        "sample_id": sample,
                    }),
                    actor_id: authed.user.ctx.actor.id(),
                },
            )
            .await
            .map_err(db_err)?;
        triage::set_run(&mut tx, started.evaluation_id, *position, run)
            .await
            .map_err(map)?;
    }
    let evaluation = triage::evaluation(&mut tx, started.evaluation_id)
        .await
        .map_err(map)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(evaluation)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrgParamBody {
    pub org_id: Uuid,
}

/// The organization's evaluations, newest first (at most 20).
#[utoipa::path(get, path = "/api/v1/triage/evaluations", operation_id = "list_triage_evaluations", tag = "servicedesk",
    params(OrgParam), responses((status = 200, body = Vec<Evaluation>)))]
pub async fn list_evaluations(
    State(state): State<AppState>,
    authed: Authed,
    Query(q): Query<OrgParam>,
) -> ApiResult<Json<Vec<Evaluation>>> {
    configure(&state, &authed, q.org_id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let all = triage::evaluations(&mut tx, q.org_id, 20)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

/// An evaluation: accuracy against the target, and the misses.
#[utoipa::path(get, path = "/api/v1/triage/evaluations/{id}", operation_id = "get_triage_evaluation", tag = "servicedesk",
    params(("id" = Uuid, Path)), responses((status = 200, body = Evaluation)))]
pub async fn get_evaluation(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Evaluation>> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let found = triage::evaluation(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    found.map(Json).ok_or_else(ApiError::not_found)
}

/// How often live suggestions matched where resolved tickets ended up
/// (the last 90 days, the organization and below).
#[utoipa::path(get, path = "/api/v1/triage/agreement", operation_id = "get_triage_agreement", tag = "servicedesk",
    params(OrgParam), responses((status = 200, body = Agreement)))]
pub async fn agreement(
    State(state): State<AppState>,
    authed: Authed,
    Query(q): Query<OrgParam>,
) -> ApiResult<Json<Agreement>> {
    require_module_at(&state, MODULE, q.org_id).await?;
    authed
        .require_at(&state, permissions::TICKET_READ, q.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let a = triage::agreement(&mut tx, q.org_id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(a))
}
