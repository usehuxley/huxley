//! Presales scoping (ADR-072): an opportunity's scoping workspace.
//! `sales:read` at the opportunity's client to see it, `sales:write` to
//! work on it. Drafting raises `scope.draft_requested` for the Presales
//! scoping pack.

use axum::extract::{Path, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::OrgScope;
use huxley_sales::estimate::{self, Actuals, Assembled, Estimate, ItemInput, Risk, TaskInput};
use huxley_sales::opportunities;
use huxley_sales::scoping::{self, Scope, ScopeOfWork, SourceInput, Template};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module;
use crate::sales_routes::{access_at, org_of, scoped};
use crate::{ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/scope-templates", get(list_templates))
        .route(
            "/opportunities/{id}/scope",
            get(opportunity_scope).post(open_scope),
        )
        .route("/scopes/{id}", get(get_scope))
        .route("/scopes/{id}/sources", post(add_source))
        .route("/scopes/{id}/sources/{source}", delete(delete_source))
        .route("/scopes/{id}/sections/{key}", put(edit_section))
        .route("/scopes/{id}/questions", post(add_question))
        .route("/scopes/{id}/questions/{question}", post(answer_question))
        .route(
            "/scopes/{id}/draft",
            post(request_draft).delete(cancel_draft),
        )
        .route("/scopes/{id}/approve", post(approve_scope))
        .route("/scope-versions/{id}", get(get_version))
        .route(
            "/scopes/{id}/estimate",
            get(get_estimate)
                .post(request_estimate)
                .delete(cancel_estimate),
        )
        .route("/scopes/{id}/estimate/tasks", put(set_tasks))
        .route("/scopes/{id}/estimate/items", put(set_items))
        .route("/scopes/{id}/estimate/risk", put(set_risk))
        .route("/scopes/{id}/quote", post(assemble_quote))
        .route("/scopes/{id}/actuals", get(scope_actuals))
}

async fn at_scope(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<huxley_db::OrgScope> {
    let org = org_of(state, async |tx| scoping::org_of(tx, id).await).await?;
    access_at(state, authed, org, permission).await
}

/// The kinds of engagement a scope can follow, and their sections.
#[utoipa::path(get, path = "/api/v1/scope-templates", operation_id = "list_scope_templates", tag = "sales",
    responses((status = 200, body = Vec<Template>)))]
pub async fn list_templates(State(state): State<AppState>) -> ApiResult<Json<Vec<Template>>> {
    require_module(&state, huxley_sales::MODULE).await?;
    Ok(Json(scoping::TEMPLATES.to_vec()))
}

/// The opportunity's scope (404 until it's started).
#[utoipa::path(get, path = "/api/v1/opportunities/{id}/scope", operation_id = "opportunity_scope", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Scope)))]
pub async fn opportunity_scope(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Scope>> {
    let org = org_of(&state, async |tx| opportunities::org_of(tx, id).await).await?;
    let scope = access_at(&state, &authed, org, permissions::SALES_READ).await?;
    let found = scoped(&state, &scope, async |tx| {
        scoping::of_opportunity(tx, id).await
    })
    .await?;
    let found = found.ok_or_else(crate::ApiError::not_found)?;
    Ok(Json(
        scoped(&state, &scope, async |tx| scoping::get(tx, found).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenScope {
    /// A scope template's key.
    pub template: String,
}

/// Starts scoping an opportunity (or returns the scope it has).
#[utoipa::path(post, path = "/api/v1/opportunities/{id}/scope", operation_id = "open_scope", tag = "sales",
    params(("id" = Uuid, Path)), request_body = OpenScope, responses((status = 200, body = Scope)))]
pub async fn open_scope(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<OpenScope>,
) -> ApiResult<Json<Scope>> {
    let org = org_of(&state, async |tx| opportunities::org_of(tx, id).await).await?;
    let scope = access_at(&state, &authed, org, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::open(tx, &ctx, id, &body.template).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/scopes/{id}", operation_id = "get_scope", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Scope)))]
pub async fn get_scope(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| scoping::get(tx, id).await).await?,
    ))
}

/// Adds discovery: a note, a call transcript, or a document's text.
#[utoipa::path(post, path = "/api/v1/scopes/{id}/sources", operation_id = "add_scope_source", tag = "sales",
    params(("id" = Uuid, Path)), request_body = SourceInput, responses((status = 200, body = Scope)))]
