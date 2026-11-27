//! Sales activities (ADR-070), under `/activities`: tasks, calls,
//! meetings, notes and logged emails on leads, opportunities, quotes and
//! contacts. `sales:read` to see them where they are; `sales:write` to add,
//! change and complete them.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::OrgScope;
use huxley_sales::MODULE;
use huxley_sales::activities::{self, Activity, ActivityInput, Filter};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module;
use crate::sales_routes::{access_at, org_of, scoped};
use crate::{ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().nest(
        "/activities",
        Router::new()
            .route("/", get(list_activities).post(create_activity))
            .route(
                "/{id}",
                get(get_activity)
                    .put(update_activity)
                    .delete(delete_activity),
            )
            .route("/{id}/done", post(complete_activity)),
    )
}

/// Activities you can see: on a record, someone's, or yours.
#[utoipa::path(get, path = "/api/v1/activities", operation_id = "list_activities", tag = "sales",
    params(Filter),
    responses((status = 200, body = Vec<Activity>)))]
pub async fn list_activities(
    State(state): State<AppState>,
    authed: Authed,
    Query(mut filter): Query<Filter>,
) -> ApiResult<Json<Vec<Activity>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::SALES_READ, MODULE)
        .await?;
    if filter.mine {
        filter.owner_id = Some(authed.user.auth.user.id);
    }
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            activities::list(tx, &filter).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/activities/{id}", operation_id = "get_activity", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Activity)))]
pub async fn get_activity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Activity>> {
    let org = org_of(&state, async |tx| activities::org_of(tx, id).await).await?;
    let scope = access_at(&state, &authed, org, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| activities::get(tx, id).await).await?,
    ))
}

/// Adds a task, call, meeting or note to a lead, opportunity, quote or
/// contact. A task given to someone else tells them.
#[utoipa::path(post, path = "/api/v1/activities", operation_id = "create_activity", tag = "sales",
    request_body = ActivityInput,
    responses((status = 201, body = Activity), (status = 422, body = crate::ApiError, content_type = "application/problem+json")))]
pub async fn create_activity(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<ActivityInput>,
) -> ApiResult<(StatusCode, Json<Activity>)> {
    let org = scoped(&state, &OrgScope::System, async |tx| {
        activities::org_for(tx, &body).await
    })
    .await?;
    let scope = access_at(&state, &authed, org, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let a = scoped(&state, &scope, async |tx| {
        activities::create(tx, &ctx, org, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(a)))
}

#[utoipa::path(put, path = "/api/v1/activities/{id}", operation_id = "update_activity", tag = "sales",
    params(("id" = Uuid, Path)), request_body = ActivityInput, responses((status = 200, body = Activity)))]
pub async fn update_activity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ActivityInput>,
) -> ApiResult<Json<Activity>> {
    let org = org_of(&state, async |tx| activities::org_of(tx, id).await).await?;
    let scope = access_at(&state, &authed, org, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            activities::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteActivity {
    pub done: bool,
    /// How it went (a call's outcome, say).
    #[serde(default)]
    pub outcome: Option<String>,
}

/// Marks it done, or not.
#[utoipa::path(post, path = "/api/v1/activities/{id}/done", operation_id = "complete_activity", tag = "sales",
    params(("id" = Uuid, Path)), request_body = CompleteActivity, responses((status = 200, body = Activity)))]
pub async fn complete_activity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CompleteActivity>,
) -> ApiResult<Json<Activity>> {
    let org = org_of(&state, async |tx| activities::org_of(tx, id).await).await?;
    let scope = access_at(&state, &authed, org, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            activities::set_done(tx, &ctx, id, body.done, body.outcome.as_deref()).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/activities/{id}", operation_id = "delete_activity", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_activity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org = org_of(&state, async |tx| activities::org_of(tx, id).await).await?;
    let scope = access_at(&state, &authed, org, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        activities::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
