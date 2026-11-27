//! Expenses (ADR-079): categories and mileage rates, and entries with
//! receipts (the expenses module).
//!
//! - `expense:write` at an organization: log your own expenses there, and
//!   change your drafts. Your own entries are yours to see.
//! - `expense:read`: see everyone's there. `expense:configure` (instance):
//!   categories and mileage rates.

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_expenses::categories::{self, Category, CategoryInput};
use huxley_expenses::entries::{self, Expense, ExpenseInput, Filter};
use huxley_expenses::rates::{self, MileageRate, MileageRateInput};
use huxley_expenses::reports::{self, Comment, CommentInput, Report, ReportFilter, SubmitInput};
use huxley_expenses::{ExpenseError, MODULE};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes(upload_limit: usize) -> Router<AppState> {
    Router::new()
        .route(
            "/expense-categories",
            get(list_categories).post(create_category),
        )
        .route("/expense-categories/{id}", put(update_category))
        .route("/mileage-rates", get(list_rates).put(set_rate))
        .route("/mileage-rates/{id}", axum::routing::delete(delete_rate))
        .route("/expenses", get(list).post(create))
        .route("/expenses/{id}", get(get_one).put(update).delete(delete))
        .route(
            "/expenses/{id}/apply-reading",
            axum::routing::post(apply_reading),
        )
        .route("/expense-reports", get(list_reports).post(submit_report))
        .route("/expense-reports/{id}", get(get_report))
        .route("/expense-reports/export.csv", get(export_reports))
        .route(
            "/expense-reports/{id}/sync",
            get(report_sync).post(sync_report),
        )
        .route(
            "/expense-reports/{id}/reimbursed",
            axum::routing::post(reimburse_report),
        )
        .route(
            "/expense-reports/{id}/comments",
            get(report_comments).post(add_report_comment),
        )
        .route(
            "/expenses/{id}/receipt",
            get(receipt)
                .put(put_receipt)
                .layer(axum::extract::DefaultBodyLimit::max(upload_limit)),
        )
}

fn map(e: ExpenseError) -> ApiError {
    match e {
        ExpenseError::NotFound | ExpenseError::Storage(huxley_storage::StorageError::NotFound) => {
            ApiError::not_found()
        }
        ExpenseError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        ExpenseError::Conflict(message) => ApiError::conflict(message),
        ExpenseError::TooLarge(mb) => {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too-large", "File too large")
                .with_detail(format!("Receipts can be up to {mb} MB."))
        }
        ExpenseError::Storage(e) => ApiError::internal(&e),
        ExpenseError::Db(e) => ApiError::internal(&e),
        ExpenseError::Sqlx(e) => ApiError::internal(&e),
    }
}

pub(crate) async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, ExpenseError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(value)
}

fn me(authed: &Authed) -> Uuid {
    authed.user.auth.user.id
}

/// Checks `permission` at `org` (with the module on there).
async fn at(state: &AppState, authed: &Authed, org: Uuid, permission: &str) -> ApiResult<()> {
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await
}

/// An entry the caller may see (theirs, or with `expense:read` at its
/// organization), and whether it's theirs.
async fn visible(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<bool> {
    let (user, org) = scoped(state, &OrgScope::System, async |tx| {
        entries::owner_of(tx, id).await
    })
    .await?
    .ok_or_else(ApiError::not_found)?;
    require_module_at(state, MODULE, org).await?;
    if user == me(authed) {
        return Ok(true);
    }
    authed
        .require_at(state, permissions::EXPENSE_READ, org)
        .await
        .map_err(|_| ApiError::not_found())?;
    Ok(false)
}

/// An entry that's the caller's own, to change.
async fn own(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<()> {
    let (user, org) = scoped(state, &OrgScope::System, async |tx| {
        entries::owner_of(tx, id).await
    })
    .await?
    .ok_or_else(ApiError::not_found)?;
    if user != me(authed) {
        return Err(ApiError::not_found());
    }
    at(state, authed, org, permissions::EXPENSE_WRITE).await
}

/// Anyone who logs or sees expenses somewhere can read categories and rates.
async fn reader(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    if authed
        .principal
        .can_instance(permissions::EXPENSE_CONFIGURE)
    {
        return Ok(());
    }
    for p in [permissions::EXPENSE_WRITE, permissions::EXPENSE_READ] {
        match authed.module_scope(state, p, MODULE).await? {
            OrgScope::Orgs(orgs) if orgs.is_empty() => {}
            _ => return Ok(()),
        }
    }
    Err(ApiError::forbidden())
}

async fn configurer(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    authed.require_instance(permissions::EXPENSE_CONFIGURE)
}

#[utoipa::path(get, path = "/api/v1/expense-categories", operation_id = "list_expense_categories", tag = "expenses",
    responses((status = 200, body = Vec<Category>)))]
