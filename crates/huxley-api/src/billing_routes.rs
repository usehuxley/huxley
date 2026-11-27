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
use huxley_billing::review::{
    self, Comment, CommentInput, ReviewState, Rule, RuleInput, RunApproval,
};
use huxley_billing::settings::{self, Settings};
use huxley_billing::sync::{
    self, Mapping, MappingInput, MappingKind, QueueFilter, RemoteOption, Settings as SyncSettings,
    Sync,
};
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
        .route(
            "/invoices/{id}/review",
            get(invoice_review).post(submit_review),
        )
        .route(
            "/invoices/{id}/comments",
            get(invoice_comments).post(add_comment),
        )
        .route(
            "/invoices/{id}/comments/{comment}/resolve",
            post(resolve_comment),
        )
        .route("/billing/runs/{id}/approve", post(approve_run))
        .route(
            "/billing/sync-settings",
            get(get_sync_settings).put(put_sync_settings),
        )
        .route("/billing/mappings", get(list_mappings).put(put_mapping))
        .route("/billing/mappings/options", get(mapping_options))
        .route(
            "/billing/mappings/{id}",
            axum::routing::delete(delete_mapping),
        )
        .route("/billing/sync-queue", get(sync_queue))
        .route("/invoices/{id}/sync", get(invoice_sync).post(request_sync))
        .route("/billing/review-rules", get(list_rules).post(create_rule))
        .route(
            "/billing/review-rules/{id}",
            axum::routing::put(update_rule).delete(delete_rule),
        )
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

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApproveRequest {
    pub version: i32,
    /// Approve one that needs review without an approved review
    /// (`billing:review:override`; it's recorded).
    #[serde(default)]
    pub override_review: bool,
}

/// Approves a draft: it can then be sent or synced. One that needs review
/// takes an approved review of this revision first (409 otherwise), unless
/// sent with `overrideReview`.
#[utoipa::path(post, path = "/api/v1/invoices/{id}/approve", operation_id = "approve_invoice", tag = "billing",
    params(("id" = Uuid, Path)), request_body = ApproveRequest,
    responses((status = 200, body = Invoice), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn approve_invoice(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ApproveRequest>,
) -> ApiResult<Json<Invoice>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_APPROVE).await?;
    if body.override_review {
        at_invoice(&state, &authed, id, permissions::BILLING_OVERRIDE).await?;
    }
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            invoices::approve(tx, &ctx, id, body.version, body.override_review).await
        })
        .await?,
    ))
}

/// Its review state, with the open approval for the caller to decide.
#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceReviewView {
    #[serde(flatten)]
    pub state: ReviewState,
    /// The review workflow's open approval, if any.
    pub approval_id: Option<Uuid>,
    /// The caller is one of its approvers.
    pub can_decide: bool,
    /// The caller may approve it without an approved review.
    pub can_override: bool,
}

async fn review_view(
    state: &AppState,
    authed: &Authed,
    scope: &OrgScope,
    id: Uuid,
) -> ApiResult<InvoiceReviewView> {
    let user = authed.user.auth.user.id;
    let (review, open) = scoped(state, scope, async |tx| {
        let review = review::state(tx, id).await?;
        let open = review::open_approval(tx, id, user).await?;
        Ok((review, open))
    })
    .await?;
    let can_override = at_invoice(state, authed, id, permissions::BILLING_OVERRIDE)
        .await
        .is_ok();
    Ok(InvoiceReviewView {
        state: review,
        approval_id: open.map(|(a, _)| a),
        can_decide: open.is_some_and(|(_, me)| me),
        can_override,
    })
}

#[utoipa::path(get, path = "/api/v1/invoices/{id}/review", operation_id = "invoice_review", tag = "billing",
    params(("id" = Uuid, Path)), responses((status = 200, body = InvoiceReviewView)))]
pub async fn invoice_review(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<InvoiceReviewView>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_READ).await?;
    Ok(Json(review_view(&state, &authed, &scope, id).await?))
}

/// Submits a draft for review (send the version you read). Raises
/// `invoice.submitted_for_review` for the review workflow (the Invoice
/// review pack).
#[utoipa::path(post, path = "/api/v1/invoices/{id}/review", operation_id = "submit_invoice_review", tag = "billing",
    params(("id" = Uuid, Path)), request_body = VersionRequest,
    responses((status = 200, body = InvoiceReviewView), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn submit_review(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<VersionRequest>,
) -> ApiResult<Json<InvoiceReviewView>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        review::submit(tx, &ctx, id, Some(body.version)).await
    })
    .await?;
    Ok(Json(review_view(&state, &authed, &scope, id).await?))
}

/// The invoice's comments, oldest first (replies point at their thread).
#[utoipa::path(get, path = "/api/v1/invoices/{id}/comments", operation_id = "invoice_comments", tag = "billing",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<Comment>)))]
pub async fn invoice_comments(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Comment>>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| review::comments(tx, id).await).await?,
    ))
}

