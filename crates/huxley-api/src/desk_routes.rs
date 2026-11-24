//! The `servicedesk` module's API (design doc §8.2, ADR-036): tickets with
//! their notes and activity, service queues, and the configuration they use.
//! Ticket routes check that the module is active at the ticket's
//! organization (404 otherwise), then `ticket:read` or `ticket:write` there.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::config::{self, MatrixCell, TicketType};
use huxley_servicedesk::queues::{self, Queue, QueueInput};
use huxley_servicedesk::sla::{self, Policy, PolicyInput, SlaAssignment};
use huxley_servicedesk::tickets::{
    self, Activity, NewNote, NewTicket, Note, StatusChange, Ticket, TicketChanges, TicketFilter,
    TicketPage,
};
use huxley_servicedesk::{DeskError, MODULE};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/servicedesk/types", get(list_types))
        .route("/servicedesk/priority-matrix", get(priority_matrix))
        .route(
            "/servicedesk/sla-policies",
            get(list_policies).post(create_policy),
        )
        .route(
            "/servicedesk/sla-policies/{id}",
            axum::routing::put(update_policy).delete(archive_policy),
        )
        .route("/queues", get(list_queues).post(create_queue))
        .route(
            "/queues/{id}",
            get(get_queue).put(update_queue).delete(archive_queue),
        )
        .route("/tickets", get(list_tickets).post(create_ticket))
        .route("/tickets/{id}", get(get_ticket).patch(update_ticket))
        .route("/tickets/{id}/status", post(change_status))
        .route("/tickets/{id}/notes", get(list_notes).post(add_note))
        .route("/tickets/{id}/activity", get(list_activity))
}

/// Routes under `/orgs`.
pub fn org_routes() -> Router<AppState> {
    Router::new().route("/{id}/sla", get(get_assignment).put(set_assignment))
}

fn map(e: DeskError) -> ApiError {
    match e {
        DeskError::NotFound => ApiError::not_found(),
        DeskError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        DeskError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        DeskError::Db(e) => ApiError::internal(&e),
        DeskError::Sqlx(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

async fn allow(state: &AppState, authed: &Authed, permission: &str, org_id: Uuid) -> ApiResult<()> {
    require_module_at(state, MODULE, org_id).await?;
    authed.require_at(state, permission, org_id).await
}

/// The organization a record belongs to (404 if there's none).
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

async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, DeskError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope).await.map_err(db_err)?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(value)
}

/// Ticket types with their statuses and allowed changes.
#[utoipa::path(get, path = "/api/v1/servicedesk/types", tag = "servicedesk",
    responses((status = 200, body = Vec<TicketType>)))]
pub async fn list_types(
    State(state): State<AppState>,
    _authed: Authed,
) -> ApiResult<Json<Vec<TicketType>>> {
    require_module(&state, MODULE).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            config::types(tx).await
        })
        .await?,
    ))
}

/// How impact and urgency (1 high … 3 low) make a priority.
#[utoipa::path(get, path = "/api/v1/servicedesk/priority-matrix", tag = "servicedesk",
    responses((status = 200, body = Vec<MatrixCell>)))]
pub async fn priority_matrix(
    State(state): State<AppState>,
    _authed: Authed,
) -> ApiResult<Json<Vec<MatrixCell>>> {
    require_module(&state, MODULE).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            config::matrix(tx).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct QueueListParams {
    pub org_id: Option<Uuid>,
    #[serde(default)]
    pub include_archived: bool,
}

/// Service queues you can see.
#[utoipa::path(get, path = "/api/v1/queues", tag = "servicedesk", params(QueueListParams),
    responses((status = 200, body = Vec<Queue>)))]
pub async fn list_queues(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<QueueListParams>,
) -> ApiResult<Json<Vec<Queue>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::TICKET_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            queues::list(tx, params.org_id, params.include_archived).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateQueue {
    /// The organization it belongs to (usually your MSP).
    pub org_id: Uuid,
    #[serde(flatten)]
    pub queue: QueueInput,
}

/// Creates a service queue. Needs `servicedesk:configure`.
#[utoipa::path(post, path = "/api/v1/queues", tag = "servicedesk", request_body = CreateQueue,
    responses((status = 201, body = Queue), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_queue(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateQueue>,
) -> ApiResult<(StatusCode, Json<Queue>)> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    require_module_at(&state, MODULE, body.org_id).await?;
    let ctx = authed.user.ctx.clone();
    let queue = scoped(&state, &OrgScope::System, async |tx| {
        queues::create(tx, &ctx, body.org_id, &body.queue).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(queue)))
}

