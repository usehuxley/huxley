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
use huxley_servicedesk::links::{self, LinkedTicket, NewLink};
use huxley_servicedesk::mail::{self, Email, Mailbox, MailboxInput};
use huxley_servicedesk::queues::{self, Queue, QueueInput};
use huxley_servicedesk::responses::{self, CannedResponse, CannedResponseInput};
use huxley_servicedesk::sla::{self, Policy, PolicyInput, SlaAssignment};
use huxley_servicedesk::tickets::{
    self, Activity, NewNote, NewTicket, Note, StatusChange, Ticket, TicketChanges, TicketFilter,
    TicketPage,
};
use huxley_servicedesk::views::{self, TicketView, TicketViewInput};
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
        .route(
            "/canned-responses",
            get(list_responses).post(create_response),
        )
        .route(
            "/canned-responses/{id}",
            axum::routing::put(update_response).delete(archive_response),
        )
        .route("/tickets/{id}/responses", get(ticket_responses))
        .route("/tickets/{id}/links", get(list_links).post(add_link))
        .route(
            "/tickets/{id}/links/{other_id}",
            axum::routing::delete(remove_link),
        )
        .route("/tickets/{id}/merge", post(merge_ticket))
        .route("/ticket-views", get(list_views).post(create_view))
        .route(
            "/ticket-views/{id}",
            axum::routing::put(update_view).delete(delete_view),
        )
        .route("/my-work", get(my_work))
        .route("/tickets", get(list_tickets).post(create_ticket))
        .route("/tickets/bulk", post(bulk_update))
        .route("/tickets/{id}", get(get_ticket).patch(update_ticket))
        .route("/tickets/{id}/status", post(change_status))
        .route("/tickets/{id}/notes", get(list_notes).post(add_note))
        .route("/tickets/{id}/activity", get(list_activity))
        .route("/mailboxes", get(list_mailboxes).post(create_mailbox))
        .route(
            "/mailboxes/{id}",
            get(get_mailbox).put(update_mailbox).delete(archive_mailbox),
        )
        .route("/mailboxes/{id}/emails", get(list_emails))
        .route(
            "/mailboxes/{id}/messages",
            post(post_message).layer(axum::extract::DefaultBodyLimit::max(MAX_MESSAGE_BYTES)),
        )
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

/// Your work: your open tickets (most in need of attention first, with
/// why), your teams' tickets nobody has, and what's unassigned in your
/// queues.
#[utoipa::path(get, path = "/api/v1/my-work", tag = "servicedesk",
    responses((status = 200, body = huxley_servicedesk::my_work::MyWork)))]
pub async fn my_work(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<huxley_servicedesk::my_work::MyWork>> {
    require_module(&state, MODULE).await?;
    let me = authed.user.auth.user.id;
    let scope = authed
        .module_scope(&state, permissions::TICKET_READ, MODULE)
        .await?;
    let now = time::OffsetDateTime::now_utc();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            huxley_servicedesk::my_work::of(tx, me, now).await
        })
        .await?,
    ))
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

/// Creates a ticket for an organization.
#[utoipa::path(post, path = "/api/v1/tickets", tag = "servicedesk", request_body = NewTicket,
    responses((status = 201, body = Ticket), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_ticket(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<NewTicket>,
) -> ApiResult<(StatusCode, Json<Ticket>)> {
    allow(&state, &authed, permissions::TICKET_WRITE, body.org_id).await?;
    let prefix = scoped(&state, &OrgScope::System, async |tx| {
        tickets::prefix_for(tx, body.org_id).await
    })
    .await?;
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

/// The largest message a mailbox takes (with its attachments).
const MAX_MESSAGE_BYTES: usize = 40 * 1024 * 1024;

/// A mailbox password, sealed for mailbox `id` (none: no new password).
async fn seal(
    state: &AppState,
    org: Uuid,
    id: Uuid,
    password: Option<&str>,
) -> ApiResult<Option<Vec<u8>>> {
    match password.filter(|p| !p.is_empty()) {
        Some(p) if p.len() > 1024 => {
            Err(ApiError::validation().with_field_error("/password", "at most 1024 characters"))
        }
        Some(p) => Ok(Some(
            state
                .crypto
                .encrypt(org, mail::secret_binding(id), p.as_bytes())
                .await
                .map_err(|e| ApiError::internal(&e))?,
        )),
        None => Ok(None),
    }
}

/// Checks `servicedesk:configure` and the module at a mailbox's organization.
async fn configure_mailbox(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<()> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let org = owner(state, "sd_mailboxes", id).await?;
    require_module_at(state, MODULE, org).await
}

/// Service desk mailboxes. Needs `servicedesk:configure`.
#[utoipa::path(get, path = "/api/v1/mailboxes", tag = "servicedesk",
    responses((status = 200, body = Vec<Mailbox>)))]
