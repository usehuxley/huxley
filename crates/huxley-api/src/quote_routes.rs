//! Quotes (design doc §8.10, ADR-064): drafting, sending (each send kept
//! as a revision), the client's answer, and a PDF.
//!
//! - `sales:read` at the client: its quotes, their revisions and PDFs.
//! - `sales:write`: create, change, send, record the answer and delete.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::OrgScope;
use huxley_sales::MODULE;
use huxley_sales::quotes::{self, DecisionInput, Quote, QuoteInput, Revision};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module;
use crate::sales_routes::{access_at, org_of, scoped};
use crate::{ApiError, ApiResult, AppState};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_quotes).post(create_quote))
        .route(
            "/{id}",
            get(get_quote).put(update_quote).delete(delete_quote),
        )
        .route("/{id}/send", post(send_quote))
        .route("/{id}/decision", post(decide_quote))
        .route("/{id}/revisions", get(quote_revisions))
        .route("/{id}/pdf", get(quote_pdf))
}

async fn quote_access(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let org = org_of(state, async |tx| quotes::org_of(tx, id).await).await?;
    access_at(state, authed, org, permission).await
}

/// Quotes you can see, newest first.
#[utoipa::path(get, path = "/api/v1/quotes", operation_id = "list_quotes", tag = "sales",
    params(quotes::Filter), responses((status = 200, body = Vec<Quote>)))]
pub async fn list_quotes(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<quotes::Filter>,
) -> ApiResult<Json<Vec<Quote>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::SALES_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| quotes::list(tx, &filter).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateQuoteRequest {
    /// The client it's for.
    pub org_id: Uuid,
    pub quote: QuoteInput,
}

/// Drafts a quote.
#[utoipa::path(post, path = "/api/v1/quotes", operation_id = "create_quote", tag = "sales",
    request_body = CreateQuoteRequest,
    responses((status = 201, body = Quote), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_quote(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateQuoteRequest>,
) -> ApiResult<(StatusCode, Json<Quote>)> {
    let scope = access_at(&state, &authed, body.org_id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let q = scoped(&state, &scope, async |tx| {
        quotes::create(tx, &ctx, body.org_id, &body.quote).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(q)))
}

#[utoipa::path(get, path = "/api/v1/quotes/{id}", operation_id = "get_quote", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Quote)))]
pub async fn get_quote(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Quote>> {
    let scope = quote_access(&state, &authed, id, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| quotes::get(tx, id).await).await?,
    ))
}

/// Saves the quote and all its lines (send the version you read). A sent
/// quote goes back to draft as its next revision.
#[utoipa::path(put, path = "/api/v1/quotes/{id}", operation_id = "update_quote", tag = "sales",
    params(("id" = Uuid, Path)), request_body = QuoteInput,
    responses((status = 200, body = Quote), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_quote(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<QuoteInput>,
) -> ApiResult<Json<Quote>> {
    let scope = quote_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            quotes::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Deletes a quote that was never sent.
#[utoipa::path(delete, path = "/api/v1/quotes/{id}", operation_id = "delete_quote", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 204), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_quote(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let scope = quote_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        quotes::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendQuoteRequest {
    pub version: i32,
}

/// Marks a draft sent, keeping what was sent as its revision.
#[utoipa::path(post, path = "/api/v1/quotes/{id}/send", operation_id = "send_quote", tag = "sales",
    params(("id" = Uuid, Path)), request_body = SendQuoteRequest,
    responses((status = 200, body = Quote), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn send_quote(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SendQuoteRequest>,
) -> ApiResult<Json<Quote>> {
    let scope = quote_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            quotes::send(tx, &ctx, id, body.version).await
        })
        .await?,
    ))
}

