//! AI provider settings over HTTP, against a real PostgreSQL and a mock
//! Anthropic API on localhost.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "ai api passphrase";

fn crypto(pools: &TestPools) -> Crypto {
    Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring())
}

struct Setup {
    pools: TestPools,
    app: axum::Router,
    org: Uuid,
    owner: String,
}

async fn setup() -> Option<Setup> {
    let pools = pools().await?;
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto(&pools));
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let mut state = AppState::new(pools.app.clone(), auth, crypto(&pools), config);
    state.engine = huxley_engine::Engine::with_config(
        pools.app.clone(),
        crypto(&pools),
        &huxley_engine::EngineConfig {
            egress: huxley_engine::EgressConfig {
                private_hosts: vec!["127.0.0.1".into()],
                ..huxley_engine::EgressConfig::default()
            },
            ..huxley_engine::EngineConfig::default()
        },
    );
    let app = router(state);
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Pk"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    let owner = login(&app, &user(&pools, true).await).await;
    Some(Setup {
        pools,
        app,
        org,
        owner,
    })
}

async fn user(pools: &TestPools, owner: bool) -> String {
    user_with_id(pools, owner).await.1
}

async fn user_with_id(pools: &TestPools, owner: bool) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("pk").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Pack User", PASSWORD)
        .await
        .unwrap();
    if owner {
        huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    (u.id, email)
}

async fn login(app: &axum::Router, email: &str) -> String {
    let req = Request::post("/api/v1/auth/login")
        .header("x-requested-with", "huxley")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "email": email, "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers()["set-cookie"].to_str().unwrap();
    cookie.split(';').next().unwrap().to_owned()
}

impl Setup {
    async fn call(
        &self,
        cookie: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-requested-with", "huxley")
            .header("cookie", cookie);
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

async fn mock_anthropic() -> String {
    let app = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post(
            |headers: http::HeaderMap, axum::Json(body): axum::Json<Value>| async move {
                assert_eq!(headers["x-api-key"], "sk-ant-api-test");
                axum::Json(json!({ "model": body["model"], "stop_reason": "end_turn",
                               "content": [{ "type": "text", "text": "ready" }],
                               "usage": { "input_tokens": 9, "output_tokens": 1 } }))
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn an_organizations_ai_provider_is_set_tested_and_inherited() {
    let Some(s) = setup().await else { return };
    let url = mock_anthropic().await;
    let uri = format!("/api/v1/ai/provider?orgId={}", s.org);
    let (status, none) = s.call(&s.owner, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{none}");
    assert_eq!(none["effective"], Value::Null);

    // A key is needed the first time.
    let provider = json!({ "kind": "anthropic", "baseUrl": url, "monthlyTokenBudget": 100_000 });
    let (status, problem) = s
        .call(
            &s.owner,
            "PUT",
            "/api/v1/ai/provider",
            Some(json!({ "orgId": s.org, "provider": provider })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    let mut with_key = provider.clone();
    with_key["apiKey"] = json!("sk-ant-api-test");
    let (status, set) = s
        .call(
            &s.owner,
            "PUT",
            "/api/v1/ai/provider",
            Some(json!({ "orgId": s.org, "provider": with_key })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set["own"]["model"], "claude-opus-5-5");
    assert!(
        !set.to_string().contains("sk-ant"),
        "the key is never shown"
    );

    let (status, tested) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/ai/provider/test?orgId={}", s.org),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        tested,
        json!({ "ok": true, "message": "ready", "model": "claude-opus-5-5" })
    );
    let (_, calls) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/ai/calls?orgId={}", s.org),
            None,
        )
        .await;
    assert_eq!(calls[0]["feature"], "provider_test");
    assert_eq!(calls[0]["inputTokens"], 9);
    let (_, after) = s.call(&s.owner, "GET", &uri, None).await;
    assert_eq!(after["usedThisMonth"], 10);

    // A client below uses it.
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(
        &mut tx,
        &ctx,
        s.org,
        huxley_orgs::OrgKind::Client,
        &unique("AI client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    let (_, inherited) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/ai/provider?orgId={client}"),
            None,
        )
        .await;
    assert_eq!(inherited["own"], Value::Null);
    assert_eq!(inherited["effective"]["orgId"], json!(s.org));

    // Others can't see or change it.
    let stranger = login(&s.app, &user(&s.pools, false).await).await;
    let (status, _) = s.call(&stranger, "GET", &uri, None).await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{status}"
    );

    let (status, _) = s.call(&s.owner, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, gone) = s.call(&s.owner, "GET", &uri, None).await;
    assert_eq!(gone["effective"], Value::Null);
}
