//! Calendar sync settings (ADR-061), under `/api/v1/workforce/calendars`:
//! how Microsoft 365 is reached, and each person's calendar (their address
//! there, or an iCalendar feed). Settings need `workforce:manage`; people
//! can set their own calendar.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_workforce::calendar::{
    self, CalendarPerson, CalendarPersonInput, Settings, SettingsInput,
};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module;
use crate::workforce_routes::system;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/calendars", get(overview))
        .route(
            "/calendars/settings",
            put(set_settings).delete(clear_settings),
        )
        .route("/calendars/people/{user_id}", put(set_person))
}

async fn manager(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, huxley_workforce::MODULE).await?;
    authed.require_instance(permissions::WORKFORCE_MANAGE)
}

/// How calendars are synced, and everyone's.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CalendarOverview {
    /// Microsoft 365, if it's set up.
    pub settings: Option<Settings>,
    pub people: Vec<CalendarPerson>,
}

#[utoipa::path(get, path = "/api/v1/workforce/calendars", operation_id = "calendar_overview", tag = "workforce",
    responses((status = 200, body = CalendarOverview)))]
pub async fn overview(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<CalendarOverview>> {
    manager(&state, &authed).await?;
    let (settings, people) = system(&state, async |tx| {
        Ok((
            calendar::settings(&mut *tx).await?,
            calendar::people(tx).await?,
        ))
    })
    .await?;
    Ok(Json(CalendarOverview { settings, people }))
}

/// Sets up Microsoft 365: an `msgraph` connection at an organization. Busy
/// time is read with its free/busy schedule.
#[utoipa::path(put, path = "/api/v1/workforce/calendars/settings", operation_id = "set_calendar_settings", tag = "workforce",
    request_body = SettingsInput,
    responses((status = 200, body = Settings), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_settings(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<SettingsInput>,
) -> ApiResult<Json<Settings>> {
    manager(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    let saved = system(&state, async |tx| {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM connections c JOIN integrations i ON i.id = c.integration_id \
              WHERE c.org_id = $1 AND c.key = $2 AND i.key = 'msgraph')",
        )
        .bind(body.org_id)
        .bind(body.connection.trim())
        .fetch_one(&mut *tx)
        .await?;
        if !found {
            return Err(huxley_workforce::WorkforceError::invalid(
                "connection",
                "a Microsoft Graph connection at that organization",
            ));
        }
        calendar::set_settings(tx, &ctx, &body).await
    })
    .await?;
    Ok(Json(saved))
}

/// Stops reading Microsoft 365 (feeds keep syncing).
#[utoipa::path(delete, path = "/api/v1/workforce/calendars/settings", operation_id = "clear_calendar_settings", tag = "workforce",
    responses((status = 204)))]
pub async fn clear_settings(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<StatusCode> {
    manager(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    system(&state, async |tx| calendar::clear_settings(tx, &ctx).await).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sets how someone's calendar is synced: from Microsoft 365 (at their
/// email, or `calendar`), from an iCalendar feed (`icsUrl`, kept sealed and
/// never shown again), or not at all. Yours, or with `workforce:manage`.
#[utoipa::path(put, path = "/api/v1/workforce/calendars/people/{user_id}", operation_id = "set_calendar_person", tag = "workforce",
    params(("user_id" = Uuid, Path)), request_body = CalendarPersonInput,
    responses((status = 200, body = CalendarPerson), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_person(
    State(state): State<AppState>,
    authed: Authed,
    Path(user_id): Path<Uuid>,
    Json(body): Json<CalendarPersonInput>,
) -> ApiResult<Json<CalendarPerson>> {
    require_module(&state, huxley_workforce::MODULE).await?;
    if authed.user.auth.user.id != user_id
        && !authed.principal.can_instance(permissions::WORKFORCE_MANAGE)
    {
        return Err(ApiError::forbidden());
    }
    // Sealed before the transaction: the first seal at an organization
    // audits in a transaction of its own.
    let sealed = match body
        .ics_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        Some(url) => {
            calendar::check_feed(url).map_err(crate::workforce_routes::map)?;
            let root = system(&state, async |tx| {
                huxley_orgs::root(tx)
                    .await
                    .map_err(|e| {
                        huxley_workforce::WorkforceError::invalid("organization", e.to_string())
                    })?
                    .ok_or(huxley_workforce::WorkforceError::NotFound)
            })
            .await?;
            Some(
                state
                    .crypto
                    .encrypt(root.id, calendar::ics_binding(user_id), url.as_bytes())
                    .await
                    .map_err(|e| ApiError::internal(&e))?,
            )
        }
        None => None,
    };
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            calendar::set_person(tx, &ctx, user_id, &body, sealed).await
        })
        .await?,
    ))
}
