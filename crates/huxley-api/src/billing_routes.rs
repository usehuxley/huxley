//! Invoicing (ADR-075): billing settings, what's unbilled, billing runs,
//! and invoices (the billing module).
//!
//! - `billing:read` at a client: its invoices; at a provider: what's
//!   unbilled below it and its runs.
//! - `billing:write`: run billing, edit, hold and void drafts, mark sent,
//!   make credit memos. `billing:approve`: approve. `billing:configure`:
//!   settings.

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_billing::generate::{self, Run, RunInput, Unbilled};
use huxley_billing::invoices::{self, Filter, Invoice, InvoiceInput};
use huxley_billing::settings::{self, Settings};
use huxley_billing::{BillingError, MODULE};
use huxley_db::{OrgScope, begin_scoped};
use rust_decimal::Decimal;
use serde::Deserialize;
use time::Date;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/billing/settings", get(get_settings).put(put_settings))
        .route("/billing/unbilled", get(get_unbilled))
        .route("/billing/runs", get(list_runs).post(start_run))
        .route("/invoices", get(list_invoices))
        .route("/invoices/{id}", get(get_invoice).put(update_invoice))
        .route("/invoices/{id}/hold", post(hold_invoice))
        .route("/invoices/{id}/approve", post(approve_invoice))
        .route("/invoices/{id}/sent", post(sent_invoice))
        .route("/invoices/{id}/void", post(void_invoice))
        .route("/invoices/{id}/credit", post(credit_invoice))
        .route("/invoices/{id}/pdf", get(invoice_pdf))
}

fn map(e: BillingError) -> ApiError {
    match e {
        BillingError::NotFound => ApiError::not_found(),
        BillingError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        BillingError::Conflict(message) => ApiError::conflict(message),
        BillingError::Db(e) => ApiError::internal(&e),
        BillingError::Sqlx(e) => ApiError::internal(&e),
        BillingError::Contracts(e) => ApiError::internal(&e),
    }
}

async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, BillingError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(value)
}

async fn access_at(
    state: &AppState,
    authed: &Authed,
    org: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    authed.scope(state).await
}

async fn at_invoice(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let org = scoped(state, &OrgScope::System, async |tx| {
        invoices::org_of(tx, id).await
    })
    .await?
    .ok_or_else(ApiError::not_found)?;
    access_at(state, authed, org, permission).await
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct OrgParams {
    pub org_id: Uuid,
}

/// The provider's billing settings.
#[utoipa::path(get, path = "/api/v1/billing/settings", operation_id = "get_billing_settings", tag = "billing",
    params(OrgParams), responses((status = 200, body = Settings)))]
pub async fn get_settings(
    State(state): State<AppState>,
    authed: Authed,
    Query(p): Query<OrgParams>,
) -> ApiResult<Json<Settings>> {
    let scope = access_at(&state, &authed, p.org_id, permissions::BILLING_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| settings::get(tx, p.org_id).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsRequest {
    pub org_id: Uuid,
    #[serde(flatten)]
    pub settings: Settings,
}

#[utoipa::path(put, path = "/api/v1/billing/settings", operation_id = "put_billing_settings", tag = "billing",
    request_body = SettingsRequest, responses((status = 200, body = Settings)))]
pub async fn put_settings(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<SettingsRequest>,
) -> ApiResult<Json<Settings>> {
    let scope = access_at(&state, &authed, body.org_id, permissions::BILLING_CONFIGURE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            settings::set(tx, &ctx, body.org_id, &body.settings).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct PeriodParams {
    /// The provider.
    pub org_id: Uuid,
    #[param(value_type = String, format = Date)]
    pub from: String,
    #[param(value_type = String, format = Date)]
    pub to: String,
}

fn date(field: &str, s: &str) -> ApiResult<Date> {
    Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).map_err(|_| {
        ApiError::validation().with_field_error(format!("/{field}"), "a date, YYYY-MM-DD")
    })
}

/// What's billable below a provider in a period, per client.
#[utoipa::path(get, path = "/api/v1/billing/unbilled", operation_id = "get_unbilled", tag = "billing",
    params(PeriodParams), responses((status = 200, body = Vec<Unbilled>)))]
pub async fn get_unbilled(
    State(state): State<AppState>,
    authed: Authed,
    Query(p): Query<PeriodParams>,
) -> ApiResult<Json<Vec<Unbilled>>> {
    access_at(&state, &authed, p.org_id, permissions::BILLING_READ).await?;
    let (from, to) = (date("from", &p.from)?, date("to", &p.to)?);
    // Coverage, rates and catalog prices span the provider's tree.
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            generate::unbilled(tx, p.org_id, from, to, None).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/billing/runs", operation_id = "list_billing_runs", tag = "billing",
    responses((status = 200, body = Vec<Run>)))]
pub async fn list_runs(State(state): State<AppState>, authed: Authed) -> ApiResult<Json<Vec<Run>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::BILLING_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| generate::runs(tx).await).await?,
    ))
}

/// Runs billing: draft invoices for what's billable in the period.
#[utoipa::path(post, path = "/api/v1/billing/runs", operation_id = "start_billing_run", tag = "billing",
    request_body = RunInput, responses((status = 201, body = Run)))]
pub async fn start_run(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<RunInput>,
) -> ApiResult<(StatusCode, Json<Run>)> {
    access_at(&state, &authed, body.org_id, permissions::BILLING_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let run = scoped(&state, &OrgScope::System, async |tx| {
        generate::run(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(run)))
}

/// Invoices and credit memos you can see, newest first.
#[utoipa::path(get, path = "/api/v1/invoices", operation_id = "list_invoices", tag = "billing",
    params(Filter), responses((status = 200, body = Vec<Invoice>)))]
