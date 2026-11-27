//! AI providers against a real PostgreSQL and mock provider APIs on
//! localhost: inheritance down the tree, keys, budgets, the call log.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use huxley_ai::embed::InputType;
use huxley_ai::store::{self, ProviderInput};
use huxley_ai::{Ai, AiError, ProviderKind, Purpose, Request};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use uuid::Uuid;

type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

/// Answers like Anthropic's Messages API: "refuse" in the prompt is
/// declined, anything else echoed (as JSON when a schema is asked for).
async fn anthropic(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    seen.lock().unwrap().push((headers, body.clone()));
    let prompt = body["messages"][0]["content"].as_str().unwrap_or_default();
    if prompt.contains("refuse") {
        return Json(
            json!({ "stop_reason": "refusal", "stop_details": { "explanation": "policy" }, "content": [] }),
        );
    }
    let text = if body.get("output_config").is_some() {
        json!({ "label": "urgent" }).to_string()
    } else {
        format!("echo: {prompt}")
    };
    Json(json!({ "model": body["model"], "stop_reason": "end_turn",
                 "content": [{ "type": "text", "text": text }],
                 "usage": { "input_tokens": 40, "output_tokens": 10 } }))
}

async fn openai(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    seen.lock().unwrap().push((headers, body.clone()));
    Json(
        json!({ "model": body["model"], "choices": [{ "finish_reason": "stop", "message": { "content": "hi from azure" } }],
                 "usage": { "prompt_tokens": 7, "completion_tokens": 3 } }),
    )
}

/// Answers like Voyage's embeddings API: a vector per text, its first
/// number the text's length.
async fn voyage(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    seen.lock().unwrap().push((headers, body.clone()));
    let data: Vec<Value> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, t)| json!({ "index": i, "embedding": [t.as_str().unwrap().len(), 0.5] }))
        .collect();
    Json(json!({ "model": body["model"], "data": data, "usage": { "total_tokens": 12 } }))
}

async fn mock() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let app = axum::Router::new()
        .route("/v1/messages", post(anthropic))
        .route("/openai/deployments/{d}/chat/completions", post(openai))
        .route("/v1/embeddings", post(voyage))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

