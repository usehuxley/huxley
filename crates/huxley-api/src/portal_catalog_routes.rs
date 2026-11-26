//! The portal's request catalog (ADR-040): forms offered to a company's
//! contacts, and what they've asked for. Submitting runs the form's
//! workflows as each workflow's run-as account, with the contact and their
//! company as `trigger.context`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_db::{OrgScope, begin_scoped};
use huxley_forms::portal::{self, CatalogForm, Request};
use huxley_forms::{Submission, Submitter};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::form_routes::map_err;
use crate::portal_routes::PortalAuthed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/catalog", get(list_catalog))
        .route("/catalog/{id}", get(get_catalog_form))
        .route("/catalog/{id}/requests", post(submit_request))
        .route("/requests", get(list_requests))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// What you can ask for: forms offered to your company's contacts.
#[utoipa::path(get, path = "/api/portal/v1/catalog", operation_id = "portal_catalog", tag = "portal",
    responses((status = 200, body = Vec<CatalogForm>)))]
pub async fn list_catalog(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Vec<CatalogForm>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let forms = portal::catalog(&mut tx, authed.principal.org_id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(forms))
}

#[utoipa::path(get, path = "/api/portal/v1/catalog/{id}", operation_id = "portal_catalog_form", tag = "portal",
    params(("id" = Uuid, Path)), responses((status = 200, body = CatalogForm)))]
pub async fn get_catalog_form(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<CatalogForm>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let form = portal::get(&mut tx, authed.principal.org_id, id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(form))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalRequestInput {
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub answers: JsonValue,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalRequestCreated {
    /// The request (submission).
    pub id: Uuid,
}

/// Asks for something with a form: starts its workflows for your company.
#[utoipa::path(post, path = "/api/portal/v1/catalog/{id}/requests", operation_id = "portal_submit_request",
    tag = "portal", params(("id" = Uuid, Path)), request_body = PortalRequestInput,
    responses(
        (status = 201, body = PortalRequestCreated),
        (status = 422, description = "The answers don't fit", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn submit_request(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
    Json(body): Json<PortalRequestInput>,
) -> ApiResult<(StatusCode, Json<PortalRequestCreated>)> {
    let p = &authed.principal;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    portal::get(&mut tx, p.org_id, id).await.map_err(map_err)?;
    let kind: String = sqlx::query_scalar("SELECT kind::text FROM organizations WHERE id = $1")
        .bind(p.org_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
    let contact = huxley_crm::contacts::get(&mut tx, p.contact_id)
        .await
        .map_err(db_err)?;
    let context = json!({
        "client": { "id": p.org_id, "name": p.org_name, "kind": kind },
        "contact": {
            "id": contact.id, "name": contact.name, "first_name": contact.first_name,
            "last_name": contact.last_name, "email": contact.email, "phone": contact.phone,
            "mobile": contact.mobile, "external_ids": contact.external_ids,
        },
        "portal": { "account": p.account_id, "role": p.role },
    });
    let submitted = huxley_forms::submit(
        &mut tx,
        &state.engine,
        &authed.ctx,
        id,
        p.org_id,
        Submitter::Portal(p.account_id),
        Submission {
            answers: body.answers,
            context: Some(context),
        },
    )
    .await
    .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        Json(PortalRequestCreated {
            id: submitted.submission_id,
        }),
    ))
}

/// What you've asked for, newest first.
#[utoipa::path(get, path = "/api/portal/v1/requests", operation_id = "portal_requests", tag = "portal",
    responses((status = 200, body = Vec<Request>)))]
pub async fn list_requests(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Vec<Request>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let requests = portal::requests(&mut tx, authed.principal.account_id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(requests))
}
