//! `/api/v1/timesheets` (design doc §8.15, the `workforce` module).
//!
//! - `timesheet:submit`: see and submit your own, and withdraw one you
//!   submitted.
//! - `timesheet:approve`: review (approve, reject, reopen) the timesheets
//!   of people in teams you lead. With `workforce:manage`, also those of
//!   people no one leads.
//! - `workforce:manage`: the settings, and the payroll export.

use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_workforce::MODULE;
use huxley_workforce::timesheets::{self, Settings, Timesheet, TimesheetSummary};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module;
use crate::workforce_routes::system;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_timesheets))
        .route("/mine", get(my_timesheet))
        .route("/submit", post(submit_timesheet))
        .route("/settings", get(get_settings).put(set_settings))
        .route("/payroll.csv", get(payroll_export))
        .route("/{id}", get(get_timesheet))
        .route("/{id}/decision", post(decide_timesheet))
        .route("/{id}/reopen", post(reopen_timesheet))
}

fn today() -> time::Date {
    time::OffsetDateTime::now_utc().date()
}

async fn on(state: &AppState) -> ApiResult<()> {
    require_module(state, MODULE).await
}

fn me(authed: &Authed) -> Uuid {
    authed.user.auth.user.id
}

fn manages(authed: &Authed) -> bool {
    authed.principal.can_instance(permissions::WORKFORCE_MANAGE)
}

/// Whether the caller reviews `person`'s timesheets.
async fn reviews(state: &AppState, authed: &Authed, person: Uuid) -> ApiResult<bool> {
    if !authed
        .principal
        .can_instance(permissions::TIMESHEET_APPROVE)
        && !manages(authed)
    {
        return Ok(false);
    }
    let (approver, manager) = (me(authed), manages(authed));
    let leads_or_manages = authed
        .principal
        .can_instance(permissions::TIMESHEET_APPROVE)
        || manager;
    Ok(leads_or_manages
        && system(state, async |tx| {
            timesheets::can_review(tx, approver, person, manager).await
        })
        .await?)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MineParams {
    /// A day in the period (`YYYY-MM-DD`; default today).
    pub date: Option<String>,
}

/// Your timesheet for the period containing a day.
#[utoipa::path(get, path = "/api/v1/timesheets/mine", operation_id = "my_timesheet", tag = "timesheets",
    params(MineParams), responses((status = 200, body = Timesheet)))]