async fn orgs(pools: &TestPools) -> (Uuid, Uuid) {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let msp = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("AI MSP"),
    )
    .await
    .unwrap()
    .id;
    let client = huxley_orgs::create(
        &mut tx,
        &ctx,
        msp,
        huxley_orgs::OrgKind::Client,
        &unique("AI client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    (msp, client)
}

fn request(prompt: &str) -> Request {
    Request {
        system: Some("Be brief.".into()),
        prompt: prompt.into(),
        schema: None,
        max_tokens: 1000,
        attachment: None,
    }
}

#[tokio::test]
async fn providers_are_inherited_budgeted_and_logged() {
    let Some(pools) = pools().await else { return };
    let (seen, msp, client, crypto, _) = anthropic_setup(&pools, Some(120)).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let view = store::own(&mut tx, msp, store::Use::Chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.model, "claude-opus-5-5");
    assert!(!serde_json::to_string(&view).unwrap().contains("SECRET"));
    drop(tx);

    // The client has none of its own: it uses the MSP's.
    let ai = Ai::new(pools.app.clone(), crypto.clone(), reqwest::Client::new());
    let purpose = Purpose {
        feature: "test",
        run_id: None,
    };
    let done = ai
        .complete(client, purpose, None, &request("hello"))
        .await
        .unwrap();
    assert_eq!(done.text, "echo: hello");
    {
        let seen = seen.lock().unwrap();
        let (headers, body) = &seen[0];
        assert_eq!(headers["x-api-key"], "sk-ant-SECRET-123");
        assert_eq!(headers["anthropic-version"], "2026-10-21");
        assert_eq!(body["system"], "Be brief.");
        assert_eq!(body["model"], "claude-opus-5-5");
    }
    // Structured output, and another model for one call.
    let mut wanted = request("triage this");
    wanted.schema = Some(
        json!({ "type": "object", "properties": { "label": { "type": "string" } }, "required": ["label"], "additionalProperties": false }),
    );
    let classified = ai
        .complete(client, purpose, Some("claude-haiku-5-5"), &wanted)
        .await
        .unwrap();
    assert_eq!(classified.data, Some(json!({ "label": "urgent" })));
    assert_eq!(seen.lock().unwrap()[1].1["model"], "claude-haiku-5-5");

    // Refusals are errors, and logged as refusals.
    let refused = ai
        .complete(client, purpose, None, &request("please refuse"))
        .await
        .unwrap_err();
    assert_eq!(refused, AiError::Refused("policy".into()));

    // 100 tokens used of 120: one more call fits, then the budget is spent.
    ai.complete(client, purpose, None, &request("again"))
        .await
        .unwrap();
    let over = ai
        .complete(client, purpose, None, &request("once more"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            over,
            AiError::OverBudget {
                used: 150,
                budget: 120
            }
        ),
        "{over:?}"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        4,
        "an over-budget call isn't sent"
    );

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let log = store::calls(&mut tx, client, 50).await.unwrap();
    let outcomes: Vec<&str> = log.iter().rev().map(|c| c.outcome.as_str()).collect();
    assert_eq!(outcomes, ["ok", "ok", "refused", "ok", "over_budget"]);
    assert_eq!(log[1].input_tokens, 40);
}

/// An MSP with an Anthropic provider (the mock), and its client.
async fn anthropic_setup(
    pools: &TestPools,
    budget: Option<i64>,
) -> (Seen, Uuid, Uuid, huxley_crypto::Crypto, ProviderInput) {
    let (url, seen) = mock().await;
    let (msp, client) = orgs(pools).await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    // The org's first data key audits on its own: make it first.
    crypto
        .encrypt(
            msp,
            huxley_crypto::Binding {
                purpose: "test",
                record: msp,
            },
            b"x",
        )
        .await
        .unwrap();
    let input = ProviderInput {
        kind: ProviderKind::Anthropic,
        model: None,
        base_url: Some(url),
        deployment: None,
        api_key: Some("sk-ant-SECRET-123".into()),
        monthly_token_budget: budget,
    };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    store::set(&mut tx, &ctx, &crypto, msp, store::Use::Chat, &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (seen, msp, client, crypto, input)
}

#[tokio::test]
async fn changing_settings_without_a_key_keeps_it() {
    let Some(pools) = pools().await else { return };
    let (seen, msp, client, crypto, input) = anthropic_setup(&pools, Some(1)).await;
    let ai = Ai::new(pools.app.clone(), crypto.clone(), reqwest::Client::new());
    let purpose = Purpose {
        feature: "test",
        run_id: None,
    };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    // Changing settings without a key keeps the key; the key is never stored in clear.
    let mut changed = input.clone();
    changed.api_key = None;
    changed.monthly_token_budget = None;
    store::set(&mut tx, &ctx, &crypto, msp, store::Use::Chat, &changed)
        .await
        .unwrap();
    let sealed: Vec<u8> =
        sqlx::query_scalar("SELECT key_sealed FROM ai_providers WHERE org_id = $1")
            .bind(msp)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("SECRET"));
    tx.commit().await.unwrap();
    ai.complete(client, purpose, None, &request("unlimited now"))
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap()[0].0["x-api-key"], "sk-ant-SECRET-123");
}

