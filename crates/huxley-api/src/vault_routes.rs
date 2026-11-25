//! The `vault` module's API (design doc §8.5): client passwords, keys and
//! secret notes.
//!
//! - `vault:read` at an organization: its items (never their secrets) and
//!   who revealed them.
//! - `vault:reveal`: reveal a secret. Each reveal is logged, audited and
//!   raised as `vault.secret_revealed`; the response isn't cached.
//! - `vault:manage`: add, change and delete items and their secrets.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_vault::{Filter, Item, ItemChanges, ItemInput, MODULE, Reveal, VaultError};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/items", get(list_vault_items).post(create_vault_item))
        .route(
            "/items/{id}",
            get(get_vault_item)
                .patch(update_vault_item)
                .delete(delete_vault_item),
        )
        .route("/items/{id}/reveal", post(reveal_vault_secret))
        .route("/items/{id}/reveals", get(list_item_reveals))
        .route("/reveals", get(list_vault_reveals))
}

fn map(e: VaultError) -> ApiError {
    match e {
        VaultError::NotFound => ApiError::not_found(),
        VaultError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        VaultError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        VaultError::NoSecret => ApiError::conflict("This item has no secret to reveal."),
        VaultError::Crypto(e) => ApiError::internal(&e),
        VaultError::Db(e) => ApiError::internal(&e),
        VaultError::Sqlx(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, VaultError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope).await.map_err(db_err)?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(value)
}

/// The module is on at `org_id` and the caller holds `permission` there:
/// the scope to work in.
async fn allow(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    org_id: Uuid,
) -> ApiResult<OrgScope> {
    require_module_at(state, MODULE, org_id).await?;
    authed.require_at(state, permission, org_id).await?;
    authed.scope(state).await
}

/// An item's organization, checked for `permission`.
async fn item_access(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM vault_items WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let org = org.ok_or_else(ApiError::not_found)?;
    allow(state, authed, permission, org).await
}

/// Whether the caller holds `permission` where the item is.
async fn can(state: &AppState, authed: &Authed, permission: &str, org_id: Uuid) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let path = huxley_authz::org_path(&mut tx, org_id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(authed.principal.can_at_path(permission, &path))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct VaultParams {
    /// Words in the name, username, URL or tags.
    pub q: Option<String>,
    /// Only this organization's items.
    pub org_id: Option<Uuid>,
}

/// Items you can see, by name (never their secrets).
#[utoipa::path(get, path = "/api/v1/vault/items", tag = "vault", params(VaultParams),
    responses((status = 200, body = Vec<Item>)))]
pub async fn list_vault_items(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<VaultParams>,
) -> ApiResult<Json<Vec<Item>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::VAULT_READ, MODULE)
        .await?;
    let filter = Filter {
        q: params.q,
        org_id: params.org_id,
        limit: 500,
    };
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            huxley_vault::list(tx, &filter).await
        })
        .await?,
    ))
}

/// An item, with what you may do with it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VaultItemView {
    #[serde(flatten)]
    pub item: Item,
    pub can_reveal: bool,
    pub can_manage: bool,
}

async fn view(state: &AppState, authed: &Authed, item: Item) -> ApiResult<VaultItemView> {
    let org = item.org_id;
    Ok(VaultItemView {
        item,
        can_reveal: can(state, authed, permissions::VAULT_REVEAL, org).await?,
        can_manage: can(state, authed, permissions::VAULT_MANAGE, org).await?,
    })
}