#[utoipa::path(get, path = "/api/v1/queues/{id}", tag = "servicedesk", params(("id" = Uuid, Path)),
    responses((status = 200, body = Queue), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_queue(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Queue>> {
    let org = owner(&state, "sd_queues", id).await?;
    allow(&state, &authed, permissions::TICKET_READ, org).await?;
    let scope = authed.scope(&state).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| queues::get(tx, id).await).await?,
    ))
}

/// Changes a queue (and, if given, its members). Needs `servicedesk:configure`.
#[utoipa::path(put, path = "/api/v1/queues/{id}", tag = "servicedesk", params(("id" = Uuid, Path)),
    request_body = QueueInput,
    responses((status = 200, body = Queue), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_queue(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<QueueInput>,
) -> ApiResult<Json<Queue>> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let org = owner(&state, "sd_queues", id).await?;
    require_module_at(&state, MODULE, org).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            queues::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives a queue. Needs `servicedesk:configure`.
#[utoipa::path(delete, path = "/api/v1/queues/{id}", tag = "servicedesk", params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_queue(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let org = owner(&state, "sd_queues", id).await?;
    require_module_at(&state, MODULE, org).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        queues::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Tickets you can see, most urgent first. `assignee=me` means you.
#[utoipa::path(get, path = "/api/v1/tickets", tag = "servicedesk", params(TicketFilter),
    responses((status = 200, body = TicketPage)))]
pub async fn list_tickets(
    State(state): State<AppState>,
    authed: Authed,
    Query(mut filter): Query<TicketFilter>,
) -> ApiResult<Json<TicketPage>> {
    require_module(&state, MODULE).await?;
    if filter.assignee.as_deref() == Some("me") {
        filter.assignee = Some(authed.user.auth.user.id.to_string());
    }
    let scope = authed
        .module_scope(&state, permissions::TICKET_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| tickets::list(tx, &filter).await).await?,
    ))
}

/// The organization's ticket prefix (`numbering.ticket_prefix`).
async fn prefix(state: &AppState, org_id: Uuid) -> ApiResult<String> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let settings = huxley_orgs::settings::resolve(&mut tx, org_id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(settings
        .into_iter()
        .find(|s| s.def.key == huxley_orgs::settings::TICKET_PREFIX)
        .and_then(|s| s.value)
        .unwrap_or_else(|| "T".into()))
}

/// Creates a ticket for an organization.
#[utoipa::path(post, path = "/api/v1/tickets", tag = "servicedesk", request_body = NewTicket,
    responses((status = 201, body = Ticket), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_ticket(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<NewTicket>,
) -> ApiResult<(StatusCode, Json<Ticket>)> {
    allow(&state, &authed, permissions::TICKET_WRITE, body.org_id).await?;
    let prefix = prefix(&state, body.org_id).await?;
    // The SLA terms come from the organizations above too: system scope.
    let plan = scoped(&state, &OrgScope::System, async |tx| {
        sla::plan_for(tx, body.org_id).await
    })
    .await?;
    let ctx = authed.user.ctx.clone();
    let scope = authed.scope(&state).await?;
    let ticket = scoped(&state, &scope, async |tx| {
        tickets::create(tx, &ctx, &body, &prefix, plan.as_ref()).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(ticket)))
}

/// The ticket's organization, after checking the module and `permission`.
async fn ticket_access(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let org = owner(state, "sd_tickets", id).await?;
    allow(state, authed, permission, org).await?;
    authed.scope(state).await
}

#[utoipa::path(get, path = "/api/v1/tickets/{id}", tag = "servicedesk", params(("id" = Uuid, Path)),
    responses((status = 200, body = Ticket), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_ticket(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Ticket>> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| tickets::get(tx, id).await).await?,
    ))
}

