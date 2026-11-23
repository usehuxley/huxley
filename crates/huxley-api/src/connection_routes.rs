//! `/api/v1/connections`: credentials for calling integrations (design doc
//! §7.2). Secrets are write-only: responses say which are set, never what
//! they are.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_integrations::connections::{self, ConnectionError, ConnectionInput, ConnectionView};
use huxley_integrations::store::{self, StoreError};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map_err(e: ConnectionError) -> ApiError {
    match e {
        ConnectionError::NotFound | ConnectionError::Store(StoreError::NotFound) => {
            ApiError::not_found()
        }
        ConnectionError::KeyTaken(_) => ApiError::conflict(e.to_string()),
        ConnectionError::Invalid(field, message) => {
            ApiError::validation().with_field_error(field, message)
        }
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

/// The connection, checking `permission` at its org (404 if not visible).
async fn authorize(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    id: Uuid,
) -> ApiResult<ConnectionView> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let view = connections::get(&mut tx, id).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    authed.require_at(state, permission, view.org_id).await?;
    Ok(view)
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ConnectionQuery {
    /// Only connections to this integration.
    pub integration_id: Option<Uuid>,
}

/// Connections the caller can see.
#[utoipa::path(get, path = "/api/v1/connections", operation_id = "list_connections", tag = "integrations",
    params(ConnectionQuery), responses((status = 200, body = Vec<ConnectionView>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<ConnectionQuery>,
) -> ApiResult<Json<Vec<ConnectionView>>> {
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let all = connections::list(&mut tx, query.integration_id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    let mut visible = Vec::new();
    for view in all {
        if let Some(path) = org_path(&state, view.org_id).await?
            && authed
                .principal
                .can_at_path(permissions::CONNECTION_READ, &path)
        {
            visible.push(view);
        }
    }
    Ok(Json(visible))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateConnectionRequest {
    pub org_id: Uuid,
    pub integration_id: Uuid,
    pub connection: ConnectionInput,
}

/// Creates a connection. The integration must belong to the connection's
/// organization or one above it.
#[utoipa::path(post, path = "/api/v1/connections", operation_id = "create_connection", tag = "integrations",
    request_body = CreateConnectionRequest,
    responses((status = 201, body = ConnectionView),
              (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateConnectionRequest>,
) -> ApiResult<(StatusCode, Json<ConnectionView>)> {
    authed
        .require_at(&state, permissions::CONNECTION_MANAGE, body.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let integration = store::summary(&mut tx, body.integration_id)
        .await
        .map_err(|e| map_err(e.into()))?;
    tx.commit().await.map_err(db_err)?;
    let (Some(connection_path), Some(integration_path)) = (
        org_path(&state, body.org_id).await?,
        org_path(&state, integration.org_id).await?,
    ) else {
        return Err(ApiError::not_found());
    };
    let inherits = connection_path == integration_path
        || connection_path.starts_with(&format!("{integration_path}."));
    if !inherits {
        return Err(ApiError::validation().with_field_error(
            "integrationId",
            "the integration belongs to an organization outside this one's tree",
        ));
    }
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let view = connections::create(
        &mut tx,
        &state.crypto,
        &authed.user.ctx,
        body.org_id,
        body.integration_id,
        &body.connection,
    )
    .await
    .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(view)))
}

#[utoipa::path(get, path = "/api/v1/connections/{id}", operation_id = "get_connection", tag = "integrations",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = ConnectionView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn detail(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ConnectionView>> {
    Ok(Json(
        authorize(&state, &authed, permissions::CONNECTION_READ, id).await?,
    ))
}

/// Changes a connection. Secret fields left out keep their value.
#[utoipa::path(put, path = "/api/v1/connections/{id}", operation_id = "update_connection", tag = "integrations",
    params(("id" = Uuid, Path)), request_body = ConnectionInput,
    responses((status = 200, body = ConnectionView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ConnectionInput>,
) -> ApiResult<Json<ConnectionView>> {
    authorize(&state, &authed, permissions::CONNECTION_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let view = connections::update(&mut tx, &state.crypto, &authed.user.ctx, id, &body)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(view))
}

#[utoipa::path(delete, path = "/api/v1/connections/{id}", operation_id = "delete_connection", tag = "integrations",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn remove(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize(&state, &authed, permissions::CONNECTION_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    connections::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
