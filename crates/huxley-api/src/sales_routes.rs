//! The sales pipeline (design doc §8.10, ADR-063).
//!
//! - `sales:read` at an organization: its leads and opportunities.
//! - `sales:write`: capture and work leads, convert them (with `crm:manage`
//!   where the lead is, as it makes a client and a contact), and create,
//!   change and move opportunities.
//! - `sales:configure` (instance): stages and win/loss reasons. Anyone with
//!   `sales:read` somewhere can list them.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_sales::leads::{self, ConvertInput, Converted, Lead, LeadInput};
use huxley_sales::opportunities::{self, MoveInput, Opportunity, OpportunityInput, StageChange};
use huxley_sales::stages::{self, Reason, ReasonInput, Stage, StageInput};
use huxley_sales::{MODULE, SalesError};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

/// Stages and reasons under `/sales`, then `/leads` and `/opportunities`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/sales", stage_routes())
        .nest("/leads", lead_routes())
        .nest("/opportunities", opportunity_routes())
        .nest("/quotes", crate::quote_routes::routes())
}

fn stage_routes() -> Router<AppState> {
    Router::new()
        .route("/stages", get(list_stages).post(create_stage))
        .route("/stages/order", put(reorder_stages))
        .route("/stages/{id}", put(update_stage).delete(archive_stage))
        .route("/reasons", get(list_reasons).post(create_reason))
        .route("/reasons/{id}", axum::routing::delete(archive_reason))
}

fn lead_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_leads).post(create_lead))
        .route("/{id}", get(get_lead).put(update_lead).delete(delete_lead))
        .route("/{id}/disqualify", post(disqualify_lead))
        .route("/{id}/convert", post(convert_lead))
}

fn opportunity_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_opportunities).post(create_opportunity))
        .route(
            "/{id}",
            get(get_opportunity)
                .put(update_opportunity)
                .delete(delete_opportunity),
        )
        .route("/{id}/move", post(move_opportunity))
        .route("/{id}/history", get(opportunity_history))
}

fn map(e: SalesError) -> ApiError {
    match e {
        SalesError::NotFound => ApiError::not_found(),
        SalesError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        SalesError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        SalesError::Conflict(message) => ApiError::conflict(message),
        SalesError::Org(e) => crate::authz::map_org(e),
        SalesError::Crm(huxley_crm::CrmError::Invalid { field, message }) => {
            ApiError::validation().with_field_error(format!("/contact/{field}"), message)
        }
        SalesError::Crm(e) => ApiError::internal(&e),
        SalesError::Db(e) => ApiError::internal(&e),
        SalesError::Sqlx(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

pub(crate) async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, SalesError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope).await.map_err(db_err)?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(value)
}

/// Where a record is (read in system scope), for checking access there.
pub(crate) async fn org_of(
    state: &AppState,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<Option<Uuid>, SalesError>,
) -> ApiResult<Uuid> {
    scoped(state, &OrgScope::System, f)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub(crate) async fn access_at(
    state: &AppState,
    authed: &Authed,
    org: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    authed.scope(state).await
}

// ---- stages and reasons ----

/// Someone who works with sales somewhere.
async fn reader(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    if authed.principal.can_instance(permissions::SALES_CONFIGURE) {
        return Ok(());
    }
    let scope = authed
        .module_scope(state, permissions::SALES_READ, MODULE)
        .await?;
    match scope {
        OrgScope::Orgs(orgs) if orgs.is_empty() => Err(ApiError::forbidden()),
        _ => Ok(()),
    }
}

async fn configurer(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    authed.require_instance(permissions::SALES_CONFIGURE)
}

/// The pipeline's stages, in order.
#[utoipa::path(get, path = "/api/v1/sales/stages", operation_id = "list_sales_stages", tag = "sales",
    responses((status = 200, body = Vec<Stage>)))]
pub async fn list_stages(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Stage>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| stages::list(tx).await).await?,
    ))
}

