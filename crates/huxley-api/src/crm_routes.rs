//! The `crm` module's API (design doc §8.1, ADR-035): company details,
//! sites and contacts. Every route checks that the module is active at the
//! organization concerned (404 otherwise), then `crm:read` or `crm:manage`
//! there.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_crm::companies::{self, Company, CompanyInput, CompanySummary};
use huxley_crm::contacts::{self, Contact, ContactCompany, ContactInput, ContactPage};
use huxley_crm::sites::{self, Site, SiteInput};
use huxley_crm::{CrmError, MODULE};
use huxley_db::{OrgScope, begin_scoped};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module_at;
use crate::{ApiError, ApiResult, AppState};

/// Routes under `/api/v1/orgs/{id}`.
pub fn org_routes() -> Router<AppState> {
    Router::new()
        .route("/{id}/company", get(get_company).put(update_company))
        .route("/{id}/sites", get(list_sites).post(create_site))
        .route("/{id}/contacts", get(list_contacts).post(create_contact))
}

/// `/api/v1/companies`, `/api/v1/sites` and `/api/v1/contacts`.
pub fn record_routes() -> Router<AppState> {
    Router::new()
        .route("/companies", get(list_companies))
        .route(
            "/sites/{id}",
            get(get_site).put(update_site).delete(archive_site),
        )
        .route(
            "/contacts/{id}",
            get(get_contact).put(update_contact).delete(archive_contact),
        )
        .route("/contacts/{id}/companies", put(set_contact_companies))
}

