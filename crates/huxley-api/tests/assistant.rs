//! The built-in assistant over HTTP: a mock Anthropic API that uses
//! Huxley's tools, approvals, and the conversation history.

use axum::body::Body;
use axum::response::IntoResponse as _;
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

/// A Messages response as the stream of events Anthropic sends: text in
/// pieces, thinking with a signature delta, tool input as partial JSON.
fn sse(model: &Value, content: &Value, stop: &str) -> axum::response::Response {
    let mut events = vec![json!({ "type": "message_start", "message": {
        "model": model, "role": "assistant", "content": [], "usage": { "input_tokens": 50, "output_tokens": 1 } } })];
    for (index, block) in content.as_array().unwrap().iter().enumerate() {
        let mut start = block.clone();
        let mut deltas = Vec::new();
        match block["type"].as_str().unwrap() {
            "text" => {
                start["text"] = json!("");
                let text = block["text"].as_str().unwrap();
                let (a, b) = text.split_at(text.len() / 2);
                for piece in [a, b] {
                    deltas.push(json!({ "type": "text_delta", "text": piece }));
                }
            }
            "thinking" => {
                start.as_object_mut().unwrap().remove("signature");
                deltas.push(json!({ "type": "signature_delta", "signature": block["signature"] }));
            }
            "tool_use" => {
                start["input"] = json!({});
                let input = block["input"].to_string();
                let (a, b) = input.split_at(input.len() / 2);
                for piece in [a, b] {
                    deltas.push(json!({ "type": "input_json_delta", "partial_json": piece }));
                }
            }
            _ => {}
        }
        events
            .push(json!({ "type": "content_block_start", "index": index, "content_block": start }));
        for delta in deltas {
            events.push(json!({ "type": "content_block_delta", "index": index, "delta": delta }));
        }
        events.push(json!({ "type": "content_block_stop", "index": index }));
    }
    events.push(json!({ "type": "message_delta", "delta": { "stop_reason": stop }, "usage": { "output_tokens": 10 } }));
    events.push(json!({ "type": "message_stop" }));
    let mut body = String::new();
    for e in &events {
        use std::fmt::Write as _;
        let _ = write!(
            body,
            "event: {}\ndata: {e}\n\n",
            e["type"].as_str().unwrap()
        );
    }
    ([(http::header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// Parses a `text/event-stream` body into (event, data) pairs.
fn events(body: &str) -> Vec<(String, Value)> {
    body.split("\n\n")
        .filter_map(|raw| {
            let mut event = None;
            let mut data = None;
            for line in raw.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = Some(v.trim().to_owned());
                } else if let Some(v) = line.strip_prefix("data:") {
                    data = serde_json::from_str(v.trim()).ok();
                }
            }
            Some((event?, data?))
        })
        .collect()
}

#[tokio::test]
async fn answers_stream_as_they_are_written() {
    let Some(s) = setup().await else { return };
    with_model(&s).await;
    let req = Request::post("/api/v1/assistant/conversations")
        .header("x-requested-with", "huxley")
        .header("cookie", &s.owner)
        .header("accept", "text/event-stream")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "orgId": s.org, "text": "list my workflows" }).to_string(),
        ))
        .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let body =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let events = events(&body);
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    // Text as it's written, the tool call, the second turn, then the result.
    assert_eq!(kinds.first(), Some(&"text"), "{kinds:?}");
    assert!(
        kinds.contains(&"tool_started") && kinds.contains(&"tool_finished"),
        "{kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == "turn_ended").count(),
        2,
        "{kinds:?}"
    );
    assert_eq!(kinds.last(), Some(&"done"), "{kinds:?}");
    let first_turn: String = events
        .iter()
        .take_while(|(k, _)| k != "turn_ended")
        .filter(|(k, _)| k == "text")
        .map(|(_, d)| d["text"].as_str().unwrap())
        .collect();
    assert_eq!(first_turn, "Let me do that.");
    let (_, started) = events.iter().find(|(k, _)| k == "tool_started").unwrap();
    assert_eq!(started["name"], "list_workflows");

    // The saved conversation is what a plain request would have given; the
    // model checked that the streamed turn (thinking included) came back.
    let conversation = &events.last().unwrap().1["conversation"];
    let roles: Vec<&str> = conversation["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
    assert!(
        conversation["messages"][3]["text"]
            .as_str()
            .unwrap()
            .starts_with("Done.")
    );
    let id = conversation["id"].as_str().unwrap();
    let (status, saved) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/assistant/conversations/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&saved, conversation);
}

