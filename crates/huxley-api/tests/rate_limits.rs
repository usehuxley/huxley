//! Rate limits are shared by every API replica (ADR-090): two routers on
//! one database count one address's sign-in attempts together.

use std::time::Duration;

use axum::body::Body;
use http::{Request, StatusCode};
use huxley_api::rate_limit::RateLimiter;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, pools};
use serde_json::json;
use tower::ServiceExt as _;

fn replica(pools: &TestPools) -> axum::Router {
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto.clone());
    let config = ApiConfig {
        login_attempts_per_minute: 3,
        trust_forwarded_for: true,
        ..ApiConfig::default()
    };
    router(AppState::new(pools.app.clone(), auth, crypto, config))
}

/// An address no other test uses.
fn address() -> String {
    let n = uuid::Uuid::now_v7().as_u128();
    format!("2001:db8:{:x}:{:x}::1", (n >> 16) & 0xffff, n & 0xffff)
}

async fn sign_in(app: &axum::Router, ip: &str) -> StatusCode {
    let req = Request::post("/api/v1/auth/login")
        .header("x-requested-with", "huxley")
        .header("content-type", "application/json")
        .header("x-forwarded-for", ip)
        .body(Body::from(
            json!({ "email": "nobody@example.com", "password": "wrong password here" }).to_string(),
        ))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn replicas_share_one_limit_per_address() {
    let Some(pools) = pools().await else { return };
    let (a, b) = (replica(&pools), replica(&pools));
    let ip = address();
    // Three attempts across both replicas, then refused on either.
    assert_ne!(sign_in(&a, &ip).await, StatusCode::TOO_MANY_REQUESTS);
    assert_ne!(sign_in(&b, &ip).await, StatusCode::TOO_MANY_REQUESTS);
    assert_ne!(sign_in(&a, &ip).await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(sign_in(&b, &ip).await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(sign_in(&a, &ip).await, StatusCode::TOO_MANY_REQUESTS);
    // Another address isn't affected.
    assert_ne!(sign_in(&a, &address()).await, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_window_resets_and_scopes_are_apart() {
    let Some(pools) = pools().await else { return };
    let ip = address().parse().unwrap();
    let limiter = RateLimiter::new(pools.app.clone(), "test_a", 1, Duration::from_secs(1));
    let other = RateLimiter::new(pools.app.clone(), "test_b", 1, Duration::from_secs(1));
    assert!(limiter.check(ip).await);
    assert!(!limiter.check(ip).await);
    assert!(other.check(ip).await, "another scope counts on its own");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(limiter.check(ip).await, "a new window");
}