fn map(e: CrmError) -> ApiError {
    match e {
        CrmError::NotFound => ApiError::not_found(),
        CrmError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        CrmError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        CrmError::Db(e) => ApiError::internal(&e),
        CrmError::Sqlx(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// The module is on at `org_id` and the caller holds `permission` there.
async fn allow(state: &AppState, authed: &Authed, permission: &str, org_id: Uuid) -> ApiResult<()> {
    require_module_at(state, MODULE, org_id).await?;
    authed.require_at(state, permission, org_id).await
}

/// The organization a site or contact belongs to (404 if there's none).
async fn owner(state: &AppState, table: &str, id: Uuid) -> ApiResult<Uuid> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT org_id FROM {table} WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    org.ok_or_else(ApiError::not_found)
}

/// Runs `f` in a transaction scoped to what the caller can see.
async fn scoped<T>(
    state: &AppState,
    authed: &Authed,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, CrmError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(value)
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct CompanyListParams {
    /// Matches names.
    pub search: Option<String>,
    /// Include provider organizations too (default: clients only).
    #[serde(default)]
    pub providers: bool,
}

/// Companies you can see with `crm:read`, by name (at most 500), with
/// their details and how many contacts and sites they have. Organizations
/// where the module is turned off are left out.
#[utoipa::path(get, path = "/api/v1/companies", tag = "crm", params(CompanyListParams),
    responses((status = 200, body = Vec<CompanySummary>), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn list_companies(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<CompanyListParams>,
) -> ApiResult<Json<Vec<CompanySummary>>> {
    crate::module_routes::require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::CRM_READ, MODULE)
        .await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let companies = companies::list(&mut tx, params.search.as_deref(), params.providers)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(companies))
}

/// An organization's company details.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/company", tag = "crm",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Company), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_company(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Company>> {
    allow(&state, &authed, permissions::CRM_READ, id).await?;
    Ok(Json(
        scoped(&state, &authed, async |tx| companies::get(tx, id).await).await?,
    ))
}

/// Saves an organization's company details. Send back the `version` you
/// read (0 the first time).
#[utoipa::path(put, path = "/api/v1/orgs/{id}/company", tag = "crm",
    params(("id" = Uuid, Path)), request_body = CompanyInput,
    responses((status = 200, body = Company), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_company(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CompanyInput>,
) -> ApiResult<Json<Company>> {
    allow(&state, &authed, permissions::CRM_MANAGE, id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &authed, async |tx| {
            companies::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedParams {
    #[serde(default)]
    pub include_archived: bool,
}

/// An organization's sites, the primary one first.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/sites", tag = "crm",
    params(("id" = Uuid, Path), ArchivedParams),
    responses((status = 200, body = Vec<Site>)))]
pub async fn list_sites(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(params): Query<ArchivedParams>,
) -> ApiResult<Json<Vec<Site>>> {
    allow(&state, &authed, permissions::CRM_READ, id).await?;
    Ok(Json(
        scoped(&state, &authed, async |tx| {
            sites::list(tx, id, params.include_archived).await
        })
        .await?,
    ))
}

/// Adds a site. An organization's first site is its primary one.
#[utoipa::path(post, path = "/api/v1/orgs/{id}/sites", tag = "crm",
    params(("id" = Uuid, Path)), request_body = SiteInput,
    responses((status = 201, body = Site), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_site(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SiteInput>,
) -> ApiResult<(StatusCode, Json<Site>)> {
    allow(&state, &authed, permissions::CRM_MANAGE, id).await?;
    let ctx = authed.user.ctx.clone();
    let site = scoped(&state, &authed, async |tx| {
        sites::create(tx, &ctx, id, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(site)))
}

#[utoipa::path(get, path = "/api/v1/sites/{id}", tag = "crm",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Site), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_site(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Site>> {
    let org = owner(&state, "crm_sites", id).await?;
    allow(&state, &authed, permissions::CRM_READ, org).await?;
    Ok(Json(
        scoped(&state, &authed, async |tx| sites::get(tx, id).await).await?,
    ))
}

/// Changes a site. Send back the `version` you read.
#[utoipa::path(put, path = "/api/v1/sites/{id}", tag = "crm",
    params(("id" = Uuid, Path)), request_body = SiteInput,
    responses((status = 200, body = Site), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_site(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SiteInput>,
) -> ApiResult<Json<Site>> {
    let org = owner(&state, "crm_sites", id).await?;
    allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &authed, async |tx| {
            sites::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives a site.
#[utoipa::path(delete, path = "/api/v1/sites/{id}", tag = "crm",
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_site(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org = owner(&state, "crm_sites", id).await?;
    allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &authed, async |tx| {
        sites::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ContactListParams {
    /// Matches names and email addresses.
    pub search: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
    /// 1 to 200 (default 50).
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Contacts of an organization (whether it's their primary company or
/// not), by name.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/contacts", tag = "crm",
    params(("id" = Uuid, Path), ContactListParams),
    responses((status = 200, body = ContactPage)))]
pub async fn list_contacts(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(params): Query<ContactListParams>,
) -> ApiResult<Json<ContactPage>> {
    allow(&state, &authed, permissions::CRM_READ, id).await?;
    Ok(Json(
        scoped(&state, &authed, async |tx| {
            contacts::list(
                tx,
                id,
                params.search.as_deref(),
                params.include_archived,
                params.limit.unwrap_or(50),
                params.offset.unwrap_or(0),
            )
            .await
        })
        .await?,
    ))
}

/// Adds a contact whose primary company is this organization.
#[utoipa::path(post, path = "/api/v1/orgs/{id}/contacts", tag = "crm",
    params(("id" = Uuid, Path)), request_body = ContactInput,
    responses((status = 201, body = Contact), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_contact(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ContactInput>,
) -> ApiResult<(StatusCode, Json<Contact>)> {
    allow(&state, &authed, permissions::CRM_MANAGE, id).await?;
    let ctx = authed.user.ctx.clone();
    let contact = scoped(&state, &authed, async |tx| {
        contacts::create(tx, &ctx, id, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(contact)))
}

#[utoipa::path(get, path = "/api/v1/contacts/{id}", tag = "crm",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Contact), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_contact(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Contact>> {
    let org = owner(&state, "crm_contacts", id).await?;
    allow(&state, &authed, permissions::CRM_READ, org).await?;
    Ok(Json(
        scoped(&state, &authed, async |tx| contacts::get(tx, id).await).await?,
    ))
}

/// Changes a contact's details. Send back the `version` you read.
#[utoipa::path(put, path = "/api/v1/contacts/{id}", tag = "crm",
    params(("id" = Uuid, Path)), request_body = ContactInput,
    responses((status = 200, body = Contact), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_contact(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ContactInput>,
) -> ApiResult<Json<Contact>> {
    let org = owner(&state, "crm_contacts", id).await?;
    allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &authed, async |tx| {
            contacts::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives a contact.
#[utoipa::path(delete, path = "/api/v1/contacts/{id}", tag = "crm",
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_contact(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org = owner(&state, "crm_contacts", id).await?;
    allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &authed, async |tx| {
        contacts::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A contact's companies.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContactCompaniesInput {
    /// Every company, exactly one of them primary.
    pub companies: Vec<ContactCompany>,
    /// The contact's version you read.
    pub version: i32,
}

/// Sets a contact's companies; the primary one becomes the company it
/// belongs to. Needs `crm:manage` at every company involved, before and
/// after.
#[utoipa::path(put, path = "/api/v1/contacts/{id}/companies", tag = "crm",
    params(("id" = Uuid, Path)), request_body = ContactCompaniesInput,
    responses((status = 200, body = Contact), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_contact_companies(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ContactCompaniesInput>,
) -> ApiResult<Json<Contact>> {
    let org = owner(&state, "crm_contacts", id).await?;
    allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let current = scoped(&state, &authed, async |tx| contacts::get(tx, id).await).await?;
    let mut involved: Vec<Uuid> = current.companies.iter().map(|c| c.org_id).collect();
    involved.extend(body.companies.iter().map(|c| c.org_id));
    involved.sort();
    involved.dedup();
    for org_id in involved {
        allow(&state, &authed, permissions::CRM_MANAGE, org_id).await?;
    }
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &authed, async |tx| {
            contacts::set_companies(tx, &ctx, id, body.version, &body.companies).await
        })
        .await?,
    ))
}