pub async fn add_source(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SourceInput>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::add_source(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/scopes/{id}/sources/{source}", operation_id = "delete_scope_source", tag = "sales",
    params(("id" = Uuid, Path), ("source" = Uuid, Path)), responses((status = 200, body = Scope)))]
pub async fn delete_source(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, source)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::delete_source(tx, id, source).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SectionBody {
    /// Markdown.
    pub body: String,
}

/// Writes a section (it's then yours: drafting leaves it alone).
#[utoipa::path(put, path = "/api/v1/scopes/{id}/sections/{key}", operation_id = "edit_scope_section", tag = "sales",
    params(("id" = Uuid, Path), ("key" = String, Path)), request_body = SectionBody, responses((status = 200, body = Scope)))]
pub async fn edit_section(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, key)): Path<(Uuid, String)>,
    Json(body): Json<SectionBody>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::edit_section(tx, &ctx, id, &key, &body.body).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewQuestion {
    #[serde(default)]
    pub section_key: Option<String>,
    pub question: String,
}

#[utoipa::path(post, path = "/api/v1/scopes/{id}/questions", operation_id = "add_scope_question", tag = "sales",
    params(("id" = Uuid, Path)), request_body = NewQuestion, responses((status = 200, body = Scope)))]
pub async fn add_question(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<NewQuestion>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::add_question(tx, id, body.section_key.as_deref(), &body.question).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionAnswer {
    #[serde(default)]
    pub answer: Option<String>,
    pub resolved: bool,
}

/// Answers an open question, and resolves it (or opens it again).
#[utoipa::path(post, path = "/api/v1/scopes/{id}/questions/{question}", operation_id = "answer_scope_question", tag = "sales",
    params(("id" = Uuid, Path), ("question" = Uuid, Path)), request_body = QuestionAnswer, responses((status = 200, body = Scope)))]
pub async fn answer_question(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, question)): Path<(Uuid, Uuid)>,
    Json(body): Json<QuestionAnswer>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::answer_question(tx, id, question, body.answer.as_deref(), body.resolved).await
        })
        .await?,
    ))
}

/// Asks for an AI draft (the Presales scoping pack writes it).
#[utoipa::path(post, path = "/api/v1/scopes/{id}/draft", operation_id = "request_scope_draft", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Scope)))]
pub async fn request_draft(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::request_draft(tx, &ctx, id).await
        })
        .await?,
    ))
}

/// Stops waiting for a draft.
#[utoipa::path(delete, path = "/api/v1/scopes/{id}/draft", operation_id = "cancel_scope_draft", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Scope)))]
pub async fn cancel_draft(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::cancel_draft(tx, id).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApproveScope {
    /// The version you read.
    pub version: i32,
}

/// Approves the Scope of Work as it stands, keeping it as the next version.
#[utoipa::path(post, path = "/api/v1/scopes/{id}/approve", operation_id = "approve_scope", tag = "sales",
    params(("id" = Uuid, Path)), request_body = ApproveScope, responses((status = 200, body = Scope)))]
pub async fn approve_scope(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ApproveScope>,
) -> ApiResult<Json<Scope>> {
    let scope = at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::approve(tx, &ctx, id, body.version).await
        })
        .await?,
    ))
}

