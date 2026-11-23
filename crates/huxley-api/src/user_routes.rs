//! `/api/v1/users`, `/api/v1/roles`, `/api/v1/permissions`, and role assignments.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use huxley_auth::UserStatus;
use huxley_authz::{Assignment, Permission, Role, permissions};
use huxley_core::{Cursor, PageRequest};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth_routes::UserView;
use crate::authz::{Authed, map_auth, map_authz};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/{id}", get(get_user).patch(update_user))
        .route("/users/{id}/assignments", get(list_assignments))
        .route("/users/{id}/mfa", delete(reset_mfa))
        .route("/role-assignments", post(create_assignment))
        .route("/role-assignments/{id}", delete(delete_assignment))
        .route("/roles", get(list_roles))
        .route("/permissions", get(list_permissions))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserPage {
    pub items: Vec<UserView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct PageParams {
    /// Page size, 1 to 200 (default 50).
    pub limit: Option<u32>,
    /// Opaque cursor from the previous page's `nextCursor`.
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct UserListParams {
    /// Page size, 1 to 200 (default 50).
    pub limit: Option<u32>,
    /// Opaque cursor from the previous page's `nextCursor`.
    pub cursor: Option<String>,
    /// `human` or `service`. Omit for both.
    pub kind: Option<String>,
}

/// Lists users (people and service accounts; filter with `kind`).
#[utoipa::path(get, path = "/api/v1/users", tag = "users", params(UserListParams),
    responses((status = 200, body = UserPage), (status = 403, body = ApiError, content_type = "application/problem+json")))]
pub async fn list_users(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<UserListParams>,
) -> ApiResult<Json<UserPage>> {
    authed.require_instance(permissions::USER_READ)?;
    let kind = match params.kind.as_deref() {
        None => None,
        Some("human") => Some(huxley_auth::UserKind::Human),
        Some("service") => Some(huxley_auth::UserKind::Service),
        Some(_) => {
            return Err(
                ApiError::validation().with_field_error("/kind", "Use `human` or `service`.")
            );
        }
    };
    let page = PageRequest {
        limit: params.limit,
        cursor: params.cursor.map(Cursor::from),
    };
    let after: Option<Uuid> = page.cursor.as_ref().map(Cursor::decode).transpose()?;
    let limit = page.limit();
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    let users = huxley_auth::list_users(&mut conn, after, limit, kind)
        .await
        .map_err(map_auth)?;
    let page = huxley_core::Page::from_overfetch(users, limit, |u| u.id);
    Ok(Json(UserPage {
        items: page.items.into_iter().map(UserView::from).collect(),
        next_cursor: page.next_cursor.map(|c| c.as_str().to_owned()),
    }))
}