/// Adds a stage at the end.
#[utoipa::path(post, path = "/api/v1/sales/stages", operation_id = "create_sales_stage", tag = "sales",
    request_body = StageInput,
    responses((status = 201, body = Stage), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_stage(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<StageInput>,
) -> ApiResult<(StatusCode, Json<Stage>)> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    let created = scoped(&state, &OrgScope::System, async |tx| {
        stages::create(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

#[utoipa::path(put, path = "/api/v1/sales/stages/{id}", operation_id = "update_sales_stage", tag = "sales",
    params(("id" = Uuid, Path)), request_body = StageInput,
    responses((status = 200, body = Stage), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_stage(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<StageInput>,
) -> ApiResult<Json<Stage>> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            stages::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives a stage no open opportunity is at.
#[utoipa::path(delete, path = "/api/v1/sales/stages/{id}", operation_id = "archive_sales_stage", tag = "sales",
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_stage(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        stages::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageOrder {
    /// Every stage, in the new order.
    pub ids: Vec<Uuid>,
}

#[utoipa::path(put, path = "/api/v1/sales/stages/order", operation_id = "reorder_sales_stages", tag = "sales",
    request_body = StageOrder, responses((status = 200, body = Vec<Stage>)))]
pub async fn reorder_stages(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<StageOrder>,
) -> ApiResult<Json<Vec<Stage>>> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            stages::reorder(tx, &ctx, &body.ids).await
        })
        .await?,
    ))
}

/// Why opportunities are won and lost.
#[utoipa::path(get, path = "/api/v1/sales/reasons", operation_id = "list_sales_reasons", tag = "sales",
    responses((status = 200, body = Vec<Reason>)))]
pub async fn list_reasons(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Reason>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            stages::reasons(tx).await
        })
        .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/sales/reasons", operation_id = "create_sales_reason", tag = "sales",
    request_body = ReasonInput, responses((status = 201, body = Reason)))]
pub async fn create_reason(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<ReasonInput>,
) -> ApiResult<(StatusCode, Json<Reason>)> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    let reason = scoped(&state, &OrgScope::System, async |tx| {
        stages::add_reason(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(reason)))
}

#[utoipa::path(delete, path = "/api/v1/sales/reasons/{id}", operation_id = "archive_sales_reason", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn archive_reason(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        stages::archive_reason(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- leads ----

/// Leads you can see, newest first.
#[utoipa::path(get, path = "/api/v1/leads", operation_id = "list_leads", tag = "sales",
    params(leads::Filter), responses((status = 200, body = Vec<Lead>)))]
pub async fn list_leads(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<leads::Filter>,
) -> ApiResult<Json<Vec<Lead>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::SALES_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| leads::list(tx, &filter).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateLeadRequest {
    /// The provider organization capturing it.
    pub org_id: Uuid,
    pub lead: LeadInput,
}

/// Captures a lead.
#[utoipa::path(post, path = "/api/v1/leads", operation_id = "create_lead", tag = "sales",
    request_body = CreateLeadRequest,
    responses((status = 201, body = Lead), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_lead(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateLeadRequest>,
) -> ApiResult<(StatusCode, Json<Lead>)> {
    let scope = access_at(&state, &authed, body.org_id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let lead = scoped(&state, &scope, async |tx| {
        leads::create(tx, &ctx, body.org_id, &body.lead).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(lead)))
}

async fn lead_access(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let org = org_of(state, async |tx| leads::org_of(tx, id).await).await?;
    access_at(state, authed, org, permission).await
}

#[utoipa::path(get, path = "/api/v1/leads/{id}", operation_id = "get_lead", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Lead)))]
pub async fn get_lead(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Lead>> {
    let scope = lead_access(&state, &authed, id, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| leads::get(tx, id).await).await?,
    ))
}

/// Changes a lead (send the version you read).
#[utoipa::path(put, path = "/api/v1/leads/{id}", operation_id = "update_lead", tag = "sales",
    params(("id" = Uuid, Path)), request_body = LeadInput,
    responses((status = 200, body = Lead), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_lead(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<LeadInput>,
) -> ApiResult<Json<Lead>> {
    let scope = lead_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            leads::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/leads/{id}", operation_id = "delete_lead", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 204), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_lead(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let scope = lead_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| leads::delete(tx, &ctx, id).await).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisqualifyRequest {
    pub reason: String,
    pub version: i32,
}

/// Marks a lead not worth pursuing.
#[utoipa::path(post, path = "/api/v1/leads/{id}/disqualify", operation_id = "disqualify_lead", tag = "sales",
    params(("id" = Uuid, Path)), request_body = DisqualifyRequest, responses((status = 200, body = Lead)))]