pub async fn list_mailboxes(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Mailbox>>> {
    require_module(&state, MODULE).await?;
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            mail::mailboxes(tx).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateMailbox {
    /// The organization it belongs to (usually your MSP).
    pub org_id: Uuid,
    #[serde(flatten)]
    pub mailbox: MailboxInput,
}

/// Adds a mailbox. Needs `servicedesk:configure`.
#[utoipa::path(post, path = "/api/v1/mailboxes", tag = "servicedesk", request_body = CreateMailbox,
    responses((status = 201, body = Mailbox), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_mailbox(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateMailbox>,
) -> ApiResult<(StatusCode, Json<Mailbox>)> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    require_module_at(&state, MODULE, body.org_id).await?;
    let ctx = authed.user.ctx.clone();
    let id = huxley_core::new_id();
    // Sealed before the transaction (see `Crypto::encrypt`).
    let secret = seal(&state, body.org_id, id, body.mailbox.password.as_deref()).await?;
    let mailbox = scoped(&state, &OrgScope::System, async |tx| {
        mail::create(tx, &ctx, body.org_id, id, &body.mailbox, secret.as_deref()).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(mailbox)))
}

#[utoipa::path(get, path = "/api/v1/mailboxes/{id}", tag = "servicedesk", params(("id" = Uuid, Path)),
    responses((status = 200, body = Mailbox), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_mailbox(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Mailbox>> {
    configure_mailbox(&state, &authed, id).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            mail::mailbox(tx, id).await
        })
        .await?,
    ))
}

/// Changes a mailbox. Needs `servicedesk:configure`.
#[utoipa::path(put, path = "/api/v1/mailboxes/{id}", tag = "servicedesk", params(("id" = Uuid, Path)),
    request_body = MailboxInput,
    responses((status = 200, body = Mailbox), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_mailbox(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<MailboxInput>,
) -> ApiResult<Json<Mailbox>> {
    configure_mailbox(&state, &authed, id).await?;
    let ctx = authed.user.ctx.clone();
    let org = owner(&state, "sd_mailboxes", id).await?;
    let secret = seal(&state, org, id, body.password.as_deref()).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            mail::update(tx, &ctx, id, &body, secret.as_deref()).await
        })
        .await?,
    ))
}

/// Archives a mailbox: it takes no more mail. Needs `servicedesk:configure`.
#[utoipa::path(delete, path = "/api/v1/mailboxes/{id}", tag = "servicedesk", params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_mailbox(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    configure_mailbox(&state, &authed, id).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        mail::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct EmailListParams {
    /// 1 to 200 (default 50).
    pub limit: Option<i64>,
}

/// A mailbox's latest emails and what became of them. Needs
/// `servicedesk:configure`.
#[utoipa::path(get, path = "/api/v1/mailboxes/{id}/emails", tag = "servicedesk",
    params(("id" = Uuid, Path), EmailListParams),
    responses((status = 200, body = Vec<Email>)))]
pub async fn list_emails(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(params): Query<EmailListParams>,
) -> ApiResult<Json<Vec<Email>>> {
    configure_mailbox(&state, &authed, id).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            mail::emails(tx, id, params.limit.unwrap_or(50)).await
        })
        .await?,
    ))
}

/// What happened to a posted message.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageReceived {
    pub email_id: Uuid,
    /// The mailbox already had this message (by its `Message-ID`).
    pub duplicate: bool,
}

