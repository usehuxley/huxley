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
        EntityError::TooLarge(mb) => {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too-large", "File too large")
                .with_detail(format!("Files can be up to {mb} MB."))
        }
        EntityError::Storage(e) => ApiError::internal(&e),
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
        huxley_entity::CONTACT | huxley_entity::SITE => {
            Ok((permissions::CRM_READ, permissions::CRM_MANAGE))
        }
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
    if let Some(module) = huxley_entity::module_of(entity_type) {
        crate::module_routes::require_module_at(state, module, org_id).await?;
    }
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

pub fn attachment_routes(upload_limit: usize) -> Router<AppState> {
    Router::new()
        .route(
            "/entities/{entity_type}/{id}/attachments",
            get(list_attachments)
                .post(upload_attachment)
                .layer(axum::extract::DefaultBodyLimit::max(upload_limit)),
        )
        .route(
            "/attachments/{id}",
            axum::routing::delete(delete_attachment),
        )
        .route("/attachments/{id}/content", get(download_attachment))
}

/// A record's attachments, newest first.
#[utoipa::path(get, path = "/api/v1/entities/{entity_type}/{id}/attachments", tag = "fields",
    params(("entity_type" = String, Path, description = "e.g. organization"), ("id" = Uuid, Path, description = "Record id")),
    responses((status = 200, body = Vec<huxley_entity::attachments::Attachment>)))]
pub async fn list_attachments(
    State(state): State<AppState>,
    authed: Authed,
    Path((entity_type, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Vec<huxley_entity::attachments::Attachment>>> {
    require_on(&state, &authed, &entity_type, id, false).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let list = huxley_entity::attachments::of_entity(&mut tx, &entity_type, id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(list))
}

/// Uploads a file (multipart field `file`) to a record.
#[utoipa::path(post, path = "/api/v1/entities/{entity_type}/{id}/attachments", tag = "fields",
    params(("entity_type" = String, Path, description = "e.g. organization"), ("id" = Uuid, Path, description = "Record id")),
    request_body(content_type = "multipart/form-data", description = "A `file` part"),
    responses((status = 201, body = huxley_entity::attachments::Attachment),
        (status = 413, description = "Too large", body = ApiError, content_type = "application/problem+json")))]
pub async fn upload_attachment(
    State(state): State<AppState>,
    authed: Authed,
    Path((entity_type, id)): Path<(String, Uuid)>,
    mut multipart: axum::extract::Multipart,
) -> ApiResult<(StatusCode, Json<huxley_entity::attachments::Attachment>)> {
    require_on(&state, &authed, &entity_type, id, true).await?;
    let too_large = || {
        ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too-large", "File too large").with_detail(
            format!(
                "Files can be up to {} MB.",
                state.storage.max_upload_bytes() / (1024 * 1024)
            ),
        )
    };
    let mut file = None;
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            too_large()
        } else {
            ApiError::bad_request(e.body_text())
        }
    })? {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("file").to_owned();
            let content_type = field.content_type().map(str::to_owned);
            let data = field.bytes().await.map_err(|e| {
                if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    too_large()
                } else {
                    ApiError::bad_request(e.body_text())
                }
            })?;
            file = Some((filename, content_type, data));
            break;
        }
    }
    let (filename, content_type, data) = file.ok_or_else(|| {
        ApiError::validation().with_field_error("/file", "Choose a file to upload.")
    })?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let attachment = huxley_entity::attachments::upload(
        &mut tx,
        &state.storage,
        &authed.user.ctx,
        huxley_entity::attachments::Upload {
            entity_type: &entity_type,
            entity_id: id,
            filename: &filename,
            content_type: content_type.as_deref(),
            data,
        },
    )
    .await
    .map_err(|e| match e {
        EntityError::TooLarge(_) => too_large(),
        other => map_entity(other),
    })?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(attachment)))
}

async fn load_attachment(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    write: bool,
) -> ApiResult<huxley_entity::attachments::Attachment> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let attachment = huxley_entity::attachments::get(&mut tx, id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    require_on(
        state,
        authed,
        &attachment.entity_type,
        attachment.entity_id,
        write,
    )
    .await?;
    Ok(attachment)
}

/// Downloads a file. Always served as a download (never shown inline), with
/// only a few content types passed through, so an uploaded page or script
/// can't run in the app's origin.
#[utoipa::path(get, path = "/api/v1/attachments/{id}/content", tag = "fields",
    params(("id" = Uuid, Path, description = "Attachment id")),
    responses((status = 200, description = "The file", content_type = "application/octet-stream"),
        (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn download_attachment(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse as _;
    let attachment = load_attachment(&state, &authed, id, false).await?;
    let bytes = huxley_entity::attachments::content(&state.storage, &attachment)
        .await
        .map_err(|e| match e {
            EntityError::Storage(huxley_storage::StorageError::NotFound) => ApiError::not_found(),
            other => map_entity(other),
        })?;
    let ascii: String = attachment
        .filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .-_()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = attachment
        .filename
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b".-_".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let disposition = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    Ok((
        [
            (
                header::CONTENT_TYPE,
                huxley_entity::attachments::safe_content_type(&attachment.content_type).to_owned(),
            ),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; sandbox".to_owned(),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_owned()),
        ],
        bytes,
    )
        .into_response())
}

/// Removes a file from its record.
#[utoipa::path(delete, path = "/api/v1/attachments/{id}", tag = "fields",
    params(("id" = Uuid, Path, description = "Attachment id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_attachment(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    load_attachment(&state, &authed, id, true).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    huxley_entity::attachments::delete(&mut tx, &state.storage, &authed.user.ctx, id)
        .await
        .map_err(map_entity)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