/// An approved version of a Scope of Work.
#[utoipa::path(get, path = "/api/v1/scope-versions/{id}", operation_id = "get_scope_version", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = ScopeOfWork)))]
pub async fn get_version(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ScopeOfWork>> {
    let org = org_of(&state, async |tx| {
        Ok(
            sqlx::query_scalar("SELECT org_id FROM sales_scope_versions WHERE id = $1")
                .bind(id)
                .fetch_optional(tx)
                .await?,
        )
    })
    .await?;
    let scope = access_at(&state, &authed, org, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            scoping::version_of(tx, id, None).await
        })
        .await?,
    ))
}

// ---- the estimate ----

/// The scope's WBS and BOM, with totals, contingency and checks.
#[utoipa::path(get, path = "/api/v1/scopes/{id}/estimate", operation_id = "get_scope_estimate", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Estimate)))]
pub async fn get_estimate(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Estimate>> {
    at_scope(&state, &authed, id, permissions::SALES_READ).await?;
    // Rates and products are the providers' catalog's.
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            estimate::get(tx, id).await
        })
        .await?,
    ))
}

/// Replaces the Work Breakdown Structure.
#[utoipa::path(put, path = "/api/v1/scopes/{id}/estimate/tasks", operation_id = "set_scope_tasks", tag = "sales",
    params(("id" = Uuid, Path)), request_body = Vec<TaskInput>, responses((status = 200, body = Estimate)))]
pub async fn set_tasks(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Vec<TaskInput>>,
) -> ApiResult<Json<Estimate>> {
    at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            estimate::set_tasks(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Replaces the Bill of Materials.
#[utoipa::path(put, path = "/api/v1/scopes/{id}/estimate/items", operation_id = "set_scope_items", tag = "sales",
    params(("id" = Uuid, Path)), request_body = Vec<ItemInput>, responses((status = 200, body = Estimate)))]
pub async fn set_items(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Vec<ItemInput>>,
) -> ApiResult<Json<Estimate>> {
    at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            estimate::set_items(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RiskBody {
    pub risk: Risk,
}

/// Sets how risky the work is: the contingency follows (10, 20 or 35%).
#[utoipa::path(put, path = "/api/v1/scopes/{id}/estimate/risk", operation_id = "set_scope_risk", tag = "sales",
    params(("id" = Uuid, Path)), request_body = RiskBody, responses((status = 200, body = Estimate)))]
pub async fn set_risk(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<RiskBody>,
) -> ApiResult<Json<Estimate>> {
    at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            estimate::set_risk(tx, id, body.risk).await
        })
        .await?,
    ))
}

/// Asks for an AI estimate (the Presales scoping pack drafts it).
#[utoipa::path(post, path = "/api/v1/scopes/{id}/estimate", operation_id = "request_scope_estimate", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Estimate)))]
pub async fn request_estimate(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Estimate>> {
    at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            estimate::request(tx, &ctx, id).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/scopes/{id}/estimate", operation_id = "cancel_scope_estimate", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Estimate)))]
pub async fn cancel_estimate(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Estimate>> {
    at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            estimate::cancel(tx, id).await
        })
        .await?,
    ))
}

/// Builds a draft quote on the opportunity from the estimate, carrying the
/// latest approved Scope of Work; says what to check.
#[utoipa::path(post, path = "/api/v1/scopes/{id}/quote", operation_id = "assemble_scope_quote", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Assembled)))]
pub async fn assemble_quote(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Assembled>> {
    at_scope(&state, &authed, id, permissions::SALES_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            estimate::assemble(tx, &ctx, id).await
        })
        .await?,
    ))
}

/// The estimate against the hours logged on the project the deal became
/// (null until its sales order starts one).
#[utoipa::path(get, path = "/api/v1/scopes/{id}/actuals", operation_id = "scope_actuals", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Option<Actuals>)))]
pub async fn scope_actuals(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Option<Actuals>>> {
    at_scope(&state, &authed, id, permissions::SALES_READ).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            estimate::actuals(tx, id).await
        })
        .await?,
    ))
}