/// Changes a ticket's fields; send back the `version` you read. `null`
/// clears queue, requester or assignee; leaving a field out keeps it.
#[utoipa::path(patch, path = "/api/v1/tickets/{id}", tag = "servicedesk", params(("id" = Uuid, Path)),
    request_body = TicketChanges,
    responses((status = 200, body = Ticket), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_ticket(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<TicketChanges>,
) -> ApiResult<Json<Ticket>> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            tickets::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Moves a ticket to another status (allowed changes only, with what the
/// status needs: a resolution for Resolved).
#[utoipa::path(post, path = "/api/v1/tickets/{id}/status", tag = "servicedesk", params(("id" = Uuid, Path)),
    request_body = StatusChange,
    responses((status = 200, body = Ticket), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn change_status(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<StatusChange>,
) -> ApiResult<Json<Ticket>> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            tickets::change_status(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/tickets/{id}/notes", tag = "servicedesk", params(("id" = Uuid, Path)),
    responses((status = 200, body = Vec<Note>)))]
pub async fn list_notes(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Note>>> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| tickets::notes(tx, id).await).await?,
    ))
}

/// Adds a public, internal or resolution note.
#[utoipa::path(post, path = "/api/v1/tickets/{id}/notes", tag = "servicedesk", params(("id" = Uuid, Path)),
    request_body = NewNote,
    responses((status = 201, body = Note), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_note(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<NewNote>,
) -> ApiResult<(StatusCode, Json<Note>)> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let note = scoped(&state, &scope, async |tx| {
        tickets::add_note(tx, &ctx, id, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(note)))
}

/// A ticket's timeline: what changed, when and by whom.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/activity", tag = "servicedesk", params(("id" = Uuid, Path)),
    responses((status = 200, body = Vec<Activity>)))]
pub async fn list_activity(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Activity>>> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| tickets::activity(tx, id).await).await?,
    ))
}

/// SLA policies, the default first.
#[utoipa::path(get, path = "/api/v1/servicedesk/sla-policies", tag = "servicedesk",
    responses((status = 200, body = Vec<Policy>)))]
pub async fn list_policies(
    State(state): State<AppState>,
    _authed: Authed,
) -> ApiResult<Json<Vec<Policy>>> {
    require_module(&state, MODULE).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            sla::policies(tx).await
        })
        .await?,
    ))
}

/// Creates an SLA policy. Needs `servicedesk:configure`.
#[utoipa::path(post, path = "/api/v1/servicedesk/sla-policies", tag = "servicedesk",
    request_body = PolicyInput,
    responses((status = 201, body = Policy), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_policy(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<PolicyInput>,
) -> ApiResult<(StatusCode, Json<Policy>)> {
    require_module(&state, MODULE).await?;
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let ctx = authed.user.ctx.clone();
    let policy = scoped(&state, &OrgScope::System, async |tx| {
        sla::create(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(policy)))
}

/// Changes an SLA policy; tickets already open keep their terms. Needs
/// `servicedesk:configure`.
#[utoipa::path(put, path = "/api/v1/servicedesk/sla-policies/{id}", tag = "servicedesk",
    params(("id" = Uuid, Path)), request_body = PolicyInput,
    responses((status = 200, body = Policy), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_policy(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<PolicyInput>,
) -> ApiResult<Json<Policy>> {
    require_module(&state, MODULE).await?;
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            sla::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives an SLA policy (not the default). Needs `servicedesk:configure`.
#[utoipa::path(delete, path = "/api/v1/servicedesk/sla-policies/{id}", tag = "servicedesk",
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_policy(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_module(&state, MODULE).await?;
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        sla::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The SLA policy for an organization: set here, inherited, or the default.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/sla", tag = "servicedesk", params(("id" = Uuid, Path)),
    responses((status = 200, body = SlaAssignment), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_assignment(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SlaAssignment>> {
    allow(&state, &authed, permissions::TICKET_READ, id).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            sla::assignment(tx, id).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentInput {
    /// The policy for this organization and those below it; null follows
    /// the one above.
    pub policy_id: Option<Uuid>,
}

/// Sets the SLA policy for an organization and those below it. Needs
/// `servicedesk:configure`.
#[utoipa::path(put, path = "/api/v1/orgs/{id}/sla", tag = "servicedesk", params(("id" = Uuid, Path)),
    request_body = AssignmentInput,
    responses((status = 200, body = SlaAssignment), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_assignment(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignmentInput>,
) -> ApiResult<Json<SlaAssignment>> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    require_module_at(&state, MODULE, id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            sla::assign(tx, &ctx, id, body.policy_id).await
        })
        .await?,
    ))
}