pub async fn list_categories(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Category>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            categories::list(tx).await
        })
        .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/expense-categories", operation_id = "create_expense_category", tag = "expenses",
    request_body = CategoryInput, responses((status = 201, body = Category)))]
pub async fn create_category(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CategoryInput>,
) -> ApiResult<(StatusCode, Json<Category>)> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    let c = scoped(&state, &OrgScope::System, async |tx| {
        categories::create(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(c)))
}

#[utoipa::path(put, path = "/api/v1/expense-categories/{id}", operation_id = "update_expense_category", tag = "expenses",
    params(("id" = Uuid, Path)), request_body = CategoryInput, responses((status = 200, body = Category)))]
pub async fn update_category(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CategoryInput>,
) -> ApiResult<Json<Category>> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            categories::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/mileage-rates", operation_id = "list_mileage_rates", tag = "expenses",
    responses((status = 200, body = Vec<MileageRate>)))]
pub async fn list_rates(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<MileageRate>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| rates::list(tx).await).await?,
    ))
}

/// Sets the mileage rate from a day (replacing one set for that day).
#[utoipa::path(put, path = "/api/v1/mileage-rates", operation_id = "set_mileage_rate", tag = "expenses",
    request_body = MileageRateInput, responses((status = 200, body = MileageRate)))]