/// An item (never its secret).
#[utoipa::path(get, path = "/api/v1/vault/items/{id}", tag = "vault", params(("id" = Uuid, Path)),
    responses((status = 200, body = VaultItemView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_vault_item(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<VaultItemView>> {
    let scope = item_access(&state, &authed, id, permissions::VAULT_READ).await?;
    let item = scoped(&state, &scope, async |tx| huxley_vault::get(tx, id).await).await?;
    Ok(Json(view(&state, &authed, item).await?))
}

/// Adds an item at an organization (`vault:manage` there). The secret is
/// stored encrypted.
#[utoipa::path(post, path = "/api/v1/vault/items", tag = "vault", request_body = ItemInput,
    responses((status = 201, body = VaultItemView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_vault_item(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<ItemInput>,
) -> ApiResult<(StatusCode, Json<VaultItemView>)> {
    let scope = allow(&state, &authed, permissions::VAULT_MANAGE, body.org_id).await?;
    let ctx = authed.user.ctx.clone();
    let crypto = state.crypto.clone();
    let item = scoped(&state, &scope, async |tx| {
        huxley_vault::create(tx, &ctx, &crypto, &body).await
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(view(&state, &authed, item).await?),
    ))
}

/// Changes an item; send back the `version` you read. A `secret` replaces
/// the current one; `clearSecret` removes it.
#[utoipa::path(patch, path = "/api/v1/vault/items/{id}", tag = "vault", params(("id" = Uuid, Path)),
    request_body = ItemChanges,
    responses((status = 200, body = VaultItemView), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_vault_item(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ItemChanges>,
) -> ApiResult<Json<VaultItemView>> {
    let scope = item_access(&state, &authed, id, permissions::VAULT_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    let crypto = state.crypto.clone();
    let item = scoped(&state, &scope, async |tx| {
        huxley_vault::update(tx, &ctx, &crypto, id, &body).await
    })
    .await?;
    Ok(Json(view(&state, &authed, item).await?))
}

/// Deletes an item; who revealed it stays on record.
#[utoipa::path(delete, path = "/api/v1/vault/items/{id}", tag = "vault", params(("id" = Uuid, Path)),
    responses((status = 204)))]
pub async fn delete_vault_item(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let scope = item_access(&state, &authed, id, permissions::VAULT_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        huxley_vault::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevealRequest {
    /// Why: kept with the reveal (e.g. the ticket being worked).
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RevealedSecret {
    pub secret: String,
}

/// Reveals an item's secret (`vault:reveal`). Each reveal is logged with
/// who, when and the reason, audited, and raised as
/// `vault.secret_revealed`. The response isn't cached.
#[utoipa::path(post, path = "/api/v1/vault/items/{id}/reveal", tag = "vault", params(("id" = Uuid, Path)),
    request_body = RevealRequest,
    responses((status = 200, body = RevealedSecret), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn reveal_vault_secret(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    body: Option<Json<RevealRequest>>,
) -> ApiResult<Response> {
    let scope = item_access(&state, &authed, id, permissions::VAULT_REVEAL).await?;
    let ctx = authed.user.ctx.clone();
    let crypto = state.crypto.clone();
    let reason = body.and_then(|Json(b)| b.reason);
    let secret = scoped(&state, &scope, async |tx| {
        huxley_vault::reveal(tx, &ctx, &crypto, id, reason.as_deref()).await
    })
    .await?;
    let mut response = Json(RevealedSecret {
        secret: secret.to_string(),
    })
    .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    Ok(response)
}

/// Who revealed an item's secret, newest first (`vault:read`).
#[utoipa::path(get, path = "/api/v1/vault/items/{id}/reveals", tag = "vault", params(("id" = Uuid, Path)),
    responses((status = 200, body = Vec<Reveal>)))]
pub async fn list_item_reveals(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Reveal>>> {
    let scope = item_access(&state, &authed, id, permissions::VAULT_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            huxley_vault::reveals(tx, Some(id), 200).await
        })
        .await?,
    ))
}

/// Every reveal you can see, newest first, including those of deleted
/// items (`vault:read`).
#[utoipa::path(get, path = "/api/v1/vault/reveals", tag = "vault",
    responses((status = 200, body = Vec<Reveal>)))]
pub async fn list_vault_reveals(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Reveal>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::VAULT_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            huxley_vault::reveals(tx, None, 500).await
        })
        .await?,
    ))
}
