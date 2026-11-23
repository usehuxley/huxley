//! Custom fields and tags: definitions (`/custom-fields`, `/tags`) and their
//! use on records (`/entities/{type}/{id}/fields`, `.../tags`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_entity::custom_fields::{self, FieldChanges, FieldDef, FieldValue, NewField};
use huxley_entity::tags::{self, Tag, TagInput};
use huxley_entity::{EntityError, EntityType};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/entity-types", get(entity_types))
        .route("/custom-fields", get(list_fields).post(create_field))
        .route("/custom-fields/{id}", patch(update_field))
        .route("/custom-fields/{id}/archive", post(archive_field))
        .route("/tags", get(list_tags).post(create_tag))
        .route("/tags/{id}", put(update_tag).delete(delete_tag))
        .route(
            "/entities/{entity_type}/{id}/fields",
            get(get_values).patch(set_values),
        )
        .route("/entities/{entity_type}/{id}/tags", get(entity_tags))
        .route(
            "/entities/{entity_type}/{id}/tags/{tag_id}",
            put(apply_tag).delete(remove_tag),
        )
}

fn map_entity(e: EntityError) -> ApiError {
    match e {
        EntityError::UnknownEntityType(_) | EntityError::NotFound => ApiError::not_found(),
        EntityError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        EntityError::KeyTaken => ApiError::conflict("A field with this key already exists."),
        EntityError::TagNameTaken => ApiError::conflict("A tag with this name already exists."),
        EntityError::Db(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// The permissions that guard reading and changing a record of this type.
fn guard(entity_type: &str) -> ApiResult<(&'static str, &'static str)> {
    match entity_type {
        huxley_entity::ORGANIZATION => Ok((permissions::ORG_READ, permissions::ORG_UPDATE)),
        _ => Err(ApiError::not_found()),
    }
}

/// Checks `permission` on the record's owning org (404 if it's invisible).
async fn require_on(
    state: &AppState,
    authed: &Authed,
    entity_type: &str,
    id: Uuid,
    write: bool,
) -> ApiResult<()> {
    let (read, update) = guard(entity_type)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (org_id, _) = huxley_entity::owning_org(&mut tx, entity_type, id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    authed
        .require_at(state, if write { update } else { read }, org_id)
        .await
}

/// Record types that support custom fields and tags.
#[utoipa::path(get, path = "/api/v1/entity-types", tag = "fields",
    responses((status = 200, body = Vec<EntityType>)))]
pub async fn entity_types(_authed: Authed) -> Json<&'static [EntityType]> {
    Json(huxley_entity::TYPES)
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct FieldListParams {
    pub entity_type: String,
    #[serde(default)]
    pub include_archived: bool,
}

/// Custom field definitions for a record type.
#[utoipa::path(get, path = "/api/v1/custom-fields", tag = "fields", params(FieldListParams),
    responses((status = 200, body = Vec<FieldDef>)))]
pub async fn list_fields(
    State(state): State<AppState>,
    _authed: Authed,
    Query(params): Query<FieldListParams>,
) -> ApiResult<Json<Vec<FieldDef>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let defs = custom_fields::list_defs(&mut tx, &params.entity_type, params.include_archived)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(defs))
}

#[utoipa::path(post, path = "/api/v1/custom-fields", tag = "fields", request_body = NewField,
    responses((status = 201, body = FieldDef), (status = 409, body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_field(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<NewField>,
) -> ApiResult<(StatusCode, Json<FieldDef>)> {
    authed.require_instance(permissions::CUSTOM_FIELD_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let def = custom_fields::create_def(&mut tx, &authed.user.ctx, &body)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(def)))
}

/// Changes a field's label, help, options, required flag, scope, or order.
/// Its key and type can't change.
#[utoipa::path(patch, path = "/api/v1/custom-fields/{id}", tag = "fields",
    params(("id" = Uuid, Path, description = "Field id")), request_body = FieldChanges,
    responses((status = 200, body = FieldDef), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_field(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<FieldChanges>,
) -> ApiResult<Json<FieldDef>> {
    authed.require_instance(permissions::CUSTOM_FIELD_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let def = custom_fields::update_def(&mut tx, &authed.user.ctx, id, &body)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(def))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ArchiveRequest {
    pub archived: bool,
}

/// Archives (hides) or restores a field. Stored values are kept.
#[utoipa::path(post, path = "/api/v1/custom-fields/{id}/archive", tag = "fields",
    params(("id" = Uuid, Path, description = "Field id")), request_body = ArchiveRequest,
    responses((status = 200, body = FieldDef)))]
pub async fn archive_field(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ArchiveRequest>,
) -> ApiResult<Json<FieldDef>> {
    authed.require_instance(permissions::CUSTOM_FIELD_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let def = custom_fields::set_archived(&mut tx, &authed.user.ctx, id, body.archived)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(def))
}

#[utoipa::path(get, path = "/api/v1/tags", tag = "fields", responses((status = 200, body = Vec<Tag>)))]
pub async fn list_tags(
    State(state): State<AppState>,
    _authed: Authed,
) -> ApiResult<Json<Vec<Tag>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let list = tags::list(&mut tx).await.map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(list))
}

