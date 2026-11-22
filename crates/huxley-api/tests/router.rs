//! Router tests that need no database. The pool points at a closed port, so
//! `/readyz` must report unavailable rather than hang or panic.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{AppState, router};
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
    router(AppState { db })
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
    assert_eq!(body["type"], "https://usehuxley.com/problems/unavailable");
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