pub async fn set_rate(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<MileageRateInput>,
) -> ApiResult<Json<MileageRate>> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            rates::set(tx, &ctx, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/mileage-rates/{id}", operation_id = "delete_mileage_rate", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_rate(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        rates::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Expenses, newest first: everyone's where you have `expense:read`;
/// otherwise just yours.
#[utoipa::path(get, path = "/api/v1/expenses", operation_id = "list_expenses", tag = "expenses",
    params(Filter), responses((status = 200, body = Vec<Expense>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
    Query(mut filter): Query<Filter>,
) -> ApiResult<Json<Vec<Expense>>> {
    require_module(&state, MODULE).await?;
    let readers = authed
        .module_scope(&state, permissions::EXPENSE_READ, MODULE)
        .await?;
    let scope = match readers {
        OrgScope::Orgs(orgs) if orgs.is_empty() => {
            filter.user_id = Some(me(&authed));
            authed
                .module_scope(&state, permissions::EXPENSE_WRITE, MODULE)
                .await?
        }
        scope => scope,
    };
    Ok(Json(
        scoped(&state, &scope, async |tx| entries::list(tx, &filter).await).await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/expenses/{id}", operation_id = "get_expense", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 200, body = Expense)))]
pub async fn get_one(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Expense>> {
    visible(&state, &authed, id).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            entries::get(tx, id).await
        })
        .await?,
    ))
}

/// Logs an expense (or mileage) of yours: against a ticket, a project (and
/// phase), a client, or your MSP.
#[utoipa::path(post, path = "/api/v1/expenses", operation_id = "create_expense", tag = "expenses",
    request_body = ExpenseInput, responses((status = 201, body = Expense)))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<ExpenseInput>,
) -> ApiResult<(StatusCode, Json<Expense>)> {
    let (org, ..) = scoped(&state, &OrgScope::System, async |tx| {
        entries::placement(tx, &body).await
    })
    .await?;
    at(&state, &authed, org, permissions::EXPENSE_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let user = me(&authed);
    let e = scoped(&state, &OrgScope::System, async |tx| {
        entries::create(tx, &ctx, user, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(e)))
}

/// Changes a draft of yours (send the version you read).
#[utoipa::path(put, path = "/api/v1/expenses/{id}", operation_id = "update_expense", tag = "expenses",
    params(("id" = Uuid, Path)), request_body = ExpenseInput, responses((status = 200, body = Expense)))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ExpenseInput>,
) -> ApiResult<Json<Expense>> {
    own(&state, &authed, id).await?;
    let (org, ..) = scoped(&state, &OrgScope::System, async |tx| {
        entries::placement(tx, &body).await
    })
    .await?;
    at(&state, &authed, org, permissions::EXPENSE_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            entries::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/expenses/{id}", operation_id = "delete_expense", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    own(&state, &authed, id).await?;
    let ctx = authed.user.ctx.clone();
    let storage = state.storage.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        entries::delete(tx, &storage, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Keeps the receipt (multipart field `file`: a photo or a PDF) on a draft of yours.
#[utoipa::path(put, path = "/api/v1/expenses/{id}/receipt", operation_id = "put_expense_receipt", tag = "expenses",
    params(("id" = Uuid, Path)),
    request_body(content_type = "multipart/form-data", description = "A `file` part"),
    responses((status = 200, body = Expense)))]
pub async fn put_receipt(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    mut multipart: axum::extract::Multipart,
) -> ApiResult<Json<Expense>> {
    own(&state, &authed, id).await?;
    let mut file = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(e.body_text()))?
    {
        if field.name() == Some("file") {
            let name = field.file_name().unwrap_or("receipt").to_owned();
            let kind = field
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_owned();
            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError::bad_request(e.body_text()))?;
            file = Some((name, kind, data));
            break;
        }
    }
    let (name, kind, data) = file.ok_or_else(|| {
        ApiError::validation().with_field_error("/file", "Choose a photo or PDF of the receipt.")
    })?;
    let ctx = authed.user.ctx.clone();
    let storage = state.storage.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            entries::put_receipt(tx, &storage, &ctx, id, &name, &kind, data).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/expenses/{id}/receipt", operation_id = "get_expense_receipt", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 200, description = "The receipt's bytes")))]
pub async fn receipt(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    visible(&state, &authed, id).await?;
    let storage = state.storage.clone();
    let (name, kind, data) = scoped(&state, &OrgScope::System, async |tx| {
        entries::receipt(tx, &storage, id).await
    })
    .await?;
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .filter(|c| *c != '"')
        .collect();
    Ok((
        [
            (header::CONTENT_TYPE, kind),
            (
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{safe}\""),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        ],
        data,
    ))
}

/// Takes what AI read from the receipt into a draft of yours: the amount,
/// tax, currency, day and vendor it found.
#[utoipa::path(post, path = "/api/v1/expenses/{id}/apply-reading", operation_id = "apply_expense_reading", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 200, body = Expense)))]
pub async fn apply_reading(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Expense>> {
    own(&state, &authed, id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            entries::apply_reading(tx, &ctx, id).await
        })
        .await?,
    ))
}

/// A report the caller may see: theirs, or with `expense:approve` or
/// `expense:read` at its organization.
async fn report_visible(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<()> {
    let (user, org) = scoped(state, &OrgScope::System, async |tx| {
        reports::owner_of(tx, id).await
    })
    .await?
    .ok_or_else(ApiError::not_found)?;
    require_module_at(state, MODULE, org).await?;
    if user == me(authed) {
        return Ok(());
    }
    if authed
        .require_at(state, permissions::EXPENSE_APPROVE, org)
        .await
        .is_ok()
    {
        return Ok(());
    }
    authed
        .require_at(state, permissions::EXPENSE_READ, org)
        .await
        .map_err(|_| ApiError::not_found())
}

/// Expense reports, newest first: everyone's where you approve or read
/// expenses; otherwise yours.
#[utoipa::path(get, path = "/api/v1/expense-reports", operation_id = "list_expense_reports", tag = "expenses",
    params(ReportFilter), responses((status = 200, body = Vec<Report>)))]
