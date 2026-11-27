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
