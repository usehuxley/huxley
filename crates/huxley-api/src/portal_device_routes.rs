//! Devices in the client portal (ADR-040). Client admins see their
//! company's devices; everyone sees those assigned to them. Needs the
//! `cmdb` module on for the company.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use huxley_cmdb::portal::{self, Device, Seen};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::Perm;
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::module_routes::require_module_at;
use crate::portal_routes::PortalAuthed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devices", get(list_devices))
        .route("/devices/{id}", get(get_device))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map(e: huxley_cmdb::CmdbError) -> ApiError {
    match e {
        huxley_cmdb::CmdbError::NotFound => ApiError::not_found(),
        other => ApiError::internal(&other),
    }
}

/// What the account sees: the company's devices for client admins, else
/// their own.
pub(crate) fn seen(authed: &PortalAuthed) -> Seen {
    if authed.principal.can(Perm::Assets) {
        Seen::Company
    } else {
        Seen::Own(authed.principal.contact_id)
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeviceParams {
    /// Words in the name, serial number, hostname, model or manufacturer.
    pub q: Option<String>,
}

/// Your devices (client admins: the company's), by name.
#[utoipa::path(get, path = "/api/portal/v1/devices", operation_id = "portal_devices", tag = "portal",
    params(DeviceParams), responses((status = 200, body = Vec<Device>)))]
pub async fn list_devices(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Query(params): Query<DeviceParams>,
) -> ApiResult<Json<Vec<Device>>> {
    let org = authed.principal.org_id;
    require_module_at(&state, huxley_cmdb::MODULE, org).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let devices = portal::list(&mut tx, org, seen(&authed), params.q.as_deref())
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(devices))
}

#[utoipa::path(get, path = "/api/portal/v1/devices/{id}", operation_id = "portal_device", tag = "portal",
    params(("id" = Uuid, Path)), responses((status = 200, body = Device)))]
pub async fn get_device(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Device>> {
    let org = authed.principal.org_id;
    require_module_at(&state, huxley_cmdb::MODULE, org).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let device = portal::get(&mut tx, org, seen(&authed), id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(device))
}