/// Acts like a model with tools: "list" → `list_workflows`; "create" →
/// `create_workflow_draft` (for `org`); after a tool result, sums it up.
async fn mock_model(org: Uuid) -> String {
    let app = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
            let messages = body["messages"].as_array().unwrap();
            let last = messages.last().unwrap();
            let streamed = body["stream"] == true;
            let reply = |content: Value, stop: &str| {
                if streamed {
                    return sse(&body["model"], &content, stop);
                }
                axum::Json(json!({ "model": body["model"], "stop_reason": stop, "content": content,
                                   "usage": { "input_tokens": 50, "output_tokens": 10 } }))
                .into_response()
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

/// What a model was given: its system prompt and tool names.
type Seen = std::sync::Arc<std::sync::Mutex<Option<(String, Vec<String>)>>>;

/// Acts like a model asked from a ticket: notes what it was given, asks to
/// add an internal note to `ticket`, then sums up the result.
async fn ticket_model(ticket: String, seen: Seen) -> String {
    let app = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let (ticket, seen) = (ticket.clone(), seen.clone());
            async move {
                let messages = body["messages"].as_array().unwrap();
                let tools: Vec<String> =
                    body["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect();
                let system = body["system"].as_str().map_or_else(|| body["system"].to_string(), str::to_owned);
                *seen.lock().unwrap() = Some((system, tools));
                let content = if messages.last().unwrap()["content"].is_array() {
                    json!([{ "type": "text", "text": "Noted on the ticket." }])
                } else {
                    json!([{ "type": "tool_use", "id": "call-note", "name": "add_ticket_note",
                             "input": { "id": ticket, "body": "Called the user back.", "kind": "internal" } }])
                };
                let stop = if content[0]["type"] == "tool_use" { "tool_use" } else { "end_turn" };
                axum::Json(json!({ "model": body["model"], "stop_reason": stop, "content": content,
                                   "usage": { "input_tokens": 50, "output_tokens": 10 } }))
                .into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn opened_from_a_ticket_the_assistant_knows_it_and_has_desk_tools() {
    let Some(s) = setup().await else { return };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    cmdb_off(&mut tx, s.org).await;
    tx.commit().await.unwrap();
    let (status, ticket) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/tickets",
            Some(json!({ "orgId": s.org, "title": "Printer jams" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{ticket}");
    let id = ticket["id"].as_str().unwrap().to_owned();
    let seen: Seen = std::sync::Arc::default();
    let url = ticket_model(id.clone(), seen.clone()).await;
    let provider = json!({ "kind": "anthropic", "baseUrl": url, "apiKey": "sk-ant-test" });
    s.call(
        &s.owner,
        "PUT",
        "/api/v1/ai/provider",
        Some(json!({ "orgId": s.org, "provider": provider })),
    )
    .await;

    let (status, convo) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/assistant/conversations",
            Some(json!({ "orgId": s.org, "text": "Note that I called them", "context": { "kind": "ticket", "id": id } })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{convo}");
    let (system, tools) = seen.lock().unwrap().clone().unwrap();
    assert!(
        system.contains(&format!(
            "{} “Printer jams”",
            ticket["reference"].as_str().unwrap()
        )),
        "{system}"
    );
    assert!(system.contains("get_ticket"), "{system}");
    for tool in [
        "find_tickets",
        "get_ticket_notes",
        "add_ticket_note",
        "list_contacts",
    ] {
        assert!(tools.iter().any(|t| t == tool), "{tool}: {tools:?}");
    }
    assert!(
        !tools.iter().any(|t| t == "find_assets"),
        "cmdb is off: {tools:?}"
    );
    // Adding a note changes something: it waits for the person.
    assert_eq!(convo["pending"][0]["name"], "add_ticket_note", "{convo}");
    let decide = format!(
        "/api/v1/assistant/conversations/{}/decisions",
        convo["id"].as_str().unwrap()
    );
    let (status, done) = s
        .call(
            &s.owner,
            "POST",
            &decide,
            Some(json!({ "decisions": { "call-note": true } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (_, notes) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/tickets/{id}/notes"),
            None,
        )
        .await;
    let note = notes
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["body"] == "Called the user back.")
        .unwrap();
    assert_eq!(note["kind"], "internal");
    assert!(
        note["authorUserId"].is_string(),
        "written as the person: {note}"
    );

    unreadable_context_is_refused(&s).await;
}

/// Context the person can't read (or that doesn't exist) is refused.
async fn unreadable_context_is_refused(s: &Setup) {
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/assistant/conversations",
            Some(json!({ "orgId": s.org, "text": "x", "context": { "kind": "ticket", "id": Uuid::now_v7() } })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Other tests may turn the CMDB on instance-wide at any moment: narrow
/// it off at `org` directly (narrowing through the API does nothing while
/// it's off instance-wide, which would leave the race open).
async fn cmdb_off(tx: &mut sqlx::PgConnection, org: Uuid) {
    sqlx::query(
        "INSERT INTO module_settings (org_id, module_key, enabled) VALUES ($1, 'cmdb', false) \
         ON CONFLICT (org_id, module_key) DO UPDATE SET enabled = false",
    )
    .bind(org)
    .execute(&mut *tx)
    .await
    .unwrap();
}