/// Delivers a raw message (`message/rfc822`) to a mailbox, as a mail
/// server's forwarding rule would. It's stored and `email.received` is
/// emitted for workflows such as *Email to ticket*. Needs `mailbox:ingest`.
#[utoipa::path(post, path = "/api/v1/mailboxes/{id}/messages", tag = "servicedesk",
    params(("id" = Uuid, Path)),
    request_body(content = Vec<u8>, content_type = "message/rfc822"),
    responses((status = 202, body = MessageReceived), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn post_message(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> ApiResult<(StatusCode, Json<MessageReceived>)> {
    authed.require_instance(permissions::MAILBOX_INGEST)?;
    let org = owner(&state, "sd_mailboxes", id).await?;
    require_module_at(&state, MODULE, org).await?;
    let mailbox = scoped(&state, &OrgScope::System, async |tx| {
        mail::mailbox(tx, id).await
    })
    .await?;
    if !mailbox.enabled {
        return Err(ApiError::conflict("This mailbox is turned off."));
    }
    let parsed = huxley_mail::parse(&body).ok_or_else(|| {
        ApiError::validation().with_field_error("/body", "not an email message (RFC 5322)")
    })?;
    let email_id = huxley_core::new_id();
    let key = mail::raw_key(mailbox.org_id, email_id);
    state
        .storage
        .put(&key, body.clone())
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let (stored, new) = scoped(&state, &OrgScope::System, async |tx| {
        mail::store(tx, &mailbox, email_id, &parsed, body.len()).await
    })
    .await?;
    if !new {
        // Kept once: drop the copy just stored.
        let _ = state.storage.delete(&key).await;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(MessageReceived {
            email_id: stored,
            duplicate: !new,
        }),
    ))
}

/// The module is on and the caller works tickets somewhere.
async fn response_reader(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    let granted = authed.principal.granted_anywhere();
    if granted.contains(&permissions::TICKET_WRITE)
        || granted.contains(&permissions::SERVICEDESK_CONFIGURE)
    {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

/// Canned responses, as written (placeholders and all).
#[utoipa::path(get, path = "/api/v1/canned-responses", tag = "servicedesk",
    responses((status = 200, body = Vec<CannedResponse>)))]
pub async fn list_responses(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<CannedResponse>>> {
    response_reader(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            responses::list(tx).await
        })
        .await?,
    ))
}

/// Adds a canned response. Needs `servicedesk:configure`.
#[utoipa::path(post, path = "/api/v1/canned-responses", tag = "servicedesk",
    request_body = CannedResponseInput,
    responses((status = 201, body = CannedResponse), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_response(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CannedResponseInput>,
) -> ApiResult<(StatusCode, Json<CannedResponse>)> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    require_module(&state, MODULE).await?;
    let ctx = authed.user.ctx.clone();
    let created = scoped(&state, &OrgScope::System, async |tx| {
        responses::create(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// Changes a canned response; send back the `version` you read.
#[utoipa::path(put, path = "/api/v1/canned-responses/{id}", tag = "servicedesk",
    params(("id" = Uuid, Path)), request_body = CannedResponseInput,
    responses((status = 200, body = CannedResponse), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_response(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CannedResponseInput>,
) -> ApiResult<Json<CannedResponse>> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    require_module(&state, MODULE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            responses::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives a canned response.
#[utoipa::path(delete, path = "/api/v1/canned-responses/{id}", tag = "servicedesk",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn archive_response(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    require_module(&state, MODULE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        responses::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Canned responses filled in for this ticket and you. Needs
/// `ticket:write` on the ticket.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/responses", tag = "servicedesk",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<CannedResponse>)))]
pub async fn ticket_responses(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<CannedResponse>>> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let ticket = scoped(&state, &scope, async |tx| tickets::get(tx, id).await).await?;
    let me = authed.user.auth.user.display_name.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            responses::for_ticket(tx, &ticket, &me).await
        })
        .await?,
    ))
}

/// The other ticket a link or merge names: by id, or by reference among
/// the tickets you can work on.
async fn other_ticket(
    state: &AppState,
    authed: &Authed,
    id: Option<Uuid>,
    reference: Option<&str>,
    field: &str,
) -> ApiResult<Uuid> {
    let other = match (id, reference.map(str::trim).filter(|r| !r.is_empty())) {
        (Some(id), _) => id,
        (None, Some(reference)) => {
            let scope = authed
                .module_scope(state, permissions::TICKET_WRITE, MODULE)
                .await?;
            scoped(state, &scope, async |tx| {
                links::by_reference(tx, field, reference).await
            })
            .await?
        }
        (None, None) => {
            return Err(
                ApiError::validation().with_field_error(format!("/{field}"), "which ticket")
            );
        }
    };
    ticket_access(state, authed, other, permissions::TICKET_WRITE).await?;
    Ok(other)
}

/// Tickets linked to this one: its parent, children and related tickets.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/links", tag = "servicedesk",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<LinkedTicket>)))]
pub async fn list_links(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<LinkedTicket>>> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| links::list(tx, id).await).await?,
    ))
}