/// Records the client's acceptance (with the options taken) or decline.
/// Accepting wins its open opportunity.
#[utoipa::path(post, path = "/api/v1/quotes/{id}/decision", operation_id = "decide_quote", tag = "sales",
    params(("id" = Uuid, Path)), request_body = DecisionInput,
    responses((status = 200, body = Quote), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn decide_quote(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<DecisionInput>,
) -> ApiResult<Json<Quote>> {
    let scope = quote_access(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            quotes::decide(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// What was sent each time, newest first.
#[utoipa::path(get, path = "/api/v1/quotes/{id}/revisions", operation_id = "quote_revisions", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<Revision>)))]
pub async fn quote_revisions(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Revision>>> {
    let scope = quote_access(&state, &authed, id, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| quotes::revisions(tx, id).await).await?,
    ))
}

/// The quote as a PDF: its lines, options and totals.
#[utoipa::path(get, path = "/api/v1/quotes/{id}/pdf", operation_id = "quote_pdf", tag = "sales",
    params(("id" = Uuid, Path)),
    responses((status = 200, description = "The PDF", content_type = "application/pdf", body = Vec<u8>)))]
pub async fn quote_pdf(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    let scope = quote_access(&state, &authed, id, permissions::SALES_READ).await?;
    let q = scoped(&state, &scope, async |tx| quotes::get(tx, id).await).await?;
    let bytes = render_pdf(&q);
    Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/pdf")
        .header(
            axum::http::header::CONTENT_DISPOSITION,
            format!(
                "attachment; filename=\"Q-{}-r{}.pdf\"",
                q.number, q.revision
            ),
        )
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ApiError::internal(&e))
}

fn render_pdf(q: &Quote) -> Vec<u8> {
    use huxley_analytics::export::{Cell, Table, pdf};
    use huxley_sales::quotes::{Billing, LineKind};
    let money = |d: rust_decimal::Decimal| format!("{} {}", q.currency, d.round_dp(2));
    let billing = |b: Billing| match b {
        Billing::OneTime => "one-time",
        Billing::Monthly => "monthly",
        Billing::Quarterly => "quarterly",
        Billing::Annual => "annual",
    };
    let row = |l: &huxley_sales::quotes::Line| {
        vec![
            Cell::Text(match &l.sku {
                Some(sku) => format!("{} ({sku})", l.name),
                None => l.name.clone(),
            }),
            Cell::Number(format!("{} {}", l.quantity.normalize(), l.unit)),
            Cell::Number(money(l.unit_price)),
            Cell::Number(if l.discount_pct.is_zero() {
                String::new()
            } else {
                format!("{}%", l.discount_pct.normalize())
            }),
            Cell::Text(billing(l.billing).into()),
            Cell::Number(money(l.total)),
        ]
    };
    let columns: Vec<String> = [
        "Item",
        "Quantity",
        "Unit price",
        "Discount",
        "Billing",
        "Total",
    ]
    .map(String::from)
    .to_vec();
    let mut tables = Vec::new();
    let mut current = Table {
        title: "Quote".into(),
        columns: columns.clone(),
        rows: Vec::new(),
        note: None,
    };
    let mut options = Vec::new();
    for l in &q.lines {
        if l.kind == LineKind::Heading {
            if !current.rows.is_empty() {
                tables.push(current);
            }
            current = Table {
                title: l.name.clone(),
                columns: columns.clone(),
                rows: Vec::new(),
                note: None,
            };
        } else if l.optional {
            options.push(row(l));
        } else {
            current.rows.push(row(l));
        }
    }
    if !current.rows.is_empty() {
        tables.push(current);
    }
    if !options.is_empty() {
        tables.push(Table {
            title: "Options".into(),
            columns,
            rows: options,
            note: Some("Not in the totals unless taken.".into()),
        });
    }
    let t = &q.totals;
    tables.push(Table {
        title: "Totals".into(),
        columns: vec![String::new(), String::new()],
        rows: vec![
            vec![
                Cell::Text("One-time".into()),
                Cell::Number(money(t.one_time)),
            ],
            vec![Cell::Text("Monthly".into()), Cell::Number(money(t.monthly))],
            vec![
                Cell::Text("First year".into()),
                Cell::Number(money(t.first_year)),
            ],
        ],
        note: (!q.terms.is_empty()).then(|| q.terms.clone()),
    });
    let sub = format!(
        "{} · revision {}{}",
        q.org_name,
        q.revision,
        q.valid_until
            .map(|d| format!(" · valid until {d}"))
            .unwrap_or_default()
    );
    pdf(&format!("Quote Q-{}: {}", q.number, q.title), &sub, &tables)
}
