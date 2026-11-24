//! Router tests that need no database. The pool points at a closed port, so
//! `/readyz` must report unavailable rather than hang or panic.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_crypto::{Crypto, KeyRing};
use huxley_db::DatabaseConfig;
use secrecy::SecretString;
use tower::ServiceExt as _;

fn app() -> axum::Router {
    let config = DatabaseConfig {
        url: SecretString::from("postgres://huxley@127.0.0.1:1/huxley"),
        max_connections: 1,
        acquire_timeout_secs: 1,
        statement_timeout_ms: 1_000,
    };
    let db = huxley_db::connect(&config).expect("lazy pool");
    let auth = AuthService::new(
        db.clone(),
        AuthConfig::default(),
        Crypto::new(db.clone(), KeyRing::ephemeral()),
    );
    let crypto = Crypto::new(db.clone(), KeyRing::ephemeral());
    router(AppState::new(db, auth, crypto, ApiConfig::default()))
}

async fn send(uri: &str) -> (StatusCode, http::HeaderMap, serde_json::Value) {
    let response = app()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, headers, json)
}

#[tokio::test]
async fn healthz_is_ok() {
    let (status, headers, body) = send("/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(
        headers.contains_key("x-request-id"),
        "request id is propagated"
    );
}

#[tokio::test]
async fn readyz_reports_unavailable_database() {
    let (status, headers, body) = send("/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(headers["content-type"], "application/problem+json");
    assert_eq!(body["type"], "https://usehuxley.io/problems/unavailable");
    assert_eq!(body["status"], 503);
}

#[tokio::test]
async fn unknown_route_is_problem_json() {
    let (status, headers, body) = send("/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(headers["content-type"], "application/problem+json");
    assert_eq!(body["title"], "Resource not found");
}

#[tokio::test]
async fn openapi_document_lists_system_routes() {
    let (status, _, body) = send("/api/v1/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["paths"]["/healthz"].is_object());
    assert!(body["paths"]["/readyz"].is_object());
}

#[tokio::test]
async fn state_changing_requests_need_the_csrf_header() {
    let request = Request::post("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"email":"a@example.com","password":"x"}"#))
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["type"], "https://usehuxley.io/problems/csrf");
}

#[tokio::test]
async fn me_without_a_session_is_unauthorized() {
    let (status, headers, body) = send("/api/v1/auth/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers["content-type"], "application/problem+json");
    assert_eq!(body["type"], "https://usehuxley.io/problems/unauthorized");
}

#[tokio::test]
async fn logout_without_a_session_still_clears_the_cookie() {
    let request = Request::post("/api/v1/auth/logout")
        .header("x-requested-with", "huxley")
        .body(Body::empty())
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with("hx_session=; Path=/; Max-Age=0"));
}

#[test]
fn openapi_operation_ids_are_unique() {
    // Duplicate ids silently merge operations in the generated TypeScript client.
    let doc: serde_json::Value =
        serde_json::from_str(&huxley_api::openapi::to_pretty_json()).unwrap();
    let mut ids: Vec<String> = doc["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|ops| ops.as_object().unwrap().values())
        .filter_map(|op| op["operationId"].as_str().map(str::to_owned))
        .collect();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "duplicate OpenAPI operationId");
}