pub async fn list_reports(
    State(state): State<AppState>,
    authed: Authed,
    Query(mut filter): Query<ReportFilter>,
) -> ApiResult<Json<Vec<Report>>> {
    require_module(&state, MODULE).await?;
    let mut scope = authed
        .module_scope(&state, permissions::EXPENSE_APPROVE, MODULE)
        .await?;
    if matches!(&scope, OrgScope::Orgs(o) if o.is_empty()) {
        scope = authed
            .module_scope(&state, permissions::EXPENSE_READ, MODULE)
            .await?;
    }
    if matches!(&scope, OrgScope::Orgs(o) if o.is_empty()) {
        filter.user_id = Some(me(&authed));
        scope = OrgScope::System;
    }
    Ok(Json(
        scoped(&state, &scope, async |tx| reports::list(tx, &filter).await).await?,
    ))
}

/// Submits expenses of yours (drafts, or ones sent back) as a report, for
/// approval. Raises `expense_report.submitted` (the Expense approval pack).
#[utoipa::path(post, path = "/api/v1/expense-reports", operation_id = "submit_expense_report", tag = "expenses",
    request_body = SubmitInput, responses((status = 201, body = Report)))]
pub async fn submit_report(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<SubmitInput>,
) -> ApiResult<(StatusCode, Json<Report>)> {
    require_module(&state, MODULE).await?;
    let ctx = authed.user.ctx.clone();
    let user = me(&authed);
    let r = scoped(&state, &OrgScope::System, async |tx| {
        reports::submit(tx, &ctx, user, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(r)))
}

/// A report, with the open approval for the caller to decide.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportView {
    #[serde(flatten)]
    pub report: Report,
    /// The approval workflow's open approval, if any.
    pub approval_id: Option<Uuid>,
    /// The caller is one of its approvers.
    pub can_decide: bool,
}

#[utoipa::path(get, path = "/api/v1/expense-reports/{id}", operation_id = "get_expense_report", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 200, body = ReportView)))]
pub async fn get_report(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ReportView>> {
    report_visible(&state, &authed, id).await?;
    let user = me(&authed);
    let (report, open) = scoped(&state, &OrgScope::System, async |tx| {
        let r = reports::get(tx, id).await?;
        let open = reports::open_approval(tx, id, user).await?;
        Ok((r, open))
    })
    .await?;
    Ok(Json(ReportView {
        report,
        approval_id: open.map(|(a, _)| a),
        can_decide: open.is_some_and(|(_, me)| me),
    }))
}

#[utoipa::path(get, path = "/api/v1/expense-reports/{id}/comments", operation_id = "expense_report_comments", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<Comment>)))]
pub async fn report_comments(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Comment>>> {
    report_visible(&state, &authed, id).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            reports::comments(tx, id).await
        })
        .await?,
    ))
}

/// Comments on a report or one of its expenses (whoever submitted it, and
/// anyone @mentioned, are told).
#[utoipa::path(post, path = "/api/v1/expense-reports/{id}/comments", operation_id = "add_expense_report_comment",
    tag = "expenses", params(("id" = Uuid, Path)), request_body = CommentInput, responses((status = 201, body = Comment)))]
