//! The MCP server over HTTP: an MCP client builds, tests and saves a
//! workflow draft with an API token, against a real PostgreSQL.

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

const PASSWORD: &str = "mcp api passphrase";

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
    let state = AppState::new(pools.app.clone(), auth, crypto(&pools), config);
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

impl Setup {
    async fn token(&self, scopes: &[&str]) -> String {
        let (status, created) = self
            .call(
                &self.owner,
                "POST",
                "/api/v1/auth/tokens",
                Some(json!({ "name": "mcp", "scopes": scopes, "expiresInDays": 1 })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created["tokenSecret"].as_str().unwrap().to_owned()
    }

    async fn mcp(
        &self,
        token: Option<&str>,
        message: Value,
    ) -> (StatusCode, http::HeaderMap, Value) {
        let mut req = Request::post("/mcp")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(token) = token {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let res = self
            .app
            .clone()
            .oneshot(req.body(Body::from(message.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Calls a tool; returns its result.
    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let (status, _, reply) = self
            .mcp(
                Some(token),
                json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                                       "params": { "name": name, "arguments": arguments } }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        reply["result"].clone()
    }
}

const WORKFLOW: &str = r#"
apiVersion: usehuxley.com/v1
kind: Workflow
metadata: { key: mcp-greeter, name: MCP greeter }
spec:
  inputs: { type: object, properties: { name: { type: string } } }
  steps:
    - id: lookup
      type: http.request
      with: { method: GET, url: "https://directory.invalid/users" }
    - { id: greet, type: transform, output: "Hi {{ inputs.name }} ({{ steps.lookup.output.body.count }} users)" }
  outputs: { greeting: "{{ steps.greet.output }}" }
"#;

#[tokio::test]
async fn an_mcp_client_discovers_validates_and_tests_a_workflow() {
    let Some(s) = setup().await else { return };
    let token = s.token(&["*"]).await;

    let (status, _, init) = s
        .mcp(Some(&token), json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } }))
        .await;
    assert_eq!(status, StatusCode::OK, "{init}");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let (status, _, _) = s
        .mcp(
            Some(&token),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (_, _, listed) = s
        .mcp(
            Some(&token),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"create_workflow_draft") && names.contains(&"test_workflow_document"),
        "{names:?}"
    );
    let (_, _, schema) = s
        .mcp(Some(&token), json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/read", "params": { "uri": "huxley://schemas/hwf/v1.json" } }))
        .await;
    assert!(
        schema["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("dataset.query")
    );

    // Validate: a broken document's problems, then the good one's none.
    let broken = s
        .tool(
            &token,
            "validate_workflow",
            json!({ "document": WORKFLOW.replace("steps.lookup", "steps.nope") }),
        )
        .await;
    assert!(
        broken["structuredContent"]["diagnostics"]
            .to_string()
            .contains("nope"),
        "{broken}"
    );
    let ok = s
        .tool(&token, "validate_workflow", json!({ "document": WORKFLOW }))
        .await;
    assert_eq!(ok["structuredContent"]["diagnostics"], json!([]), "{ok}");

    // Test it without saving, mocking the step that would reach a real system.
    let started = s
        .tool(
            &token,
            "test_workflow_document",
            json!({
            "org_id": s.org, "document": WORKFLOW, "inputs": { "name": "Ada" },
            "mocks": { "lookup": { "status": 200, "body": { "count": 3 } } } }),
        )
        .await;
    assert_eq!(started["isError"], false, "{started}");
    let run_id = started["structuredContent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let crypto =
        huxley_crypto::Crypto::new(s.pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = huxley_engine::Engine::new(s.pools.app.clone(), crypto);
    let mut run = Value::Null;
    for _ in 0..100 {
        engine.drain("mcp-test", 50).await.unwrap();
        run = s.tool(&token, "get_run", json!({ "id": run_id })).await["structuredContent"].clone();
        if run["status"] == "succeeded" || run["status"] == "failed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(run["outputs"]["greeting"], "Hi Ada (3 users)");
}

#[tokio::test]
async fn an_mcp_client_saves_drafts_audited_as_mcp() {
    let Some(s) = setup().await else { return };
    let token = s.token(&["*"]).await;
    // Save it as a draft; a person publishes it.
    let created = s
        .tool(
            &token,
            "create_workflow_draft",
            json!({ "org_id": s.org, "document": WORKFLOW }),
        )
        .await;
    assert_eq!(created["isError"], false, "{created}");
    let workflow = created["structuredContent"].clone();
    assert_eq!(workflow["currentVersion"], Value::Null);
    let id = workflow["id"].as_str().unwrap();
    let updated = s
        .tool(
            &token,
            "update_workflow_draft",
            json!({ "id": id, "document": WORKFLOW.replace("Hi ", "Hello ") }),
        )
        .await;
    assert_eq!(updated["isError"], false, "{updated}");
    assert!(
        updated["structuredContent"]["draft"]
            .to_string()
            .contains("Hello")
    );

    // Audited as done through MCP.
    let via: String = sqlx::query_scalar(
        "SELECT via FROM audit_events WHERE action = 'workflow.created' AND target_id = $1",
    )
    .bind(Uuid::parse_str(id).unwrap())
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    assert_eq!(via, "mcp");
}

#[tokio::test]
async fn mcp_calls_get_the_users_own_permissions() {
    let Some(s) = setup().await else { return };
    // No token: 401, saying how to authenticate.
    let (status, headers, _) = s
        .mcp(
            None,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.contains_key("www-authenticate"));
    // A token scoped to reading workflows can't create one.
    let token = s.token(&["workflow:read", "org:read"]).await;
    let refused = s
        .tool(
            &token,
            "create_workflow_draft",
            json!({ "org_id": s.org, "document": WORKFLOW }),
        )
        .await;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("403"),
        "{refused}"
    );
    // Unknown tools and methods are JSON-RPC errors.
    let (_, _, unknown) = s
        .mcp(Some(&token), json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "publish_workflow" } }))
        .await;
    assert_eq!(unknown["error"]["code"], -32602);
    let (_, _, nope) = s
        .mcp(
            Some(&token),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "sampling/createMessage" }),
        )
        .await;
    assert_eq!(nope["error"]["code"], -32601);
}