pub async fn my_timesheet(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<MineParams>,
) -> ApiResult<Json<Timesheet>> {
    on(&state).await?;
    authed.require_instance(permissions::TIMESHEET_SUBMIT)?;
    let user = me(&authed);
    Ok(Json(
        system(&state, async |tx| {
            let day = match params.date.as_deref().filter(|d| !d.is_empty()) {
                Some(d) => timesheets::parse_day("date", d)?,
                None => today(),
            };
            timesheets::for_day(tx, user, day).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitTimesheetRequest {
    /// The period's first day (`YYYY-MM-DD`).
    pub period_start: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Submits your timesheet for a period, for review.
#[utoipa::path(post, path = "/api/v1/timesheets/submit", operation_id = "submit_timesheet", tag = "timesheets",
    request_body = SubmitTimesheetRequest,
    responses((status = 200, body = Timesheet), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn submit_timesheet(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<SubmitTimesheetRequest>,
) -> ApiResult<Json<Timesheet>> {
    on(&state).await?;
    authed.require_instance(permissions::TIMESHEET_SUBMIT)?;
    let (user, ctx) = (me(&authed), authed.user.ctx.clone());
    Ok(Json(
        system(&state, async |tx| {
            let start = timesheets::parse_day("periodStart", &body.period_start)?;
            timesheets::submit(tx, &ctx, user, start, body.note.as_deref(), today()).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListParams {
    /// `submitted` (the default), `approved`, `rejected` or `open`.
    pub status: Option<String>,
}

/// Timesheets you review, oldest first (submitted ones by default).
#[utoipa::path(get, path = "/api/v1/timesheets", operation_id = "list_timesheets", tag = "timesheets",
    params(ListParams), responses((status = 200, body = Vec<TimesheetSummary>)))]
pub async fn list_timesheets(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<TimesheetSummary>>> {
    on(&state).await?;
    let approve = authed
        .principal
        .can_instance(permissions::TIMESHEET_APPROVE);
    let manager = manages(&authed);
    if !approve && !manager {
        return Err(ApiError::forbidden());
    }
    let approver = me(&authed);
    let status = params.status.unwrap_or_else(|| "submitted".into());
    Ok(Json(
        system(&state, async |tx| {
            let all = timesheets::list(tx, Some(&status), None).await?;
            let mut mine = Vec::new();
            for s in all {
                if timesheets::can_review(tx, approver, s.user_id, manager).await? {
                    mine.push(s);
                }
            }
            Ok(mine)
        })
        .await?,
    ))
}

/// A timesheet: yours, or one you review.
#[utoipa::path(get, path = "/api/v1/timesheets/{id}", operation_id = "get_timesheet", tag = "timesheets",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Timesheet), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_timesheet(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Timesheet>> {
    on(&state).await?;
    let sheet = system(&state, async |tx| timesheets::get(tx, id).await).await?;
    if sheet.user_id != me(&authed) && !reviews(&state, &authed, sheet.user_id).await? {
        return Err(ApiError::not_found());
    }
    Ok(Json(sheet))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TimesheetDecision {
    pub approve: bool,
    /// Needed to reject: what needs changing.
    #[serde(default)]
    pub note: Option<String>,
}

/// Approves (locking its entries) or rejects a submitted timesheet.
#[utoipa::path(post, path = "/api/v1/timesheets/{id}/decision", operation_id = "decide_timesheet", tag = "timesheets",
    params(("id" = Uuid, Path)), request_body = TimesheetDecision,
    responses((status = 200, body = Timesheet), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn decide_timesheet(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<TimesheetDecision>,
) -> ApiResult<Json<Timesheet>> {
    on(&state).await?;
    let sheet = system(&state, async |tx| timesheets::get(tx, id).await).await?;
    if !reviews(&state, &authed, sheet.user_id).await? {
        return Err(ApiError::forbidden());
    }
    let (by, ctx) = (me(&authed), authed.user.ctx.clone());
    Ok(Json(
        system(&state, async |tx| {
            timesheets::decide(tx, &ctx, id, body.approve, by, body.note.as_deref()).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReopenTimesheet {
    /// Needed when a reviewer reopens one: why.
    #[serde(default)]
    pub note: Option<String>,
}

/// Reopens a timesheet: a reviewer reopens a submitted or approved one, or
/// you withdraw one you submitted. Its entries can change again.
#[utoipa::path(post, path = "/api/v1/timesheets/{id}/reopen", operation_id = "reopen_timesheet", tag = "timesheets",
    params(("id" = Uuid, Path)), request_body = ReopenTimesheet,
    responses((status = 200, body = Timesheet), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn reopen_timesheet(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ReopenTimesheet>,
) -> ApiResult<Json<Timesheet>> {
    on(&state).await?;
    let sheet = system(&state, async |tx| timesheets::get(tx, id).await).await?;
    let mine = sheet.user_id == me(&authed);
    let reviewer = !mine && reviews(&state, &authed, sheet.user_id).await?;
    if !mine && !reviewer {
        return Err(ApiError::forbidden());
    }
    let (by, ctx) = (me(&authed), authed.user.ctx.clone());
    Ok(Json(
        system(&state, async |tx| {
            timesheets::reopen(tx, &ctx, id, by, reviewer, body.note.as_deref()).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/timesheets/settings", operation_id = "get_timesheet_settings", tag = "timesheets",
    responses((status = 200, body = Settings)))]
pub async fn get_settings(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Settings>> {
    on(&state).await?;
    if !authed.principal.can_instance(permissions::TIMESHEET_SUBMIT) && !manages(&authed) {
        return Err(ApiError::forbidden());
    }
    Ok(Json(
        system(&state, async |tx| timesheets::settings(tx).await).await?,
    ))
}

/// How periods are cut, and whether billing previews use only approved
/// time.
#[utoipa::path(put, path = "/api/v1/timesheets/settings", operation_id = "set_timesheet_settings", tag = "timesheets",
    request_body = Settings, responses((status = 200, body = Settings)))]
pub async fn set_settings(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<Settings>,
) -> ApiResult<Json<Settings>> {
    on(&state).await?;
    authed.require_instance(permissions::WORKFORCE_MANAGE)?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            timesheets::set_settings(tx, &ctx, &body).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PayrollParams {
    /// `YYYY-MM-DD`.
    pub from: String,
    /// `YYYY-MM-DD`, at most 100 days after `from`.
    pub to: String,
}

fn csv_field(value: &str) -> String {
    // Quoted, and never read as a formula by spreadsheets.
    let value = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Approved time (from approved timesheets) and approved time off, as CSV:
/// person, email, date, kind, minutes, billable.
#[utoipa::path(get, path = "/api/v1/timesheets/payroll.csv", operation_id = "payroll_export", tag = "timesheets",
    params(PayrollParams), responses((status = 200, content_type = "text/csv", body = String)))]
pub async fn payroll_export(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<PayrollParams>,
) -> ApiResult<impl IntoResponse> {
    on(&state).await?;
    authed.require_instance(permissions::WORKFORCE_MANAGE)?;
    let rows = system(&state, async |tx| {
        let from = timesheets::parse_day("from", &params.from)?;
        let to = timesheets::parse_day("to", &params.to)?;
        if to < from || (to - from).whole_days() > 100 {
            return Err(huxley_workforce::WorkforceError::Invalid {
                field: "to".into(),
                message: "on or after from, within 100 days".into(),
            });
        }
        timesheets::payroll(tx, from, to).await
    })
    .await?;
    let mut csv = String::from("person,email,date,kind,minutes,billable\r\n");
    for r in rows {
        let _ = write!(
            csv,
            "{},{},{},{},{},{}\r\n",
            csv_field(&r.person),
            csv_field(&r.email),
            r.date,
            csv_field(&r.kind),
            r.minutes,
            r.billable
        );
    }
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"payroll-{}-{}.csv\"",
                    params.from, params.to
                ),
            ),
        ],
        csv,
    ))
}
