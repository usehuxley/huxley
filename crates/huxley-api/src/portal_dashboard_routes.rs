//! Dashboards in the client portal (ADR-040, ADR-041): shared dashboards
//! published to the portal, for client admins, with their company's numbers
//! only. Widgets over other sources (or modules that are off) show as
//! unavailable.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use huxley_analytics::dashboards::{self, Dashboard, PORTAL_SOURCES};
use huxley_analytics::{Answer, Scope};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::Perm;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::analytics_routes::map;
use crate::portal_routes::PortalAuthed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dashboards", get(list_dashboards))
        .route("/dashboards/{id}", get(view_dashboard))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn allowed(authed: &PortalAuthed) -> ApiResult<()> {
    if authed.principal.can(Perm::Dashboards) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Dashboards are for your organization's admins",
        ))
    }
}

async fn published(state: &AppState, org: Uuid) -> ApiResult<Vec<Dashboard>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let all = dashboards::for_portal(&mut tx, org).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(all)
}

/// The dashboards published to your company's portal.
#[utoipa::path(get, path = "/api/portal/v1/dashboards", operation_id = "portal_dashboards", tag = "portal",
    responses((status = 200, body = Vec<Dashboard>)))]
pub async fn list_dashboards(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Vec<Dashboard>>> {
    allowed(&authed)?;
    Ok(Json(published(&state, authed.principal.org_id).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PortalDashboardParams {
    /// A named range, e.g. `last_30_days` (default: the dashboard's).
    pub range: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalWidgetResult {
    pub answer: Option<Answer>,
    pub problem: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalDashboardView {
    pub dashboard: Dashboard,
    pub range: String,
    /// By widget id.
    pub results: BTreeMap<String, PortalWidgetResult>,
}

/// A published dashboard with your company's numbers.
#[utoipa::path(get, path = "/api/portal/v1/dashboards/{id}", operation_id = "portal_dashboard", tag = "portal",
    params(("id" = Uuid, Path), PortalDashboardParams), responses((status = 200, body = PortalDashboardView)))]
pub async fn view_dashboard(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
    Query(params): Query<PortalDashboardParams>,
) -> ApiResult<Json<PortalDashboardView>> {
    allowed(&authed)?;
    let org = authed.principal.org_id;
    let dashboard = published(&state, org)
        .await?
        .into_iter()
        .find(|d| d.id == id.to_string())
        .ok_or_else(ApiError::not_found)?;
    let range = params
        .range
        .filter(|r| huxley_analytics::query::ranges().any(|k| k == r))
        .unwrap_or_else(|| dashboard.range.clone());
    let pool = state.read_db.as_ref().unwrap_or(&state.db);
    let mut results = BTreeMap::new();
    for widget in &dashboard.widgets {
        let source = huxley_analytics::catalog::source(&widget.spec.source);
        let on = match source.and_then(|s| s.module) {
            Some(module) if PORTAL_SOURCES.contains(&widget.spec.source.as_str()) => {
                let mut tx = begin_scoped(&state.db, &OrgScope::System)
                    .await
                    .map_err(db_err)?;
                let on = huxley_modules::is_active_at(&mut tx, module, org)
                    .await
                    .map_err(db_err)?;
                tx.commit().await.map_err(db_err)?;
                on
            }
            _ => false,
        };
        let result = if on {
            let spec = dashboards::with_range(widget, &range);
            match huxley_analytics::run(pool, &state.analytics, &spec, &Scope::Orgs(vec![org]))
                .await
            {
                Ok(answer) => PortalWidgetResult {
                    answer: Some(answer),
                    problem: None,
                },
                Err(huxley_analytics::AnalyticsError::TooSlow) => PortalWidgetResult {
                    answer: None,
                    problem: Some("This took too long; try a shorter range.".into()),
                },
                Err(e) => return Err(map(e)),
            }
        } else {
            PortalWidgetResult {
                answer: None,
                problem: Some("Not available here.".into()),
            }
        };
        results.insert(widget.id.clone(), result);
    }
    Ok(Json(PortalDashboardView {
        dashboard,
        range,
        results,
    }))
}
