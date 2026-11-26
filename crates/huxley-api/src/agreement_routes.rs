//! Agreements (design doc §8.7, ADR-039, the `contracts` module), and how
//! time falls under them.
//!
//! - `agreement:read` at an organization: its agreements, their usage, and
//!   how its tickets' time is covered.
//! - `agreement:manage`: add, change, cancel and delete them.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_contracts::agreements::{self, Agreement, AgreementInput};
use huxley_contracts::coverage::{self, Evaluation, Usage};
use huxley_contracts::{ContractsError, MODULE};
use huxley_db::{OrgScope, begin_scoped};
use serde::Deserialize;
use time::Date;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agreements", get(list_agreements).post(create_agreement))
        .route(
            "/agreements/{id}",
            get(get_agreement)
                .put(update_agreement)
                .delete(delete_agreement),
        )
        .route("/agreements/{id}/usage", get(agreement_usage))
        .route("/tickets/{id}/time-coverage", get(ticket_time_coverage))
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

/// The organization a row of `table` belongs to (404 if none).
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

/// The module is on at `org` and the caller holds `permission` there.
async fn allow(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    org: Uuid,
) -> ApiResult<OrgScope> {
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    authed.scope(state).await
}

/// Asset-type coverage needs Assets on where the agreement is.
async fn assets_on(state: &AppState, org: Uuid, input: &AgreementInput) -> ApiResult<()> {
    if !input.asset_type_ids.is_empty()
        && require_module_at(state, huxley_cmdb::MODULE, org)
            .await
            .is_err()
    {
        return Err(ApiError::validation()
            .with_field_error("/assetTypeIds", "Assets (the cmdb module) are off here"));
    }
    Ok(())
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct AgreementParams {
    /// Only this organization's.
    pub org_id: Option<Uuid>,
}

/// Agreements you can see.
#[utoipa::path(get, path = "/api/v1/agreements", operation_id = "list_agreements", tag = "agreements",
    params(AgreementParams), responses((status = 200, body = Vec<Agreement>)))]
pub async fn list_agreements(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<AgreementParams>,
) -> ApiResult<Json<Vec<Agreement>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::AGREEMENT_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            agreements::list(tx, params.org_id).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/agreements/{id}", operation_id = "get_agreement", tag = "agreements",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Agreement), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_agreement(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Agreement>> {
    let org = owner(&state, "contract_agreements", id).await?;
    let scope = allow(&state, &authed, permissions::AGREEMENT_READ, org).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| agreements::get(tx, id).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateAgreementRequest {
    pub org_id: Uuid,
    pub agreement: AgreementInput,
}

/// Adds an agreement (a draft unless `status` says otherwise).
#[utoipa::path(post, path = "/api/v1/agreements", operation_id = "create_agreement", tag = "agreements",
    request_body = CreateAgreementRequest,
    responses((status = 201, body = Agreement), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_agreement(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateAgreementRequest>,
) -> ApiResult<(StatusCode, Json<Agreement>)> {
    let scope = allow(&state, &authed, permissions::AGREEMENT_MANAGE, body.org_id).await?;
    assets_on(&state, body.org_id, &body.agreement).await?;
    let ctx = authed.user.ctx.clone();
    let agreement = scoped(&state, &scope, async |tx| {
        agreements::create(tx, &ctx, body.org_id, &body.agreement).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(agreement)))
}

/// Changes an agreement (send the version you read). Coverage, rates and
/// lines are replaced with the input's.
#[utoipa::path(put, path = "/api/v1/agreements/{id}", operation_id = "update_agreement", tag = "agreements",
    params(("id" = Uuid, Path)), request_body = AgreementInput,
    responses(
        (status = 200, body = Agreement),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn update_agreement(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<AgreementInput>,
) -> ApiResult<Json<Agreement>> {
    let org = owner(&state, "contract_agreements", id).await?;
    let scope = allow(&state, &authed, permissions::AGREEMENT_MANAGE, org).await?;
    assets_on(&state, org, &body).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            agreements::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Deletes a draft (others are cancelled instead).
#[utoipa::path(delete, path = "/api/v1/agreements/{id}", operation_id = "delete_agreement", tag = "agreements",
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_agreement(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org = owner(&state, "contract_agreements", id).await?;
    let scope = allow(&state, &authed, permissions::AGREEMENT_MANAGE, org).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        agreements::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UsageParams {
    /// `YYYY-MM-DD` (default: the start of this month).
    pub from: Option<String>,
    /// `YYYY-MM-DD` (default: the end of this month).
    pub to: Option<String>,
    /// Only time on approved timesheets (default: the timesheet setting
    /// "bill only approved time").
    #[serde(rename = "approvedOnly")]
    pub approved_only: Option<bool>,
}

fn day(field: &str, value: Option<&str>, default: Date) -> ApiResult<Date> {
    let format = time::macros::format_description!("[year]-[month]-[day]");
    match value {
        None | Some("") => Ok(default),
        Some(v) => Date::parse(v, &format)
            .map_err(|_| ApiError::validation().with_field_error(field, "a date, YYYY-MM-DD")),
    }
}

/// The time that fell under an agreement between two days (this month by
/// default), entry by entry, with totals.
#[utoipa::path(get, path = "/api/v1/agreements/{id}/usage", operation_id = "agreement_usage", tag = "agreements",
    params(("id" = Uuid, Path), UsageParams), responses((status = 200, body = Usage)))]
pub async fn agreement_usage(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(params): Query<UsageParams>,
) -> ApiResult<Json<Usage>> {
    let org = owner(&state, "contract_agreements", id).await?;
    allow(&state, &authed, permissions::AGREEMENT_READ, org).await?;
    let today = time::OffsetDateTime::now_utc().date();
    let first = today.replace_day(1).unwrap_or(today);
    let last = first
        .replace_day(time::util::days_in_month(first.month(), first.year()))
        .unwrap_or(today);
    let from = day("from", params.from.as_deref(), first)?;
    let to = day("to", params.to.as_deref(), last)?;
    if to < from || (to - from).whole_days() > 400 {
        return Err(
            ApiError::validation().with_field_error("to", "on or after from, within 400 days")
        );
    }
    // Evaluation looks across organizations (agreements above, allowances).
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            let approved_only = match params.approved_only {
                Some(only) => only,
                None => {
                    sqlx::query_scalar(
                        "SELECT bill_approved_only FROM wf_timesheet_settings WHERE id = 1",
                    )
                    .fetch_one(&mut *tx)
                    .await?
                }
            };
            coverage::usage(tx, id, from, to, approved_only).await
        })
        .await?,
    ))
}

/// How a ticket's time falls under agreements, entry by entry.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/time-coverage", operation_id = "ticket_time_coverage",
    tag = "agreements", params(("id" = Uuid, Path)), responses((status = 200, body = Vec<Evaluation>)))]
pub async fn ticket_time_coverage(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Evaluation>>> {
    let org = owner(&state, "sd_tickets", id).await?;
    allow(&state, &authed, permissions::AGREEMENT_READ, org).await?;
    authed
        .require_at(&state, permissions::TICKET_READ, org)
        .await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            coverage::for_ticket(tx, id).await
        })
        .await?,
    ))
}