/// Links another ticket (by id or reference) as related, this one's parent,
/// or its child. Needs `ticket:write` on both.
#[utoipa::path(post, path = "/api/v1/tickets/{id}/links", tag = "servicedesk",
    params(("id" = Uuid, Path)), request_body = NewLink,
    responses((status = 200, body = Vec<LinkedTicket>), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_link(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<NewLink>,
) -> ApiResult<Json<Vec<LinkedTicket>>> {
    ticket_access(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let other = other_ticket(
        &state,
        &authed,
        body.other_id,
        body.reference.as_deref(),
        "reference",
    )
    .await?;
    let kind = body.kind.unwrap_or_else(|| "related".into());
    let ctx = authed.user.ctx.clone();
    // Both are yours to change; the links of either side may be in
    // organizations the other's scope doesn't cover.
    scoped(&state, &OrgScope::System, async |tx| {
        links::link(tx, &ctx, id, other, &kind).await
    })
    .await?;
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_READ).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| links::list(tx, id).await).await?,
    ))
}

/// Removes a link (from both tickets).
#[utoipa::path(delete, path = "/api/v1/tickets/{id}/links/{other_id}", tag = "servicedesk",
    params(("id" = Uuid, Path), ("other_id" = Uuid, Path)), responses((status = 204)))]
pub async fn remove_link(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, other)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    ticket_access(&state, &authed, id, permissions::TICKET_WRITE).await?;
    ticket_access(&state, &authed, other, permissions::TICKET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        links::unlink(tx, &ctx, id, other).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MergeInto {
    /// The ticket to merge this one into, by id…
    #[serde(default)]
    pub into_id: Option<Uuid>,
    /// …or by reference (e.g. `T-1042`).
    #[serde(default)]
    pub into: Option<String>,
}

/// Merges this ticket into another of the same organization: its notes,
/// attachments and emails move there, and this one is closed. Needs
/// `ticket:write` on both. Returns the ticket it was merged into.
#[utoipa::path(post, path = "/api/v1/tickets/{id}/merge", tag = "servicedesk",
    params(("id" = Uuid, Path)), request_body = MergeInto,
    responses((status = 200, body = Ticket), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn merge_ticket(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<MergeInto>,
) -> ApiResult<Json<Ticket>> {
    let scope = ticket_access(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let into = other_ticket(&state, &authed, body.into_id, body.into.as_deref(), "into").await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            links::merge(tx, &ctx, id, into).await
        })
        .await?,
    ))
}

/// The module is on and the caller reads tickets somewhere.
async fn view_user(state: &AppState, authed: &Authed) -> ApiResult<Uuid> {
    require_module(state, MODULE).await?;
    if authed
        .principal
        .granted_anywhere()
        .contains(&permissions::TICKET_READ)
    {
        Ok(authed.user.auth.user.id)
    } else {
        Err(ApiError::forbidden())
    }
}

/// Your saved ticket views and the shared ones.
#[utoipa::path(get, path = "/api/v1/ticket-views", tag = "servicedesk",
    responses((status = 200, body = Vec<TicketView>)))]
pub async fn list_views(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<TicketView>>> {
    let me = view_user(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            views::list(tx, me).await
        })
        .await?,
    ))
}