#[utoipa::path(get, path = "/api/v1/users/{id}", tag = "users",
    params(("id" = Uuid, Path, description = "User id")),
    responses((status = 200, body = UserView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_user(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<UserView>> {
    authed.require_instance(permissions::USER_READ)?;
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    let user = huxley_auth::find_user(&mut conn, id)
        .await
        .map_err(map_auth)?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(user.into()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    pub email: String,
    pub display_name: String,
    /// Initial password. If omitted, one is generated and returned once.
    #[schema(format = Password)]
    pub password: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserResponse {
    pub user: UserView,
    /// Present only when Huxley generated the password. Shown once.
    pub generated_password: Option<String>,
}

/// Creates a local user. The new user has no roles until one is assigned.
#[utoipa::path(post, path = "/api/v1/users", tag = "users", request_body = CreateUserRequest,
    responses(
        (status = 201, body = CreateUserResponse),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn create_user(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateUserRequest>,
) -> ApiResult<(StatusCode, Json<CreateUserResponse>)> {
    authed.require_instance(permissions::USER_CREATE)?;
    let (password, generated) = match body.password {
        Some(p) if !p.is_empty() => (p, false),
        _ => {
            let mut bytes = [0u8; 18];
            getrandom::fill(&mut bytes).map_err(|e| ApiError::internal(&e))?;
            (URL_SAFE_NO_PAD.encode(bytes), true)
        }
    };
    let mut tx = state.db.begin().await.map_err(db_err)?;
    let user = huxley_auth::create_user(
        &mut tx,
        &authed.user.ctx,
        &body.email,
        &body.display_name,
        &password,
    )
    .await
    .map_err(map_auth)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        Json(CreateUserResponse {
            user: user.into(),
            generated_password: generated.then_some(password),
        }),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    pub display_name: Option<String>,
    /// `active` or `disabled`. Disabling signs the user out everywhere.
    pub status: Option<String>,
}

/// Edits a user's name or status.
#[utoipa::path(patch, path = "/api/v1/users/{id}", tag = "users",
    params(("id" = Uuid, Path, description = "User id")),
    request_body = UpdateUserRequest,
    responses((status = 200, body = UserView), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_user(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> ApiResult<Json<UserView>> {
    authed.require_instance(permissions::USER_UPDATE)?;
    let status = match body.status.as_deref() {
        None => None,
        Some("active") => Some(UserStatus::Active),
        Some("disabled") => Some(UserStatus::Disabled),
        Some(_) => {
            return Err(
                ApiError::validation().with_field_error("/status", "Use `active` or `disabled`.")
            );
        }
    };
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    if status == Some(UserStatus::Disabled) {
        if id == authed.principal.user_id {
            return Err(ApiError::conflict("You can't disable your own account."));
        }
        if huxley_authz::is_last_owner(&mut tx, id)
            .await
            .map_err(map_authz)?
        {
            return Err(ApiError::conflict(
                "The instance must keep at least one active owner.",
            ));
        }
    }
    let user = huxley_auth::update_user(
        &mut tx,
        &authed.user.ctx,
        id,
        body.display_name.as_deref(),
        status,
    )
    .await
    .map_err(map_auth)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(user.into()))
}

/// Removes a user's second factors (authenticator app and recovery codes) and
/// signs them out everywhere, for when they've lost access to both. Only an
/// owner can reset an owner's MFA.
#[utoipa::path(delete, path = "/api/v1/users/{id}/mfa", tag = "users",
    params(("id" = Uuid, Path, description = "User id")),
    responses(
        (status = 204, description = "MFA removed; the user signs in with a password and can set it up again"),
        (status = 403, body = ApiError, content_type = "application/problem+json"),
        (status = 404, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn reset_mfa(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::USER_UPDATE)?;
    if id == authed.principal.user_id {
        return Err(ApiError::conflict(
            "Reset your own MFA from your profile instead.",
        ));
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    huxley_auth::find_user(&mut tx, id)
        .await
        .map_err(map_auth)?
        .ok_or_else(ApiError::not_found)?;
    let target = huxley_authz::load_principal(&mut tx, id)
        .await
        .map_err(map_authz)?;
    if target.is_owner() && !authed.principal.is_owner() {
        return Err(ApiError::forbidden().with_detail("Only an owner can reset an owner's MFA."));
    }
    huxley_auth::mfa::reset_mfa(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_auth)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// A user's role assignments that the caller can see.
#[utoipa::path(get, path = "/api/v1/users/{id}/assignments", tag = "users",
    params(("id" = Uuid, Path, description = "User id")),
    responses((status = 200, body = Vec<Assignment>)))]
pub async fn list_assignments(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Assignment>>> {
    authed.require_instance(permissions::USER_READ)?;
    let scope = authed.scope(&state).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let assignments = huxley_authz::user_assignments(&mut tx, id)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(assignments))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateAssignmentRequest {
    /// Grant to this user. Send exactly one of `userId` or `teamId`.
    pub user_id: Option<Uuid>,
    /// Grant to this team; its members receive the role.
    pub team_id: Option<Uuid>,
    pub role_id: Uuid,
    pub org_id: Uuid,
    /// Also apply to every sub-organization (default true).
    #[serde(default = "default_true")]
    pub inherit: bool,
}

fn default_true() -> bool {
    true
}

/// Grants a role to a user at an organization.
#[utoipa::path(post, path = "/api/v1/role-assignments", tag = "users",
    request_body = CreateAssignmentRequest,
    responses(
        (status = 201, body = Assignment),
        (status = 403, body = ApiError, content_type = "application/problem+json"),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn create_assignment(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateAssignmentRequest>,
) -> ApiResult<(StatusCode, Json<Assignment>)> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let assignee = match (body.user_id, body.team_id) {
        (Some(user_id), None) => {
            huxley_auth::find_user(&mut tx, user_id)
                .await
                .map_err(map_auth)?
                .ok_or_else(ApiError::not_found)?;
            huxley_authz::Assignee::User(user_id)
        }
        (None, Some(team_id)) => huxley_authz::Assignee::Team(team_id),
        _ => {
            return Err(ApiError::validation()
                .with_field_error("/userId", "Send exactly one of `userId` or `teamId`."));
        }
    };
    let assignment = huxley_authz::assign_to(
        &mut tx,
        &authed.user.ctx,
        &authed.principal,
        assignee,
        body.role_id,
        body.org_id,
        body.inherit,
    )
    .await
    .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(assignment)))
}

/// Removes a role assignment.
#[utoipa::path(delete, path = "/api/v1/role-assignments/{id}", tag = "users",
    params(("id" = Uuid, Path, description = "Assignment id")),
    responses((status = 204), (status = 409, description = "Would remove the last owner", body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_assignment(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    huxley_authz::unassign(&mut tx, &authed.user.ctx, &authed.principal, id)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Roles that can be assigned.
#[utoipa::path(get, path = "/api/v1/roles", tag = "users", responses((status = 200, body = Vec<Role>)))]
pub async fn list_roles(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Role>>> {
    // Anyone who can assign roles somewhere needs the list, not just role:read holders.
    if !authed.principal.can_instance(permissions::ROLE_READ)
        && !authed
            .principal
            .granted_anywhere()
            .contains(&permissions::ROLE_ASSIGN)
    {
        return Err(ApiError::forbidden());
    }
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    Ok(Json(
        huxley_authz::list_roles(&mut conn)
            .await
            .map_err(map_authz)?,
    ))
}

/// Every permission Huxley checks.
#[utoipa::path(get, path = "/api/v1/permissions", tag = "users", responses((status = 200, body = Vec<Permission>)))]
pub async fn list_permissions(_authed: Authed) -> Json<&'static [Permission]> {
    Json(permissions::ALL)
}