pub async fn disqualify_lead(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<DisqualifyRequest>,
) -> ApiResult<Json<Lead>> {
    let scope = lead_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            leads::disqualify(tx, &ctx, id, &body.reason, body.version).await
        })
        .await?,
    ))
}

/// Converts a lead into a client (new or existing), a contact and an
/// opportunity. Needs `crm:manage` where the lead is, and at an existing
/// client given.
#[utoipa::path(post, path = "/api/v1/leads/{id}/convert", operation_id = "convert_lead", tag = "sales",
    params(("id" = Uuid, Path)), request_body = ConvertInput,
    responses((status = 200, body = Converted), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn convert_lead(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ConvertInput>,
) -> ApiResult<Json<Converted>> {
    let org = org_of(&state, async |tx| leads::org_of(tx, id).await).await?;
    access_at(&state, &authed, org, permissions::SALES_WRITE).await?;
    authed
        .require_at(&state, permissions::CRM_MANAGE, org)
        .await?;
    if let Some(client) = body.org_id {
        require_module_at(&state, MODULE, client).await?;
        authed
            .require_at(&state, permissions::SALES_WRITE, client)
            .await?;
    }
    let scope = authed.scope(&state).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            leads::convert(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

// ---- opportunities ----

/// Opportunities you can see: open ones by close date, then closed ones.
#[utoipa::path(get, path = "/api/v1/opportunities", operation_id = "list_opportunities", tag = "sales",
    params(opportunities::Filter), responses((status = 200, body = Vec<Opportunity>)))]
pub async fn list_opportunities(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<opportunities::Filter>,
) -> ApiResult<Json<Vec<Opportunity>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::SALES_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            opportunities::list(tx, &filter).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateOpportunityRequest {
    /// The client it would sell to.
    pub org_id: Uuid,
    pub opportunity: OpportunityInput,
}

#[utoipa::path(post, path = "/api/v1/opportunities", operation_id = "create_opportunity", tag = "sales",
    request_body = CreateOpportunityRequest,
    responses((status = 201, body = Opportunity), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_opportunity(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateOpportunityRequest>,
) -> ApiResult<(StatusCode, Json<Opportunity>)> {
    let scope = access_at(&state, &authed, body.org_id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let o = scoped(&state, &scope, async |tx| {
        opportunities::create(tx, &ctx, body.org_id, &body.opportunity, None).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(o)))
}

async fn opportunity_access(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let org = org_of(state, async |tx| opportunities::org_of(tx, id).await).await?;
    access_at(state, authed, org, permission).await
}

#[utoipa::path(get, path = "/api/v1/opportunities/{id}", operation_id = "get_opportunity", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Opportunity)))]
pub async fn get_opportunity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Opportunity>> {
    let scope = opportunity_access(&state, &authed, id, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| opportunities::get(tx, id).await).await?,
    ))
}

/// Changes an opportunity's details (send the version you read).
#[utoipa::path(put, path = "/api/v1/opportunities/{id}", operation_id = "update_opportunity", tag = "sales",
    params(("id" = Uuid, Path)), request_body = OpportunityInput,
    responses((status = 200, body = Opportunity), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_opportunity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<OpportunityInput>,
) -> ApiResult<Json<Opportunity>> {
    let scope = opportunity_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            opportunities::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/opportunities/{id}", operation_id = "delete_opportunity", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_opportunity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let scope = opportunity_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        opportunities::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Moves an opportunity to a stage; a won or lost stage closes it.
#[utoipa::path(post, path = "/api/v1/opportunities/{id}/move", operation_id = "move_opportunity", tag = "sales",
    params(("id" = Uuid, Path)), request_body = MoveInput,
    responses((status = 200, body = Opportunity), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn move_opportunity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<MoveInput>,
) -> ApiResult<Json<Opportunity>> {
    let scope = opportunity_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            opportunities::move_to(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Its moves between stages, oldest first.
#[utoipa::path(get, path = "/api/v1/opportunities/{id}/history", operation_id = "opportunity_history", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<StageChange>)))]
pub async fn opportunity_history(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<StageChange>>> {
    let scope = opportunity_access(&state, &authed, id, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            opportunities::history(tx, id).await
        })
        .await?,
    ))
}
