//! The Receipt reading pack (ADR-080): a receipt added to an expense is
//! sent to the AI provider as an image; what it read is kept beside the
//! entry, and taking it changes the draft.

use std::time::{Duration, Instant};

use axum::Json as AxumJson;
use axum::routing::post;
use huxley_core::{Actor, RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_expenses::entries::{self, ExpenseInput};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const WORKFLOW: &str =
    include_str!("../../huxley-packs/builtin/receipt-reading/workflows/read-receipt.yaml");

/// Answers like Anthropic's Messages API, checking the receipt came as an image.
async fn mock_anthropic() -> u16 {
    let app = axum::Router::new().route(
        "/v1/messages",
        post(|AxumJson(body): AxumJson<Json>| async move {
            let content = &body["messages"][0]["content"];
            assert_eq!(content[0]["type"], "image", "{body}");
            assert_eq!(content[0]["source"]["media_type"], "image/png");
            assert!(body["output_config"]["format"]["schema"]["properties"]["vendor"].is_object());
            let answer = json!({ "amount": "64.20", "tax": "4.20", "currency": "usd", "spentOn": null,
                                 "vendor": "Home Depot", "description": "Cable and connectors" });
            AxumJson(json!({ "model": body["model"], "stop_reason": "end_turn",
                             "content": [{ "type": "text", "text": answer.to_string() }],
                             "usage": { "input_tokens": 900, "output_tokens": 40 } }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// A technician's expense at an MSP with an AI provider, its receipt kept:
/// `(msp, user, expense, the expense.receipt_added payload)`.
async fn world(
    pools: &huxley_db::testing::TestPools,
    crypto: &huxley_crypto::Crypto,
    storage: &huxley_storage::Storage,
    port: u16,
) -> (Uuid, Uuid, Uuid, Json) {
    let root = ensure_root(pools).await;
    let system = RequestContext::system(Via::System);
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Tech')")
        .bind(user)
        .bind(format!("{}@example.com", user.simple()))
        .execute(&pools.owner)
        .await
        .unwrap();
    let ctx = RequestContext {
        actor: Actor::User(user),
        ..system.clone()
    };
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &system, "expenses", true, true)
        .await
        .unwrap();
    let msp = huxley_orgs::create(
        &mut tx,
        &system,
        root,
        huxley_orgs::OrgKind::Provider,
        "MSP",
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_ai::store::set(
        &mut tx,
        &system,
        crypto,
        msp,
        huxley_ai::store::Use::Chat,
        &serde_json::from_value(json!({
            "kind": "anthropic", "baseUrl": format!("http://localhost:{port}"), "apiKey": "sk-ant-test"
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let parts: Uuid =
        sqlx::query_scalar("SELECT id FROM exp_categories WHERE name = 'Parts and supplies'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let input: ExpenseInput = serde_json::from_value(json!({
        "categoryId": parts, "spentOn": time::OffsetDateTime::now_utc().date().to_string(),
        "description": "Cabling", "amount": "1", "orgId": msp
    }))
    .unwrap();
    let e = entries::create(&mut tx, &ctx, user, &input).await.unwrap();
    entries::put_receipt(
        &mut tx,
        storage,
        &ctx,
        e.id,
        "receipt.png",
        "image/png",
        "png".into(),
    )
    .await
    .unwrap();
    let payload: Json = sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE event_type = 'expense.receipt_added' AND subject_id = $1",
    )
    .bind(e.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    (msp, user, e.id, payload)
}

/// Runs the pack's workflow on `payload` until it succeeds.
async fn run_pack(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    msp: Uuid,
    payload: Json,
) {
    let document = serde_json::to_value(huxley_hwf::parse_yaml(WORKFLOW).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let run = engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: msp,
                workflow_id: None,
                version: None,
                document: Some(document),
                trigger_type: "event".into(),
                trigger: json!({}),
                inputs: payload,
                actor_id: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.drain("receipts", 20).await.unwrap();
        let (status, error): (RunStatus, Option<Json>) =
            sqlx::query_as("SELECT status, error FROM runs WHERE id = $1")
                .bind(run)
                .fetch_one(&pools.owner)
                .await
                .unwrap();
        if status.is_finished() {
            assert_eq!(status, RunStatus::Succeeded, "{error:?}");
            break;
        }
        assert!(Instant::now() < deadline, "the run didn't finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_receipt_is_read_and_its_reading_taken() {
    let Some(db) = isolated("receipts").await else {
        return;
    };
    let pools = db.pools.clone();
    let system = RequestContext::system(Via::System);
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let storage = huxley_storage::Storage::memory();
    let port = mock_anthropic().await;

    let (msp, user, expense, payload) = world(&pools, &crypto, &storage, port).await;
    let ctx = RequestContext {
        actor: Actor::User(user),
        ..system.clone()
    };
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine =
        Engine::with_config(pools.app.clone(), crypto, &config).with_storage(storage.clone());
    run_pack(&engine, &pools, msp, payload).await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let read = entries::get(&mut tx, expense).await.unwrap();
    let reading = read.reading.clone().unwrap();
    assert_eq!(reading.vendor.as_deref(), Some("Home Depot"));
    assert_eq!(
        read.amount.to_string(),
        "1.0000",
        "nothing changed by itself"
    );
    let taken = entries::apply_reading(&mut tx, &ctx, expense)
        .await
        .unwrap();
    assert_eq!(taken.amount.normalize().to_string(), "64.2");
    assert_eq!(taken.tax.normalize().to_string(), "4.2");
    assert_eq!(taken.currency, "USD");
    assert_eq!(taken.vendor.as_deref(), Some("Home Depot"));
    assert_eq!(
        taken.spent_on, read.spent_on,
        "an unread date keeps the entry's"
    );
}