#[tokio::test]
async fn azure_and_missing_providers() {
    let Some(pools) = pools().await else { return };
    let (url, seen) = mock().await;
    let (msp, client) = orgs(&pools).await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let ai = Ai::new(pools.app.clone(), crypto.clone(), reqwest::Client::new());
    let purpose = Purpose {
        feature: "test",
        run_id: None,
    };
    assert_eq!(
        ai.complete(client, purpose, None, &request("hi"))
            .await
            .unwrap_err(),
        AiError::NotConfigured
    );

    crypto
        .encrypt(
            client,
            huxley_crypto::Binding {
                purpose: "test",
                record: client,
            },
            b"x",
        )
        .await
        .unwrap();
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    // Azure needs its base URL; the key is required the first time.
    let mut input = ProviderInput {
        kind: ProviderKind::AzureOpenai,
        model: Some("gpt-4o".into()),
        base_url: None,
        deployment: Some("prod".into()),
        api_key: Some("azure-key".into()),
        monthly_token_budget: None,
    };
    assert!(
        store::set(&mut tx, &ctx, &crypto, client, store::Use::Chat, &input)
            .await
            .is_err()
    );
    input.base_url = Some(url);
    let mut keyless = input.clone();
    keyless.api_key = None;
    assert!(
        store::set(&mut tx, &ctx, &crypto, client, store::Use::Chat, &keyless)
            .await
            .is_err()
    );
    store::set(&mut tx, &ctx, &crypto, client, store::Use::Chat, &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let done = ai
        .complete(client, purpose, None, &request("hi"))
        .await
        .unwrap();
    assert_eq!(done.text, "hi from azure");
    assert_eq!((done.input_tokens, done.output_tokens), (7, 3));
    assert_eq!(seen.lock().unwrap()[0].0["api-key"], "azure-key");
    // The MSP above has none: the client's own doesn't serve it.
    assert_eq!(
        ai.complete(msp, purpose, None, &request("hi"))
            .await
            .unwrap_err(),
        AiError::NotConfigured
    );
}

#[tokio::test]
async fn embeddings_have_their_own_provider() {
    let Some(pools) = pools().await else { return };
    let (seen, msp, client, crypto, chat) = anthropic_setup(&pools, None).await;
    let ai = Ai::new(pools.app.clone(), crypto.clone(), reqwest::Client::new());
    let purpose = Purpose {
        feature: "embeddings",
        run_id: None,
    };
    let texts = vec!["printer jam".to_owned(), "vpn".to_owned()];
    // The chat provider doesn't make embeddings.
    assert_eq!(
        ai.embed(client, purpose, &texts, InputType::Document)
            .await
            .unwrap_err(),
        AiError::NoEmbeddings
    );
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let voyage = ProviderInput {
        kind: ProviderKind::Voyage,
        model: None,
        api_key: Some("pa-SECRET".into()),
        ..chat.clone()
    };
    // Voyage can't answer; Anthropic can't embed.
    assert!(
        store::set(&mut tx, &ctx, &crypto, msp, store::Use::Chat, &voyage)
            .await
            .is_err()
    );
    assert!(
        store::set(&mut tx, &ctx, &crypto, msp, store::Use::Embeddings, &chat)
            .await
            .is_err()
    );
    let view = store::set(&mut tx, &ctx, &crypto, msp, store::Use::Embeddings, &voyage)
        .await
        .unwrap();
    assert_eq!(view.model, "voyage-3.5");
    // Both are kept: one of each.
    assert_eq!(
        store::own(&mut tx, msp, store::Use::Chat)
            .await
            .unwrap()
            .unwrap()
            .model,
        "claude-opus-5-5"
    );
    tx.commit().await.unwrap();

    let e = ai
        .embed(client, purpose, &texts, InputType::Query)
        .await
        .unwrap();
    assert_eq!(e.vectors, vec![vec![11.0, 0.5], vec![3.0, 0.5]]);
    assert_eq!((e.model.as_str(), e.tokens), ("voyage-3.5", 12));
    let (headers, body) = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(headers["authorization"], "Bearer pa-SECRET");
    assert_eq!(body["input_type"], "query");
    let logged: (String, i64) = sqlx::query_as(
        "SELECT c.feature, c.input_tokens FROM ai_calls c JOIN ai_providers p ON p.id = c.provider_id \
          WHERE c.org_id = $1 AND p.purpose = 'embeddings'",
    )
    .bind(client)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(logged, ("embeddings".to_owned(), 12));
}