pub async fn add_report_comment(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CommentInput>,
) -> ApiResult<(StatusCode, Json<Comment>)> {
    report_visible(&state, &authed, id).await?;
    let ctx = authed.user.ctx.clone();
    let c = scoped(&state, &OrgScope::System, async |tx| {
        reports::comment(tx, &ctx, id, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(c)))
}

fn billing_map(e: huxley_billing::BillingError) -> ApiError {
    use huxley_billing::BillingError as B;
    match e {
        B::NotFound => ApiError::not_found(),
        B::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        B::Conflict(m) => ApiError::conflict(m),
        B::Db(e) => ApiError::internal(&e),
        B::Sqlx(e) => ApiError::internal(&e),
        B::Contracts(e) => ApiError::internal(&e),
    }
}

/// Checks `expense:approve` at a report's organization.
async fn report_approver(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<()> {
    let (_, org) = scoped(state, &OrgScope::System, async |tx| {
        reports::owner_of(tx, id).await
    })
    .await?
    .ok_or_else(ApiError::not_found)?;
    at(state, authed, org, permissions::EXPENSE_APPROVE).await
}

async fn billing_tx<T>(
    state: &AppState,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, huxley_billing::BillingError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let v = f(&mut tx).await.map_err(billing_map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(v)
}

/// A report's accounting status: whether it's in the accounting system, and paid back.
#[utoipa::path(get, path = "/api/v1/expense-reports/{id}/sync", operation_id = "expense_report_sync", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 200, body = huxley_billing::expense_sync::ReportSync)))]
pub async fn report_sync(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<huxley_billing::expense_sync::ReportSync>> {
    report_visible(&state, &authed, id).await?;
    Ok(Json(
        billing_tx(&state, async |tx| {
            huxley_billing::expense_sync::state(tx, id).await
        })
        .await?,
    ))
}

/// Sends an approved report to the accounting system as a bill to the
/// person (raises `expense_report.sync_requested` for the sync pack), or
/// fails it saying what to map.
#[utoipa::path(post, path = "/api/v1/expense-reports/{id}/sync", operation_id = "sync_expense_report", tag = "expenses",
    params(("id" = Uuid, Path)), responses((status = 200, body = huxley_billing::expense_sync::ReportSync)))]
pub async fn sync_report(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<huxley_billing::expense_sync::ReportSync>> {
    report_approver(&state, &authed, id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        billing_tx(&state, async |tx| {
            huxley_billing::expense_sync::request(tx, &ctx, id).await
        })
        .await?,
    ))
}

/// Marks an approved report paid back.
#[utoipa::path(post, path = "/api/v1/expense-reports/{id}/reimbursed", operation_id = "reimburse_expense_report",
    tag = "expenses", params(("id" = Uuid, Path)), responses((status = 200, body = huxley_billing::expense_sync::ReportSync)))]
pub async fn reimburse_report(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<huxley_billing::expense_sync::ReportSync>> {
    report_approver(&state, &authed, id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        billing_tx(&state, async |tx| {
            huxley_billing::expense_sync::mark_reimbursed(tx, &ctx, id).await
        })
        .await?,
    ))
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ExportParams {
    /// `approved` (the default) or `reimbursed`.
    pub status: Option<String>,
}

/// Reimbursable expenses in approved (or paid back) reports, as CSV.
#[utoipa::path(get, path = "/api/v1/expense-reports/export.csv", operation_id = "export_expense_reports", tag = "expenses",
    params(ExportParams), responses((status = 200, content_type = "text/csv")))]
pub async fn export_reports(
    State(state): State<AppState>,
    authed: Authed,
    Query(p): Query<ExportParams>,
) -> ApiResult<impl IntoResponse> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::EXPENSE_APPROVE, MODULE)
        .await?;
    if matches!(&scope, OrgScope::Orgs(o) if o.is_empty()) {
        return Err(ApiError::forbidden());
    }
    let status = match p.status.as_deref() {
        None | Some("approved") => "approved",
        Some("reimbursed") => "reimbursed",
        Some(_) => {
            return Err(
                ApiError::validation().with_field_error("/status", "approved or reimbursed")
            );
        }
    };
    let mut tx = begin_scoped(&state.db, &scope)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let csv = huxley_billing::expense_sync::export_csv(&mut tx, Some(status))
        .await
        .map_err(billing_map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"expenses-{status}.csv\""),
            ),
        ],
        csv,
    ))
}