/// Saves a view of the Tickets list: yours, or `shared` (needs
/// `servicedesk:configure`).
#[utoipa::path(post, path = "/api/v1/ticket-views", tag = "servicedesk",
    request_body = TicketViewInput,
    responses((status = 201, body = TicketView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_view(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<TicketViewInput>,
) -> ApiResult<(StatusCode, Json<TicketView>)> {
    let me = view_user(&state, &authed).await?;
    if body.shared {
        authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    }
    let owner = (!body.shared).then_some(me);
    let ctx = authed.user.ctx.clone();
    let view = scoped(&state, &OrgScope::System, async |tx| {
        views::create(tx, &ctx, owner, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// Shared views are changed only by service desk configurers.
async fn may_change_view(state: &AppState, authed: &Authed, me: Uuid, id: Uuid) -> ApiResult<()> {
    let (view, _) = scoped(state, &OrgScope::System, async |tx| {
        views::get(tx, me, id).await
    })
    .await?;
    if view.shared {
        authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    }
    Ok(())
}

/// Renames a view or changes its filters; send back the `version` you read.
#[utoipa::path(put, path = "/api/v1/ticket-views/{id}", tag = "servicedesk",
    params(("id" = Uuid, Path)), request_body = TicketViewInput,
    responses((status = 200, body = TicketView), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_view(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<TicketViewInput>,
) -> ApiResult<Json<TicketView>> {
    let me = view_user(&state, &authed).await?;
    may_change_view(&state, &authed, me, id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            views::update(tx, &ctx, me, id, &body).await
        })
        .await?,
    ))
}

/// Deletes a view.
#[utoipa::path(delete, path = "/api/v1/ticket-views/{id}", tag = "servicedesk",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_view(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let me = view_user(&state, &authed).await?;
    may_change_view(&state, &authed, me, id).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        views::delete(tx, &ctx, me, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Most tickets one bulk edit changes.
const BULK_MAX: usize = 100;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BulkUpdate {
    /// The tickets (at most 100).
    pub ids: Vec<Uuid>,
    /// Field changes for all of them.
    #[serde(default)]
    pub changes: Option<tickets::BulkFields>,
    /// Then a status for all of them (a status key, e.g. `in_progress`).
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkResult {
    pub id: Uuid,
    pub ok: bool,
    /// Why it wasn't changed.
    pub error: Option<String>,
}

/// Applies the same changes to many tickets, each on its own: those you
/// can't change, or that can't take a change (a status they can't move to,
/// say), are reported and the rest are changed. Needs `ticket:write` on
/// each.
#[utoipa::path(post, path = "/api/v1/tickets/bulk", tag = "servicedesk", request_body = BulkUpdate,
    responses((status = 200, body = Vec<BulkResult>), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn bulk_update(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<BulkUpdate>,
) -> ApiResult<Json<Vec<BulkResult>>> {
    require_module(&state, MODULE).await?;
    let changes = body.changes.unwrap_or_default();
    let status = body.status.filter(|s| !s.trim().is_empty());
    if body.ids.is_empty() || body.ids.len() > BULK_MAX {
        return Err(ApiError::validation().with_field_error("/ids", "1 to 100 tickets"));
    }
    if changes.is_empty() && status.is_none() {
        return Err(ApiError::validation().with_field_error("/changes", "something to change"));
    }
    let ctx = authed.user.ctx.clone();
    let mut results = Vec::with_capacity(body.ids.len());
    let mut seen = std::collections::BTreeSet::new();
    for id in body.ids {
        if !seen.insert(id) {
            continue;
        }
        let outcome: ApiResult<()> = async {
            let scope = ticket_access(&state, &authed, id, permissions::TICKET_WRITE).await?;
            scoped(&state, &scope, async |tx| {
                let mut version = tickets::get(tx, id).await?.version;
                if !changes.is_empty() {
                    version = tickets::update(tx, &ctx, id, &changes.at(version))
                        .await?
                        .version;
                }
                if let Some(status) = &status {
                    let change = StatusChange {
                        status: status.clone(),
                        resolution: None,
                        version,
                    };
                    tickets::change_status(tx, &ctx, id, &change).await?;
                }
                Ok(())
            })
            .await
        }
        .await;
        results.push(match outcome {
            Ok(()) => BulkResult {
                id,
                ok: true,
                error: None,
            },
            Err(e) => BulkResult {
                id,
                ok: false,
                error: Some(e.summary()),
            },
        });
    }
    Ok(Json(results))
}