pub async fn list_invoices(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<Filter>,
) -> ApiResult<Json<Vec<Invoice>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::BILLING_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| invoices::list(tx, &filter).await).await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/invoices/{id}", operation_id = "get_invoice", tag = "billing",
    params(("id" = Uuid, Path)), responses((status = 200, body = Invoice)))]
pub async fn get_invoice(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Invoice>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| invoices::get(tx, id).await).await?,
    ))
}

/// Changes a draft's lines, notes and due date (send the version you read).
#[utoipa::path(put, path = "/api/v1/invoices/{id}", operation_id = "update_invoice", tag = "billing",
    params(("id" = Uuid, Path)), request_body = InvoiceInput, responses((status = 200, body = Invoice)))]
pub async fn update_invoice(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<InvoiceInput>,
) -> ApiResult<Json<Invoice>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            invoices::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HoldRequest {
    pub on_hold: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

#[utoipa::path(post, path = "/api/v1/invoices/{id}/hold", operation_id = "hold_invoice", tag = "billing",
    params(("id" = Uuid, Path)), request_body = HoldRequest, responses((status = 200, body = Invoice)))]
pub async fn hold_invoice(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<HoldRequest>,
) -> ApiResult<Json<Invoice>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            invoices::hold(tx, &ctx, id, body.on_hold, body.reason.as_deref()).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionRequest {
    pub version: i32,
}

/// Approves a draft: it can then be sent or synced.
#[utoipa::path(post, path = "/api/v1/invoices/{id}/approve", operation_id = "approve_invoice", tag = "billing",
    params(("id" = Uuid, Path)), request_body = VersionRequest, responses((status = 200, body = Invoice)))]
pub async fn approve_invoice(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<VersionRequest>,
) -> ApiResult<Json<Invoice>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_APPROVE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            invoices::approve(tx, &ctx, id, body.version).await
        })
        .await?,
    ))
}

/// Marks an approved invoice sent (it's then locked).
#[utoipa::path(post, path = "/api/v1/invoices/{id}/sent", operation_id = "mark_invoice_sent", tag = "billing",
    params(("id" = Uuid, Path)), responses((status = 200, body = Invoice)))]
pub async fn sent_invoice(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Invoice>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            invoices::mark_sent(tx, &ctx, id).await
        })
        .await?,
    ))
}

/// Voids a draft or approved invoice: what it billed can be billed again.
#[utoipa::path(post, path = "/api/v1/invoices/{id}/void", operation_id = "void_invoice", tag = "billing",
    params(("id" = Uuid, Path)), request_body = VersionRequest, responses((status = 200, body = Invoice)))]
pub async fn void_invoice(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<VersionRequest>,
) -> ApiResult<Json<Invoice>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            invoices::void(tx, &ctx, id, body.version).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreditRequest {
    /// The lines to credit (default: all).
    #[serde(default)]
    pub line_ids: Vec<Uuid>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Makes a credit memo draft against an invoice that's been sent.
#[utoipa::path(post, path = "/api/v1/invoices/{id}/credit", operation_id = "credit_invoice", tag = "billing",
    params(("id" = Uuid, Path)), request_body = CreditRequest, responses((status = 201, body = Invoice)))]
pub async fn credit_invoice(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CreditRequest>,
) -> ApiResult<(StatusCode, Json<Invoice>)> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let memo = scoped(&state, &scope, async |tx| {
        invoices::credit(tx, &ctx, id, &body.line_ids, body.reason.as_deref()).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(memo)))
}

/// The invoice (or credit memo) as a PDF.
#[utoipa::path(get, path = "/api/v1/invoices/{id}/pdf", operation_id = "invoice_pdf", tag = "billing",
    params(("id" = Uuid, Path)), responses((status = 200, content_type = "application/pdf", body = Vec<u8>)))]
pub async fn invoice_pdf(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_READ).await?;
    let i = scoped(&state, &scope, async |tx| invoices::get(tx, id).await).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"INV-{}.pdf\"", i.number),
            ),
        ],
        render_pdf(&i),
    ))
}

fn render_pdf(i: &Invoice) -> Vec<u8> {
    use huxley_analytics::export::{Cell, Table, pdf};
    let money = |d: Decimal| format!("{} {}", i.currency, d.round_dp(2));
    let rows = i
        .lines
        .iter()
        .map(|l| {
            vec![
                Cell::Text(l.description.clone()),
                Cell::Number(format!("{} {}", l.quantity.normalize(), l.unit)),
                Cell::Number(money(l.unit_price)),
                Cell::Number(money(l.amount)),
            ]
        })
        .collect();
    let what = if i.kind == "credit_memo" {
        "Credit memo"
    } else {
        "Invoice"
    };
    let tables = vec![
        Table {
            title: what.into(),
            columns: ["Description", "Quantity", "Price", "Amount"]
                .map(String::from)
                .to_vec(),
            rows,
            note: None,
        },
        Table {
            title: "Total".into(),
            columns: vec![String::new(), String::new()],
            rows: vec![vec![
                Cell::Text("Total".into()),
                Cell::Number(money(i.total)),
            ]],
            note: i.notes.clone(),
        },
    ];
    let mut sub = vec![i.org_name.clone()];
    if let (Some(a), Some(b)) = (i.period_start, i.period_end) {
        sub.push(format!("{a} to {b}"));
    }
    if let Some(d) = i.due_on {
        sub.push(format!("due {d}"));
    }
    pdf(
        &format!("{what} INV-{}", i.number),
        &sub.join(" · "),
        &tables,
    )
}
