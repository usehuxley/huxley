//! `/api/v1/notifications`: the signed-in user's notifications, preferences,
//! and a real-time stream (Server-Sent Events).

use std::convert::Infallible;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures_util::Stream;
use huxley_core::{Cursor, PageRequest};
use huxley_db::{OrgScope, begin_scoped};
use huxley_notify::{Channels, Notification, NotifyError, Preference};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::session::CurrentUser;
use crate::{ApiError, ApiResult, AppState};

/// A stream ends after this long; the browser reconnects (`EventSource`
/// does so automatically), which re-checks the session.
const STREAM_LIFETIME: Duration = Duration::from_secs(600);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/unread-count", get(unread_count))
        .route("/stream", get(stream))
        .route("/read-all", post(read_all))
        .route("/{id}/read", post(read_one))
        .route("/preferences", get(preferences))
        .route("/preferences/{kind}", put(set_preference))
}

fn map_notify(e: NotifyError) -> ApiError {
    match e {
        NotifyError::NotFound | NotifyError::UnknownKind(_) => ApiError::not_found(),
        NotifyError::Mandatory(_) => ApiError::conflict("Security notices can't be turned off."),
        NotifyError::Db(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct NotificationParams {
    /// Page size, 1 to 200 (default 50).
    pub limit: Option<u32>,
    /// Opaque cursor from the previous page's `nextCursor`.
    pub cursor: Option<String>,
    /// Only unread notifications.
    #[serde(default)]
    pub unread_only: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPage {
    pub items: Vec<Notification>,
    pub next_cursor: Option<String>,
    pub unread_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnreadCount {
    pub unread_count: i64,
}

/// Your notifications, newest first.
#[utoipa::path(get, path = "/api/v1/notifications", operation_id = "list_notifications", tag = "notifications",
    params(NotificationParams),
    responses((status = 200, body = NotificationPage)))]
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<NotificationParams>,
) -> ApiResult<Json<NotificationPage>> {
    let page = PageRequest {
        limit: params.limit,
        cursor: params.cursor.map(Cursor::from),
    };
    let before: Option<Uuid> = page.cursor.as_ref().map(Cursor::decode).transpose()?;
    let limit = page.limit();
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    let items = huxley_notify::list(
        &mut conn,
        user.auth.user.id,
        before,
        limit,
        params.unread_only,
    )
    .await
    .map_err(map_notify)?;
    let unread_count = huxley_notify::unread_count(&mut conn, user.auth.user.id)
        .await
        .map_err(map_notify)?;
    let page = huxley_core::Page::from_overfetch(items, limit, |n| n.id);
    Ok(Json(NotificationPage {
        items: page.items,
        next_cursor: page.next_cursor.map(|c| c.as_str().to_owned()),
        unread_count,
    }))
}

#[utoipa::path(get, path = "/api/v1/notifications/unread-count", tag = "notifications",
    responses((status = 200, body = UnreadCount)))]
pub async fn unread_count(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<UnreadCount>> {
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    let unread_count = huxley_notify::unread_count(&mut conn, user.auth.user.id)
        .await
        .map_err(map_notify)?;
    Ok(Json(UnreadCount { unread_count }))
}

#[utoipa::path(post, path = "/api/v1/notifications/{id}/read", tag = "notifications",
    params(("id" = Uuid, Path, description = "Notification id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn read_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    huxley_notify::mark_read(&mut conn, user.auth.user.id, id)
        .await
        .map_err(map_notify)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/notifications/read-all", tag = "notifications",
    responses((status = 204)))]
pub async fn read_all(State(state): State<AppState>, user: CurrentUser) -> ApiResult<StatusCode> {
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    huxley_notify::mark_all_read(&mut conn, user.auth.user.id)
        .await
        .map_err(map_notify)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Your channel choices for each kind of notification.
#[utoipa::path(get, path = "/api/v1/notifications/preferences", tag = "notifications",
    responses((status = 200, body = Vec<Preference>)))]
pub async fn preferences(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<Preference>>> {
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    let all = huxley_notify::preferences(&mut conn, user.auth.user.id)
        .await
        .map_err(map_notify)?;
    drop(conn);
    // Kinds of modules that are off aren't offered.
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let enabled = huxley_modules::enabled_set(&mut tx).await.map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(
        all.into_iter()
            .filter(|p| p.module.is_none_or(|m| enabled.contains(m)))
            .collect(),
    ))
}

/// Chooses the channels for one kind. Security notices can't be changed.
#[utoipa::path(put, path = "/api/v1/notifications/preferences/{kind}", tag = "notifications",
    params(("kind" = String, Path, description = "Notification kind")),
    request_body = Channels,
    responses((status = 204), (status = 409, description = "Mandatory kind", body = ApiError, content_type = "application/problem+json")))]
pub async fn set_preference(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(kind): Path<String>,
    Json(body): Json<Channels>,
) -> ApiResult<StatusCode> {
    let mut conn = state.db.acquire().await.map_err(db_err)?;
    huxley_notify::set_preference(&mut conn, user.auth.user.id, &kind, body)
        .await
        .map_err(map_notify)?;
    Ok(StatusCode::NO_CONTENT)
}

/// An `unread` event with the user's current count.
async fn count_event(db: &sqlx::PgPool, user_id: Uuid) -> Option<Event> {
    let mut conn = db.acquire().await.ok()?;
    let count = huxley_notify::unread_count(&mut conn, user_id).await.ok()?;
    Some(Event::default().event("unread").data(count.to_string()))
}

/// Server-Sent Events: an `unread` event with your unread count when the
/// stream opens and whenever a notification arrives. Closes after ten minutes
/// (reconnect to continue).
#[utoipa::path(get, path = "/api/v1/notifications/stream", tag = "notifications",
    responses((status = 200, description = "text/event-stream of `unread` events", content_type = "text/event-stream")))]
pub async fn stream(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let user_id = user.auth.user.id;
    let rx = state.notifications.subscribe();
    let db = state.db.clone();
    let deadline = Instant::now() + STREAM_LIFETIME;

    let stream = futures_util::stream::unfold((rx, true), move |(mut rx, first)| {
        let db = db.clone();
        async move {
            if first {
                let event = count_event(&db, user_id).await?;
                return Some((Ok(event), (rx, false)));
            }
            loop {
                let remaining = deadline.checked_duration_since(Instant::now())?;
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Err(_) | Ok(Err(RecvError::Closed)) => return None,
                    Ok(Ok(id)) if id != user_id => {}
                    // Ours, or we fell behind and may have missed ours.
                    Ok(Ok(_) | Err(RecvError::Lagged(_))) => {
                        let event = count_event(&db, user_id).await?;
                        return Some((Ok(event), (rx, false)));
                    }
                }
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