/// Comments on the invoice or one of its lines, telling anyone @mentioned.
#[utoipa::path(post, path = "/api/v1/invoices/{id}/comments", operation_id = "add_invoice_comment", tag = "billing",
    params(("id" = Uuid, Path)), request_body = CommentInput,
    responses((status = 201, body = Comment), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_comment(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CommentInput>,
) -> ApiResult<(StatusCode, Json<Comment>)> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_READ).await?;
    let ctx = authed.user.ctx.clone();
    let c = scoped(&state, &scope, async |tx| {
        review::comment(tx, &ctx, id, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(c)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveCommentRequest {
    pub resolved: bool,
}

/// Resolves a comment thread, or opens it again.
#[utoipa::path(post, path = "/api/v1/invoices/{id}/comments/{comment}/resolve", operation_id = "resolve_invoice_comment",
    tag = "billing", params(("id" = Uuid, Path), ("comment" = Uuid, Path)), request_body = ResolveCommentRequest,
    responses((status = 204)))]
pub async fn resolve_comment(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, comment)): Path<(Uuid, Uuid)>,
    Json(body): Json<ResolveCommentRequest>,
) -> ApiResult<StatusCode> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_READ).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        review::resolve(tx, &ctx, id, comment, body.resolved).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApproveRunRequest {
    /// Submit those that need a review (and aren't in one) for review.
    #[serde(default)]
    pub submit_reviews: bool,
}

/// Approves a billing run's drafts in one pass; those on hold or needing
/// review are returned as exceptions.
#[utoipa::path(post, path = "/api/v1/billing/runs/{id}/approve", operation_id = "approve_billing_run", tag = "billing",
    params(("id" = Uuid, Path)), request_body = ApproveRunRequest, responses((status = 200, body = RunApproval)))]
pub async fn approve_run(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ApproveRunRequest>,
) -> ApiResult<Json<RunApproval>> {
    let run = scoped(&state, &OrgScope::System, async |tx| {
        generate::get_run(tx, id).await
    })
    .await?;
    access_at(&state, &authed, run.org_id, permissions::BILLING_APPROVE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            review::approve_run(tx, &ctx, id, body.submit_reviews).await
        })
        .await?,
    ))
}

async fn rule_reader(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    if authed
        .principal
        .can_instance(permissions::BILLING_CONFIGURE)
    {
        return Ok(());
    }
    match authed
        .module_scope(state, permissions::BILLING_READ, MODULE)
        .await?
    {
        OrgScope::Orgs(orgs) if orgs.is_empty() => Err(ApiError::forbidden()),
        _ => Ok(()),
    }
}

async fn rule_writer(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    authed.require_instance(permissions::BILLING_CONFIGURE)
}

/// When invoices need review, and who reviews them.
#[utoipa::path(get, path = "/api/v1/billing/review-rules", operation_id = "list_invoice_review_rules", tag = "billing",
    responses((status = 200, body = Vec<Rule>)))]
pub async fn list_rules(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Rule>>> {
    rule_reader(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            review::rules(tx).await
        })
        .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/billing/review-rules", operation_id = "create_invoice_review_rule", tag = "billing",
    request_body = RuleInput, responses((status = 201, body = Rule), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_rule(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<RuleInput>,
) -> ApiResult<(StatusCode, Json<Rule>)> {
    rule_writer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    let rule = scoped(&state, &OrgScope::System, async |tx| {
        review::create_rule(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

#[utoipa::path(put, path = "/api/v1/billing/review-rules/{id}", operation_id = "update_invoice_review_rule", tag = "billing",
    params(("id" = Uuid, Path)), request_body = RuleInput, responses((status = 200, body = Rule)))]
pub async fn update_rule(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<RuleInput>,
) -> ApiResult<Json<Rule>> {
    rule_writer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            review::update_rule(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/billing/review-rules/{id}", operation_id = "delete_invoice_review_rule", tag = "billing",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_rule(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    rule_writer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        review::delete_rule(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
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

// ---- accounting sync (ADR-077) ----

/// The provider's accounting sync settings (none until it's set up).
#[utoipa::path(get, path = "/api/v1/billing/sync-settings", operation_id = "get_accounting_sync_settings", tag = "billing",
    params(OrgParams), responses((status = 200, body = Option<SyncSettings>)))]
pub async fn get_sync_settings(
    State(state): State<AppState>,
    authed: Authed,
    Query(p): Query<OrgParams>,
) -> ApiResult<Json<Option<SyncSettings>>> {
    let scope = access_at(&state, &authed, p.org_id, permissions::BILLING_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            sync::settings(tx, p.org_id).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncSettingsRequest {
    pub org_id: Uuid,
    pub target: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub on_approval: bool,
}

const fn yes() -> bool {
    true
}

#[utoipa::path(put, path = "/api/v1/billing/sync-settings", operation_id = "put_accounting_sync_settings", tag = "billing",
    request_body = SyncSettingsRequest, responses((status = 200, body = SyncSettings)))]
pub async fn put_sync_settings(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<SyncSettingsRequest>,
) -> ApiResult<Json<SyncSettings>> {
    let scope = access_at(&state, &authed, body.org_id, permissions::BILLING_CONFIGURE).await?;
    let ctx = authed.user.ctx.clone();
    let s = SyncSettings {
        target: body.target,
        enabled: body.enabled,
        on_approval: body.on_approval,
    };
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            sync::set_settings(tx, &ctx, body.org_id, &s).await
        })
        .await?,
    ))
}

