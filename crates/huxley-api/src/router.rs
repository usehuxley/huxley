use std::any::Any;
use std::time::Duration;

use axum::Router;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http::{HeaderName, Request, StatusCode, header};
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::{
    ApiError, AppState, audit_routes, auth_routes, csrf, entity_routes, health, hook_routes,
    mfa_routes, module_routes, notification_routes, openapi, org_routes, passkey_routes,
    security_routes, sso_routes, team_routes, token_routes, user_routes, workflow_routes,
};

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Sync webhooks wait up to 5 minutes for their run.
const HOOK_TIMEOUT: Duration = Duration::from_secs(310);
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Builds the full API router with the standard middleware stack.
pub fn router(state: AppState) -> Router {
    let body_limit = MAX_BODY_BYTES.max(
        usize::try_from(state.storage.max_upload_bytes())
            .unwrap_or(usize::MAX)
            .saturating_add(1024 * 1024),
    );
    let api_v1 = Router::new()
        .route("/openapi.json", get(openapi::openapi_json))
        .nest("/auth", auth_routes::routes())
        .nest("/auth/mfa", mfa_routes::routes())
        .nest("/auth/passkeys", passkey_routes::routes())
        .nest("/auth/oidc", sso_routes::sign_in_routes())
        .nest("/auth/tokens", token_routes::own_routes())
        .nest("/service-accounts", token_routes::service_account_routes())
        .nest("/security/sso/providers", sso_routes::admin_routes())
        .nest("/orgs", org_routes::routes())
        .nest("/audit", audit_routes::routes())
        .nest("/modules", module_routes::routes())
        .nest("/teams", team_routes::routes())
        .nest("/notifications", notification_routes::routes())
        .nest("/workflows", workflow_routes::workflow_routes())
        .nest("/runs", workflow_routes::run_routes())
        .nest("/security", security_routes::routes())
        .merge(user_routes::routes())
        .merge(entity_routes::routes())
        .merge(module_routes::org_routes())
        .merge(entity_routes::attachment_routes(
            usize::try_from(state.storage.max_upload_bytes())
                .unwrap_or(usize::MAX)
                .saturating_add(64 * 1024),
        ))
        .layer(middleware::from_fn(csrf::require_header));

    let stack = ServiceBuilder::new()
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
        ]))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
        .layer(TraceLayer::new_for_http().make_span_with(make_span))
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(CatchPanicLayer::custom(handle_panic))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        // The hard cap fits the largest upload. JSON bodies keep axum's own
        // 2 MB extractor limit; only the upload route raises it.
        .layer(RequestBodyLimitLayer::new(body_limit));

    let main = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .nest("/api/v1", api_v1)
        .fallback(not_found)
        .layer(stack);

    // Webhooks: outside /api/v1 (no sessions or CSRF), with a longer timeout
    // for sync responses and a smaller body limit.
    let hooks = Router::new()
        .route(
            "/hooks/{public_id}",
            axum::routing::post(hook_routes::receive),
        )
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
                .layer(TraceLayer::new_for_http().make_span_with(make_span))
                .layer(PropagateRequestIdLayer::new(REQUEST_ID))
                .layer(CatchPanicLayer::custom(handle_panic))
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    HOOK_TIMEOUT,
                ))
                .layer(RequestBodyLimitLayer::new(hook_routes::MAX_BODY)),
        );

    Router::new().merge(hooks).merge(main).with_state(state)
}

/// One span per request, continuing the caller's trace if it sent `traceparent`.
fn make_span<B>(request: &Request<B>) -> tracing::Span {
    let request_id = request
        .headers()
        .get(&REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let span = tracing::info_span!(
        "http.request",
        otel.name = %format!("{} {}", request.method(), request.uri().path()),
        http.request.method = %request.method(),
        url.path = %request.uri().path(),
        request_id = %request_id,
    );
    huxley_telemetry::set_parent_from_headers(&span, request.headers());
    span
}

async fn not_found() -> ApiError {
    ApiError::not_found()
}

fn handle_panic(_payload: Box<dyn Any + Send + 'static>) -> Response {
    tracing::error!("request handler panicked");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "Internal server error",
    )
    .into_response()
}