#[utoipa::path(post, path = "/api/v1/tags", tag = "fields", request_body = TagInput,
    responses((status = 201, body = Tag), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_tag(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<TagInput>,
) -> ApiResult<(StatusCode, Json<Tag>)> {
    authed.require_instance(permissions::TAG_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let tag = tags::create(&mut tx, &authed.user.ctx, &body)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(tag)))
}

#[utoipa::path(put, path = "/api/v1/tags/{id}", tag = "fields",
    params(("id" = Uuid, Path, description = "Tag id")), request_body = TagInput,
    responses((status = 200, body = Tag)))]
pub async fn update_tag(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<TagInput>,
) -> ApiResult<Json<Tag>> {
    authed.require_instance(permissions::TAG_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let tag = tags::update(&mut tx, &authed.user.ctx, id, &body)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(tag))
}

/// Deletes a tag and removes it from every record.
#[utoipa::path(delete, path = "/api/v1/tags/{id}", tag = "fields",
    params(("id" = Uuid, Path, description = "Tag id")), responses((status = 204)))]
pub async fn delete_tag(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::TAG_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    tags::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The custom fields that apply to a record, with values.
#[utoipa::path(get, path = "/api/v1/entities/{entity_type}/{id}/fields", tag = "fields",
    params(("entity_type" = String, Path, description = "e.g. organization"), ("id" = Uuid, Path, description = "Record id")),
    responses((status = 200, body = Vec<FieldValue>), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_values(
    State(state): State<AppState>,
    authed: Authed,
    Path((entity_type, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Vec<FieldValue>>> {
    require_on(&state, &authed, &entity_type, id, false).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let values = custom_fields::values(&mut tx, &entity_type, id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(values))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetValuesRequest {
    /// Field key → value. `null` clears a value. Other fields are unchanged.
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub values: serde_json::Map<String, serde_json::Value>,
}

/// Sets custom field values on a record.
#[utoipa::path(patch, path = "/api/v1/entities/{entity_type}/{id}/fields", tag = "fields",
    params(("entity_type" = String, Path, description = "e.g. organization"), ("id" = Uuid, Path, description = "Record id")),
    request_body = SetValuesRequest,
    responses((status = 200, body = Vec<FieldValue>), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_values(
    State(state): State<AppState>,
    authed: Authed,
    Path((entity_type, id)): Path<(String, Uuid)>,
    Json(body): Json<SetValuesRequest>,
) -> ApiResult<Json<Vec<FieldValue>>> {
    require_on(&state, &authed, &entity_type, id, true).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let values =
        custom_fields::set_values(&mut tx, &authed.user.ctx, &entity_type, id, &body.values)
            .await
            .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(values))
}

#[utoipa::path(get, path = "/api/v1/entities/{entity_type}/{id}/tags", tag = "fields",
    params(("entity_type" = String, Path, description = "e.g. organization"), ("id" = Uuid, Path, description = "Record id")),
    responses((status = 200, body = Vec<Tag>)))]
pub async fn entity_tags(
    State(state): State<AppState>,
    authed: Authed,
    Path((entity_type, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Vec<Tag>>> {
    require_on(&state, &authed, &entity_type, id, false).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let list = tags::of_entity(&mut tx, &entity_type, id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(list))
}

#[utoipa::path(put, path = "/api/v1/entities/{entity_type}/{id}/tags/{tag_id}", tag = "fields",
    params(("entity_type" = String, Path, description = "e.g. organization"), ("id" = Uuid, Path, description = "Record id"),
        ("tag_id" = Uuid, Path, description = "Tag id")),
    responses((status = 204)))]
pub async fn apply_tag(
    State(state): State<AppState>,
    authed: Authed,
    Path((entity_type, id, tag_id)): Path<(String, Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    require_on(&state, &authed, &entity_type, id, true).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    tags::apply(&mut tx, &authed.user.ctx, &entity_type, id, tag_id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/entities/{entity_type}/{id}/tags/{tag_id}", tag = "fields",
    params(("entity_type" = String, Path, description = "e.g. organization"), ("id" = Uuid, Path, description = "Record id"),
        ("tag_id" = Uuid, Path, description = "Tag id")),
    responses((status = 204)))]
pub async fn remove_tag(
    State(state): State<AppState>,
    authed: Authed,
    Path((entity_type, id, tag_id)): Path<(String, Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    require_on(&state, &authed, &entity_type, id, true).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    tags::remove(&mut tx, &authed.user.ctx, &entity_type, id, tag_id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