/// The provider's sync target (409 when it has none).
async fn target_of(state: &AppState, scope: &OrgScope, provider: Uuid) -> ApiResult<String> {
    scoped(state, scope, async |tx| sync::settings(tx, provider).await)
        .await?
        .map(|s| s.target)
        .ok_or_else(|| {
            ApiError::conflict("no accounting system is set up: choose one in Settings → Billing")
        })
}

/// The provider's mappings to its accounting system.
#[utoipa::path(get, path = "/api/v1/billing/mappings", operation_id = "list_accounting_mappings", tag = "billing",
    params(OrgParams), responses((status = 200, body = Vec<Mapping>)))]
pub async fn list_mappings(
    State(state): State<AppState>,
    authed: Authed,
    Query(p): Query<OrgParams>,
) -> ApiResult<Json<Vec<Mapping>>> {
    let scope = access_at(&state, &authed, p.org_id, permissions::BILLING_READ).await?;
    let target = target_of(&state, &scope, p.org_id).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            sync::mappings(tx, p.org_id, &target).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct OptionsParams {
    pub org_id: Uuid,
    pub kind: MappingKind,
}

/// The accounting system's customers, items, tax codes or classes, from the
/// sync pack's datasets.
#[utoipa::path(get, path = "/api/v1/billing/mappings/options", operation_id = "accounting_mapping_options", tag = "billing",
    params(OptionsParams), responses((status = 200, body = Vec<RemoteOption>)))]
pub async fn mapping_options(
    State(state): State<AppState>,
    authed: Authed,
    Query(p): Query<OptionsParams>,
) -> ApiResult<Json<Vec<RemoteOption>>> {
    let scope = access_at(&state, &authed, p.org_id, permissions::BILLING_READ).await?;
    let target = target_of(&state, &scope, p.org_id).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            sync::remote_options(tx, p.org_id, &target, p.kind).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MappingRequest {
    pub org_id: Uuid,
    pub mapping: MappingInput,
}

/// Matches something of Huxley's to the accounting system's (replacing any match).
#[utoipa::path(put, path = "/api/v1/billing/mappings", operation_id = "put_accounting_mapping", tag = "billing",
    request_body = MappingRequest, responses((status = 200, body = Mapping)))]
pub async fn put_mapping(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<MappingRequest>,
) -> ApiResult<Json<Mapping>> {
    let scope = access_at(&state, &authed, body.org_id, permissions::BILLING_CONFIGURE).await?;
    let target = target_of(&state, &scope, body.org_id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            sync::set_mapping(tx, &ctx, body.org_id, &target, &body.mapping).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/billing/mappings/{id}", operation_id = "delete_accounting_mapping", tag = "billing",
    params(("id" = Uuid, Path), OrgParams), responses((status = 204)))]
pub async fn delete_mapping(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(p): Query<OrgParams>,
) -> ApiResult<StatusCode> {
    let scope = access_at(&state, &authed, p.org_id, permissions::BILLING_CONFIGURE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        sync::delete_mapping(tx, &ctx, p.org_id, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Invoice syncs in a status: the error queue (failed) by default.
#[utoipa::path(get, path = "/api/v1/billing/sync-queue", operation_id = "accounting_sync_queue", tag = "billing",
    params(QueueFilter), responses((status = 200, body = Vec<Sync>)))]
pub async fn sync_queue(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<QueueFilter>,
) -> ApiResult<Json<Vec<Sync>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::BILLING_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| sync::queue(tx, &filter).await).await?,
    ))
}

/// The invoice's accounting sync (none if it was never asked for).
#[utoipa::path(get, path = "/api/v1/invoices/{id}/sync", operation_id = "invoice_sync", tag = "billing",
    params(("id" = Uuid, Path)), responses((status = 200, body = Option<Sync>)))]
pub async fn invoice_sync(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Option<Sync>>> {
    let scope = at_invoice(&state, &authed, id, permissions::BILLING_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| sync::state(tx, id).await).await?,
    ))
}

/// Syncs an approved invoice to the accounting system, or tries again.
/// Raises `invoice.sync_requested` for the sync pack's workflow; an unmapped
/// client or item fails it straight away, with the reason.
#[utoipa::path(post, path = "/api/v1/invoices/{id}/sync", operation_id = "request_invoice_sync", tag = "billing",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Sync), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn request_sync(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Sync>> {
    at_invoice(&state, &authed, id, permissions::BILLING_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    // The provider's settings and mappings sit above the client.
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            sync::request(tx, &ctx, id).await
        })
        .await?,
    ))
}
