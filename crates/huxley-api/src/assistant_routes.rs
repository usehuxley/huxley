//! `/api/v1/assistant` (design doc §10.3): your conversations with the
//! built-in assistant. It uses the organization's AI provider and acts with
//! your own permissions; tools that change something wait for your say.
//! Needs `ai:use` at the organization.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use huxley_authz::permissions;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::assistant::{self, Conversation, ConversationSummary, Decisions};
use crate::authz::Authed;
use crate::mcp::{Caller, Internal};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(list).post(start))
        .route("/conversations/{id}", get(detail).delete(remove))
        .route("/conversations/{id}/messages", post(message))
        .route("/conversations/{id}/decisions", post(decide))
}

fn user(authed: &Authed) -> Uuid {
    authed.user.auth.user.id
}

/// Its organization, after checking the caller may still use it there.
async fn allowed(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<Conversation> {
    let conversation = assistant::view(state, user(authed), id).await?;
    authed
        .require_at(state, permissions::AI_USE, conversation.org_id)
        .await?;
    Ok(conversation)
}

const MAX_TEXT: usize = 20_000;

fn check_text(text: &str) -> ApiResult<&str> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_TEXT {
        return Err(
            ApiError::validation().with_field_error("text", format!("1 to {MAX_TEXT} characters"))
        );
    }
    Ok(text)
}

/// Your conversations, newest first.
#[utoipa::path(get, path = "/api/v1/assistant/conversations", operation_id = "list_assistant_conversations", tag = "assistant",
    responses((status = 200, body = Vec<ConversationSummary>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<ConversationSummary>>> {
    Ok(Json(assistant::list(&state, user(&authed)).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartConversation {
    /// The organization it works in (whose AI provider it uses).
    pub org_id: Uuid,
    pub text: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SayRequest {
    pub text: String,
}

/// Starts a conversation with a first message, and returns it answered
/// (or waiting for your approval of an action).
#[utoipa::path(post, path = "/api/v1/assistant/conversations", operation_id = "start_assistant_conversation", tag = "assistant",
    request_body = StartConversation,
    responses((status = 201, body = Conversation), (status = 502, body = ApiError, content_type = "application/problem+json")))]
pub async fn start(
    State(state): State<AppState>,
    Extension(Internal(internal)): Extension<Internal>,
    authed: Authed,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<StartConversation>,
) -> ApiResult<(StatusCode, Json<Conversation>)> {
    authed
        .require_at(&state, permissions::AI_USE, body.org_id)
        .await?;
    let text = check_text(&body.text)?;
    // Say so up front, rather than start a conversation that can't answer.
    let mut tx = huxley_db::begin_scoped(&state.db, &huxley_db::OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let provider = huxley_ai::store::effective(&mut tx, body.org_id)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    if provider.is_none() {
        return Err(ApiError::conflict(
            "Set up an AI provider first (Settings → AI): the assistant uses your own key.",
        ));
    }
    let title: String = text.chars().take(80).collect();
    let id = assistant::create(&state, user(&authed), body.org_id, &title).await?;
    let caller = Caller::new(&internal, &headers, peer.map(|Extension(p)| p));
    assistant::say(&state, &caller, user(&authed), id, text).await?;
    Ok((
        StatusCode::CREATED,
        Json(assistant::view(&state, user(&authed), id).await?),
    ))
}

/// One of your conversations.
#[utoipa::path(get, path = "/api/v1/assistant/conversations/{id}", operation_id = "get_assistant_conversation", tag = "assistant",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Conversation), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn detail(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Conversation>> {
    Ok(Json(allowed(&state, &authed, id).await?))
}

/// Sends a message, and returns the conversation answered (or waiting for
/// your approval of an action).
#[utoipa::path(post, path = "/api/v1/assistant/conversations/{id}/messages", operation_id = "send_assistant_message", tag = "assistant",
    params(("id" = Uuid, Path)), request_body = SayRequest,
    responses((status = 200, body = Conversation), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 502, body = ApiError, content_type = "application/problem+json")))]
pub async fn message(
    State(state): State<AppState>,
    Extension(Internal(internal)): Extension<Internal>,
    authed: Authed,
    Path(id): Path<Uuid>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<SayRequest>,
) -> ApiResult<Json<Conversation>> {
    allowed(&state, &authed, id).await?;
    let text = check_text(&body.text)?;
    let caller = Caller::new(&internal, &headers, peer.map(|Extension(p)| p));
    assistant::say(&state, &caller, user(&authed), id, text).await?;
    Ok(Json(assistant::view(&state, user(&authed), id).await?))
}

/// Approves or denies the actions waiting for you, and lets the assistant
/// carry on.
#[utoipa::path(post, path = "/api/v1/assistant/conversations/{id}/decisions", operation_id = "decide_assistant_actions", tag = "assistant",
    params(("id" = Uuid, Path)), request_body = Decisions,
    responses((status = 200, body = Conversation), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn decide(
    State(state): State<AppState>,
    Extension(Internal(internal)): Extension<Internal>,
    authed: Authed,
    Path(id): Path<Uuid>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<Decisions>,
) -> ApiResult<Json<Conversation>> {
    allowed(&state, &authed, id).await?;
    let caller = Caller::new(&internal, &headers, peer.map(|Extension(p)| p));
    assistant::decide(&state, &caller, user(&authed), id, &body).await?;
    Ok(Json(assistant::view(&state, user(&authed), id).await?))
}

/// Deletes one of your conversations.
#[utoipa::path(delete, path = "/api/v1/assistant/conversations/{id}", operation_id = "delete_assistant_conversation", tag = "assistant",
    params(("id" = Uuid, Path)), responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn remove(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    assistant::delete(&state, user(&authed), id).await?;
    Ok(StatusCode::NO_CONTENT)
}
