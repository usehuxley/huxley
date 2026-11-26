//! The product catalog (design doc §8.7, the `contracts` module).
//!
//! - `catalog:read` at an organization: its products, services and bundles.
//! - `catalog:manage`: add, change, retire and delete them.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_contracts::catalog::{self, Filter, Product, ProductInput};
use huxley_contracts::{ContractsError, MODULE};
use huxley_db::{OrgScope, begin_scoped};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_products).post(create_product))
        .route(
            "/{id}",
            get(get_product).put(update_product).delete(delete_product),
        )
}

fn map(e: ContractsError) -> ApiError {
    match e {
        ContractsError::NotFound => ApiError::not_found(),
        ContractsError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        ContractsError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        ContractsError::Conflict(message) => ApiError::conflict(message),
        ContractsError::Db(e) => ApiError::internal(&e),
        ContractsError::Sqlx(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, ContractsError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope).await.map_err(db_err)?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(value)
}

/// The module is on at the product's organization and the caller holds
/// `permission` there: the scope to work in.
async fn product_access(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM catalog_products WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let org = org.ok_or_else(ApiError::not_found)?;
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    authed.scope(state).await
}

/// Products you can see, by name (at most 1000).
#[utoipa::path(get, path = "/api/v1/catalog/products", operation_id = "list_catalog_products",
    tag = "catalog", params(Filter), responses((status = 200, body = Vec<Product>)))]
pub async fn list_products(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<Filter>,
) -> ApiResult<Json<Vec<Product>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::CATALOG_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| catalog::list(tx, &filter).await).await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/catalog/products/{id}", operation_id = "get_catalog_product",
    tag = "catalog", params(("id" = Uuid, Path)),
    responses((status = 200, body = Product), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_product(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Product>> {
    let scope = product_access(&state, &authed, id, permissions::CATALOG_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| catalog::get(tx, id).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProductRequest {
    pub org_id: Uuid,
    pub product: ProductInput,
}

/// Adds a product, service or bundle.
#[utoipa::path(post, path = "/api/v1/catalog/products", operation_id = "create_catalog_product",
    tag = "catalog", request_body = CreateProductRequest,
    responses(
        (status = 201, body = Product),
        (status = 409, description = "The SKU is taken there", body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn create_product(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateProductRequest>,
) -> ApiResult<(StatusCode, Json<Product>)> {
    require_module_at(&state, MODULE, body.org_id).await?;
    authed
        .require_at(&state, permissions::CATALOG_MANAGE, body.org_id)
        .await?;
    let scope = authed.scope(&state).await?;
    let ctx = authed.user.ctx.clone();
    let product = scoped(&state, &scope, async |tx| {
        catalog::create(tx, &ctx, body.org_id, &body.product).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(product)))
}

/// Changes a product (send the version you read). Retire one with
/// `active: false`.
#[utoipa::path(put, path = "/api/v1/catalog/products/{id}", operation_id = "update_catalog_product",
    tag = "catalog", params(("id" = Uuid, Path)), request_body = ProductInput,
    responses(
        (status = 200, body = Product),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn update_product(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ProductInput>,
) -> ApiResult<Json<Product>> {
    let scope = product_access(&state, &authed, id, permissions::CATALOG_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            catalog::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Deletes a product no bundle uses.
#[utoipa::path(delete, path = "/api/v1/catalog/products/{id}", operation_id = "delete_catalog_product",
    tag = "catalog", params(("id" = Uuid, Path)),
    responses(
        (status = 204),
        (status = 409, description = "A bundle uses it", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn delete_product(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let scope = product_access(&state, &authed, id, permissions::CATALOG_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        catalog::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
