//! `/api/v1/teams`: groups of users that hold role assignments.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use huxley_authz::teams::{self, Team, TeamMember};
use huxley_authz::{Assignment, permissions};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::{Authed, map_authz};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(get_team).put(update).delete(remove))
        .route(
            "/{id}/members/{user_id}",
            put(add_member).delete(remove_member),
        )
        .route("/of-user/{user_id}", get(of_user))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamDetail {
    #[serde(flatten)]
    pub team: Team,
    pub members: Vec<TeamMember>,
    /// The team's role assignments that the caller can see.
    pub assignments: Vec<Assignment>,
}

/// Teams. Anyone who can view users can view teams.
#[utoipa::path(get, path = "/api/v1/teams", operation_id = "list_teams", tag = "users",
    responses((status = 200, body = Vec<Team>), (status = 403, body = ApiError, content_type = "application/problem+json")))]
pub async fn list(State(state): State<AppState>, authed: Authed) -> ApiResult<Json<Vec<Team>>> {
    authed.require_instance(permissions::USER_READ)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let list = teams::list(&mut tx).await.map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(list))
}

/// A team with its members and role assignments.
#[utoipa::path(get, path = "/api/v1/teams/{id}", operation_id = "get_team", tag = "users",
    params(("id" = Uuid, Path, description = "Team id")),
    responses((status = 200, body = TeamDetail), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_team(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TeamDetail>> {
    authed.require_instance(permissions::USER_READ)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let team = teams::get(&mut tx, id).await.map_err(map_authz)?;
    let members = teams::members(&mut tx, id).await.map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    // Assignments are read in the caller's RLS scope, like a user's.
    let mut scoped = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let assignments = teams::assignments(&mut scoped, id)
        .await
        .map_err(map_authz)?;
    scoped.commit().await.map_err(db_err)?;
    Ok(Json(TeamDetail {
        team,
        members,
        assignments,
    }))
}

#[utoipa::path(post, path = "/api/v1/teams", operation_id = "create_team", tag = "users",
    request_body = TeamInput,
    responses((status = 201, body = Team), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<TeamInput>,
) -> ApiResult<(StatusCode, Json<Team>)> {
    authed.require_instance(permissions::TEAM_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let team = teams::create(&mut tx, &authed.user.ctx, &body.name, &body.description)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(team)))
}

#[utoipa::path(put, path = "/api/v1/teams/{id}", operation_id = "update_team", tag = "users",
    params(("id" = Uuid, Path, description = "Team id")),
    request_body = TeamInput,
    responses((status = 200, body = Team), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<TeamInput>,
) -> ApiResult<Json<Team>> {
    authed.require_instance(permissions::TEAM_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let team = teams::update(&mut tx, &authed.user.ctx, id, &body.name, &body.description)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(team))
}

/// Deletes a team and the roles it held. Members keep their own roles.
#[utoipa::path(delete, path = "/api/v1/teams/{id}", operation_id = "delete_team", tag = "users",
    params(("id" = Uuid, Path, description = "Team id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn remove(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::TEAM_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    teams::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Adds a user to a team (idempotent). They get the team's roles at once.
#[utoipa::path(put, path = "/api/v1/teams/{id}/members/{user_id}", operation_id = "add_team_member", tag = "users",
    params(("id" = Uuid, Path, description = "Team id"), ("user_id" = Uuid, Path, description = "User id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_member(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::TEAM_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    teams::add_member(&mut tx, &authed.user.ctx, id, user_id)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a user from a team. They lose the team's roles at once.
#[utoipa::path(delete, path = "/api/v1/teams/{id}/members/{user_id}", operation_id = "remove_team_member", tag = "users",
    params(("id" = Uuid, Path, description = "Team id"), ("user_id" = Uuid, Path, description = "User id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn remove_member(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::TEAM_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    teams::remove_member(&mut tx, &authed.user.ctx, id, user_id)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The teams a user belongs to.
#[utoipa::path(get, path = "/api/v1/teams/of-user/{user_id}", operation_id = "list_user_teams", tag = "users",
    params(("user_id" = Uuid, Path, description = "User id")),
    responses((status = 200, body = Vec<Team>)))]
pub async fn of_user(
    State(state): State<AppState>,
    authed: Authed,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Team>>> {
    authed.require_instance(permissions::USER_READ)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let list = teams::of_user(&mut tx, user_id).await.map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(list))
}
