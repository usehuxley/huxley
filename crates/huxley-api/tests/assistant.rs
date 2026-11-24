//! The built-in assistant over HTTP: a mock Anthropic API that uses
//! Huxley's tools, approvals, and the conversation history.

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

const PASSWORD: &str = "assistant api passphrase";

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

const WORKFLOW: &str = "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: { key: from-assistant, name: From the assistant }\nspec:\n  steps:\n    - { id: hi, type: transform, output: hi }\n";

/// Acts like a model with tools: "list" → `list_workflows`; "create" →
/// `create_workflow_draft` (for `org`); after a tool result, sums it up.
async fn mock_model(org: Uuid) -> String {
    let app = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
            let messages = body["messages"].as_array().unwrap();
            let last = messages.last().unwrap();
            let reply = |content: Value, stop: &str| {
                axum::Json(json!({ "model": body["model"], "stop_reason": stop, "content": content,
                                   "usage": { "input_tokens": 50, "output_tokens": 10 } }))
            };
            if let Some(blocks) = last["content"].as_array() {
                // A tool result: the assistant's earlier turn (thinking
                // included) must have come back unchanged.
                let earlier = &messages[messages.len() - 2]["content"];
                assert_eq!(earlier[0]["signature"], "sig-1", "thinking blocks go back as they came");
                let result = &blocks[0];
                let text = if result["is_error"] == true {
                    format!("That didn't happen: {}", result["content"].as_str().unwrap_or(""))
                } else {
                    format!("Done. The tool said: {}", &result["content"].as_str().unwrap_or("")[..20.min(result["content"].as_str().unwrap_or("").len())])
                };
                return reply(json!([{ "type": "text", "text": text }]), "end_turn");
            }
            let asked = last["content"].as_str().unwrap_or("");
            let (name, input) = if asked.contains("create") {
                ("create_workflow_draft", json!({ "org_id": org, "document": WORKFLOW }))
            } else {
                ("list_workflows", json!({}))
            };
            reply(
                json!([{ "type": "thinking", "thinking": "", "signature": "sig-1" },
                       { "type": "text", "text": "Let me do that." },
                       { "type": "tool_use", "id": format!("call-{}", messages.len()), "name": name, "input": input }]),
                "tool_use",
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn workflow_exists(s: &Setup) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM workflows WHERE org_id = $1 AND key = 'from-assistant')",
    )
    .bind(s.org)
    .fetch_one(&s.pools.owner)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_assistant_uses_tools_and_asks_before_changing_things() {
    let Some(s) = setup().await else { return };
    with_model(&s).await;

    // A read-only tool runs straight away.
    let (status, chat) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/assistant/conversations",
            Some(json!({ "orgId": s.org, "text": "list my workflows" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{chat}");
    let roles: Vec<&str> = chat["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
    assert_eq!(
        chat["messages"][1]["toolCalls"][0]["name"],
        "list_workflows"
    );
    assert_eq!(chat["messages"][1]["toolCalls"][0]["needsApproval"], false);
    assert!(
        chat["messages"][3]["text"]
            .as_str()
            .unwrap()
            .starts_with("Done."),
        "{chat}"
    );
    assert_eq!(chat["pending"], json!([]));
    let id = chat["id"].as_str().unwrap().to_owned();

    // Creating something waits for the person.
    let uri = format!("/api/v1/assistant/conversations/{id}");
    let (status, waiting) = s
        .call(
            &s.owner,
            "POST",
            &format!("{uri}/messages"),
            Some(json!({ "text": "create a workflow" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{waiting}");
    let call = waiting["pending"][0].clone();
    assert_eq!(call["name"], "create_workflow_draft");
    assert_eq!(call["needsApproval"], true);
    assert!(!workflow_exists(&s).await);
    // Nothing else until it's decided.
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("{uri}/messages"),
            Some(json!({ "text": "hello?" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Denied: nothing is created, and the model is told.
    let decide = |approve: bool, call_id: &str| json!({ "decisions": { call_id: approve } });
    let (status, denied) = s
        .call(
            &s.owner,
            "POST",
            &format!("{uri}/decisions"),
            Some(decide(false, call["id"].as_str().unwrap())),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{denied}");
    assert!(!workflow_exists(&s).await);
    let last = denied["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert!(
        last["text"].as_str().unwrap().contains("declined"),
        "{last}"
    );
}

/// A provider (the mock model) for the setup's organization.
async fn with_model(s: &Setup) {
    let url = mock_model(s.org).await;
    let provider = json!({ "kind": "anthropic", "baseUrl": url, "apiKey": "sk-ant-test" });
    let (status, set) = s
        .call(
            &s.owner,
            "PUT",
            "/api/v1/ai/provider",
            Some(json!({ "orgId": s.org, "provider": provider })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
}

#[tokio::test]
async fn approved_actions_run_as_the_person_and_conversations_stay_theirs() {
    let Some(s) = setup().await else { return };
    with_model(&s).await;
    let (status, chat) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/assistant/conversations",
            Some(json!({ "orgId": s.org, "text": "create a workflow" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{chat}");
    let uri = format!(
        "/api/v1/assistant/conversations/{}",
        chat["id"].as_str().unwrap()
    );
    let decide = |approve: bool, call_id: &str| json!({ "decisions": { call_id: approve } });
    let again = chat;
    // Approved: the draft is created, as the person, through MCP's tools.
    let call = again["pending"][0]["id"].as_str().unwrap().to_owned();
    let (status, approved) = s
        .call(
            &s.owner,
            "POST",
            &format!("{uri}/decisions"),
            Some(decide(true, &call)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert!(workflow_exists(&s).await);
    let (published, via): (Option<i32>, String) = sqlx::query_as(
        "SELECT w.current_version, a.via FROM workflows w JOIN audit_events a ON a.target_id = w.id AND a.action = 'workflow.created' \
          WHERE w.org_id = $1 AND w.key = 'from-assistant'",
    )
    .bind(s.org)
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    assert_eq!(published, None, "only drafts");
    assert_eq!(via, "mcp");

    // Conversations are their owner's; every turn is logged as assistant use.
    let stranger = login(&s.app, &user(&s.pools, false).await).await;
    let (status, _) = s.call(&stranger, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, conversations) = s
        .call(&s.owner, "GET", "/api/v1/assistant/conversations", None)
        .await;
    assert_eq!(conversations[0]["title"], "create a workflow");
    let turns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_calls WHERE org_id = $1 AND feature = 'assistant'",
    )
    .bind(s.org)
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    assert_eq!(turns, 2, "the request, then the answer after approval");
    let (status, _) = s.call(&s.owner, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
