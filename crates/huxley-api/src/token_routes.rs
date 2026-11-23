//! API tokens: `/api/v1/auth/tokens` (your own) and `/api/v1/service-accounts`.

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use huxley_auth::api_tokens::{self, ApiToken};
use huxley_auth::{User, UserKind, UserStatus};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth_routes::UserView;
use crate::authz::{Authed, map_auth};
use crate::session::CurrentUser;
use crate::{ApiError, ApiResult, AppState};

pub fn own_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_own).post(create_own))
        .route("/{id}", delete(revoke_own))
}

pub fn service_account_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_accounts).post(create_account))
        .route("/{id}", get(get_account).patch(update_account))
        .route(
            "/{id}/tokens",
            get(list_account_tokens).post(create_account_token),
        )
        .route("/{id}/tokens/{token_id}", delete(revoke_account_token))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenView {
    pub id: Uuid,
    pub name: String,
    /// The start of the token, to tell tokens apart.
    pub hint: String,
    pub scopes: Vec<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    pub expired: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    pub last_used_ip: Option<String>,
}

impl From<ApiToken> for ApiTokenView {
    fn from(t: ApiToken) -> Self {
        Self {
            expired: t
                .expires_at
                .is_some_and(|at| at <= OffsetDateTime::now_utc()),
            id: t.id,
            name: t.name,
            hint: t.hint,
            scopes: t.scopes,
            expires_at: t.expires_at,
            created_at: t.created_at,
            last_used_at: t.last_used_at,
            last_used_ip: t.last_used_ip,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTokenRequest {
    pub name: String,
    /// Permission keys the token may use, or `["*"]` for everything its
    /// owner may do. Scopes only narrow the owner's roles.
    pub scopes: Vec<String>,
    /// 1 to 366 for people. Omit (service accounts only) for no expiry.
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedTokenView {
    #[serde(flatten)]
    pub token: ApiTokenView,
    /// Send as `Authorization: Bearer <token>`. Shown only now.
    pub token_secret: String,
}

fn validate_scopes(scopes: &[String]) -> ApiResult<Vec<String>> {
    let mut scopes: Vec<String> = scopes.iter().map(|s| s.trim().to_owned()).collect();
    scopes.sort();
    scopes.dedup();
    if scopes.is_empty() {
        return Err(
            ApiError::validation().with_field_error("/scopes", "Choose at least one scope.")
        );
    }
    if let Some(bad) = scopes.iter().find(|s| !permissions::is_known(s)) {
        return Err(
            ApiError::validation().with_field_error("/scopes", format!("Unknown scope `{bad}`."))
        );
    }
    if scopes.iter().any(|s| s == permissions::WILDCARD) {
        return Ok(vec![permissions::WILDCARD.to_owned()]);
    }
    Ok(scopes)
}

fn map_token_error(e: huxley_auth::AuthError) -> ApiError {
    match e {
        huxley_auth::AuthError::InvalidTokenName => {
            ApiError::validation().with_field_error("/name", "Enter a name of 1 to 100 characters.")
        }
        huxley_auth::AuthError::InvalidTokenExpiry(why) => ApiError::validation()
            .with_field_error("/expiresInDays", format!("Invalid expiry: {why}.")),
        huxley_auth::AuthError::TokenNotFound => ApiError::not_found(),
        other => map_auth(other),
    }
}

async fn create_token(
    state: &AppState,
    ctx: &huxley_core::RequestContext,
    owner: &User,
    body: &CreateTokenRequest,
) -> ApiResult<Response> {
    let scopes = validate_scopes(&body.scopes)?;
    let expires_at = body
        .expires_in_days
        .map(|d| OffsetDateTime::now_utc() + Duration::days(i64::from(d)));
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let created = api_tokens::create(&mut tx, ctx, owner, &body.name, &scopes, expires_at)
        .await
        .map_err(map_token_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(CreatedTokenView {
            token: created.token.into(),
            token_secret: created.secret,
        }),
    )
        .into_response())
}

async fn list_tokens(state: &AppState, user_id: Uuid) -> ApiResult<Json<Vec<ApiTokenView>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let tokens = api_tokens::list(&mut tx, user_id)
        .await
        .map_err(map_token_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(tokens.into_iter().map(Into::into).collect()))
}

async fn revoke_token(
    state: &AppState,
    ctx: &huxley_core::RequestContext,
    user_id: Uuid,
    token_id: Uuid,
) -> ApiResult<StatusCode> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    api_tokens::revoke(&mut tx, ctx, user_id, token_id)
        .await
        .map_err(map_token_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Your API tokens. Needs a signed-in session (not a token).
#[utoipa::path(get, path = "/api/v1/auth/tokens", tag = "auth",
    responses((status = 200, body = Vec<ApiTokenView>)))]
pub async fn list_own(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<ApiTokenView>>> {
    list_tokens(&state, user.auth.user.id).await
}

/// Creates a personal access token that acts as you, narrowed to `scopes`.
/// It must expire within a year. Needs a signed-in session (not a token).
#[utoipa::path(post, path = "/api/v1/auth/tokens", tag = "auth",
    request_body = CreateTokenRequest,
    responses((status = 201, body = CreatedTokenView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_own(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateTokenRequest>,
) -> ApiResult<Response> {
    create_token(&state, &user.ctx, &user.auth.user, &body).await
}

/// Revokes one of your API tokens.
#[utoipa::path(delete, path = "/api/v1/auth/tokens/{id}", tag = "auth",
    params(("id" = Uuid, Path, description = "Token id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn revoke_own(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    revoke_token(&state, &user.ctx, user.auth.user.id, id).await
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAccountView {
    #[serde(flatten)]
    pub user: UserView,
    pub description: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateServiceAccountRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateServiceAccountRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    /// `active` or `disabled`. Disabled accounts' tokens stop working.
    pub status: Option<String>,
}

/// The service account `id`, or 404 (also for people's accounts).
async fn service_account(state: &AppState, id: Uuid) -> ApiResult<ServiceAccountView> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let user = huxley_auth::find_user(&mut tx, id)
        .await
        .map_err(map_auth)?
        .filter(|u| u.kind == UserKind::Service)
        .ok_or_else(ApiError::not_found)?;
    let description = api_tokens::description(&mut tx, id)
        .await
        .map_err(map_auth)?;
    tx.commit().await.map_err(db_err)?;
    Ok(ServiceAccountView {
        user: user.into(),
        description,
    })
}

/// Service accounts: non-human identities for integrations and scripts.
#[utoipa::path(get, path = "/api/v1/service-accounts", tag = "users",
    responses((status = 200, body = Vec<ServiceAccountView>), (status = 403, body = ApiError, content_type = "application/problem+json")))]
pub async fn list_accounts(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<ServiceAccountView>>> {
    authed.require_instance(permissions::SERVICE_ACCOUNT_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let accounts = api_tokens::list_service_accounts(&mut tx)
        .await
        .map_err(map_auth)?;
    let mut views = Vec::with_capacity(accounts.len());
    for user in accounts {
        let description = api_tokens::description(&mut tx, user.id)
            .await
            .map_err(map_auth)?;
        views.push(ServiceAccountView {
            user: user.into(),
            description,
        });
    }
    tx.commit().await.map_err(db_err)?;
    Ok(Json(views))
}

/// Creates a service account. Grant it roles with `POST /role-assignments`,
/// then create tokens for it.
#[utoipa::path(post, path = "/api/v1/service-accounts", tag = "users",
    request_body = CreateServiceAccountRequest,
    responses((status = 201, body = ServiceAccountView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_account(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateServiceAccountRequest>,
) -> ApiResult<(StatusCode, Json<ServiceAccountView>)> {
    authed.require_instance(permissions::SERVICE_ACCOUNT_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let user = api_tokens::create_service_account(
        &mut tx,
        &authed.user.ctx,
        &body.name,
        &body.description,
    )
    .await
    .map_err(map_auth)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        Json(ServiceAccountView {
            user: user.into(),
            description: body.description.trim().to_owned(),
        }),
    ))
}

#[utoipa::path(get, path = "/api/v1/service-accounts/{id}", tag = "users",
    params(("id" = Uuid, Path, description = "Service account id")),
    responses((status = 200, body = ServiceAccountView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_account(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ServiceAccountView>> {
    authed.require_instance(permissions::SERVICE_ACCOUNT_MANAGE)?;
    Ok(Json(service_account(&state, id).await?))
}

/// Renames, describes, disables, or re-enables a service account.
#[utoipa::path(patch, path = "/api/v1/service-accounts/{id}", tag = "users",
    params(("id" = Uuid, Path, description = "Service account id")),
    request_body = UpdateServiceAccountRequest,
    responses((status = 200, body = ServiceAccountView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_account(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateServiceAccountRequest>,
) -> ApiResult<Json<ServiceAccountView>> {
    authed.require_instance(permissions::SERVICE_ACCOUNT_MANAGE)?;
    service_account(&state, id).await?;
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
    huxley_auth::update_user(&mut tx, &authed.user.ctx, id, body.name.as_deref(), status)
        .await
        .map_err(map_auth)?;
    if let Some(description) = &body.description {
        api_tokens::set_description(&mut tx, &authed.user.ctx, id, description)
            .await
            .map_err(map_auth)?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(Json(service_account(&state, id).await?))
}

#[utoipa::path(get, path = "/api/v1/service-accounts/{id}/tokens", tag = "users",
    params(("id" = Uuid, Path, description = "Service account id")),
    responses((status = 200, body = Vec<ApiTokenView>)))]
pub async fn list_account_tokens(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<ApiTokenView>>> {
    authed.require_instance(permissions::SERVICE_ACCOUNT_MANAGE)?;
    service_account(&state, id).await?;
    list_tokens(&state, id).await
}

/// Creates a token for a service account. It may be non-expiring.
#[utoipa::path(post, path = "/api/v1/service-accounts/{id}/tokens", tag = "users",
    params(("id" = Uuid, Path, description = "Service account id")),
    request_body = CreateTokenRequest,
    responses((status = 201, body = CreatedTokenView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_account_token(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateTokenRequest>,
) -> ApiResult<Response> {
    authed.require_instance(permissions::SERVICE_ACCOUNT_MANAGE)?;
    service_account(&state, id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let owner = huxley_auth::find_user(&mut tx, id)
        .await
        .map_err(map_auth)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    create_token(&state, &authed.user.ctx, &owner, &body).await
}

#[utoipa::path(delete, path = "/api/v1/service-accounts/{id}/tokens/{token_id}", tag = "users",
    params(("id" = Uuid, Path, description = "Service account id"), ("token_id" = Uuid, Path, description = "Token id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn revoke_account_token(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, token_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SERVICE_ACCOUNT_MANAGE)?;
    service_account(&state, id).await?;
    revoke_token(&state, &authed.user.ctx, id, token_id).await
}
