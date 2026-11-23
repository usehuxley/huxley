//! Poll triggers against a mock list API on localhost.

use std::sync::{Arc, Mutex};

use axum::Json as AxumJson;
use axum::extract::State;
use axum::routing::get;
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use huxley_triggers::poll::poll_due;
use serde_json::{Value as Json, json};
use uuid::Uuid;

type Devices = Arc<Mutex<Vec<Json>>>;

async fn list(State(devices): State<Devices>) -> AxumJson<Json> {
    AxumJson(json!({ "value": *devices.lock().unwrap() }))
}

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    org: Uuid,
    devices: Devices,
}

async fn harness() -> Option<Harness> {
    let db = isolated("poll").await?;
    let pools = db.pools.clone();
    let org = ensure_root(&pools).await;
    let devices: Devices = Arc::new(Mutex::new(vec![
        json!({ "id": "d1", "name": "Laptop" }),
        json!({ "id": "d2", "name": "Desktop" }),
    ]));
    let app = axum::Router::new()
        .route("/devices", get(list))
        .with_state(devices.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let spec = format!(
        r#"
openapi: 3.0.3
info: {{ title: Devices, version: "1" }}
servers: [{{ url: "http://localhost:{port}" }}]
paths:
  /devices:
    get: {{ operationId: listDevices, responses: {{ "200": {{ description: ok }} }} }}
"#
    );
    let ctx = RequestContext::system(Via::System);
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let integration = store::create(&mut tx, &ctx, org, Some("devices"), &spec)
        .await
        .unwrap();
    let open = ConnectionInput {
        name: "Devices".into(),
        ..ConnectionInput::default()
    };
    connections::create(&mut tx, &crypto, &ctx, org, integration.summary.id, &open)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(pools.app.clone(), crypto, &config);
    Some(Harness {
        _db: db,
        pools,
        engine,
        org,
        devices,
    })
}

impl Harness {
    /// Publishes a workflow polling the devices, and returns its id.
    async fn publish(&self, key: &str, on: &str, items: &str) -> Uuid {
        let document = serde_json::to_value(
            huxley_hwf::parse_yaml(&format!(
                r#"apiVersion: usehuxley.com/v1
kind: Workflow
metadata: {{ key: {key}, name: {key} }}
spec:
  triggers:
    - {{ type: poll, action: devices/listDevices, interval: 1m, items: {items}, on: {on} }}
  steps:
    - {{ id: seen, type: transform, output: "{{{{ trigger.item.name }}}}" }}
"#
            ))
            .unwrap(),
        )
        .unwrap();
        let ctx = RequestContext::system(Via::System);
        let expr = huxley_expr::Engine::new();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let created = huxley_workflows::create(&mut tx, &ctx, &expr, self.org, document)
            .await
            .unwrap();
        huxley_workflows::publish(&mut tx, &ctx, &expr, created.summary.id, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        created.summary.id
    }

    /// Makes every poll due and checks them.
    async fn poll(&self) -> usize {
        sqlx::query("UPDATE workflow_triggers SET next_run_at = now() WHERE type = 'poll'")
            .execute(&self.pools.owner)
            .await
            .unwrap();
        poll_due(&self.engine).await.unwrap()
    }

    async fn runs(&self, workflow: Uuid) -> Vec<Json> {
        sqlx::query_scalar("SELECT trigger FROM runs WHERE workflow_id = $1 ORDER BY id")
            .bind(workflow)
            .fetch_all(&self.pools.owner)
            .await
            .unwrap()
    }

    async fn last_error(&self, workflow: Uuid) -> Option<String> {
        sqlx::query_scalar("SELECT last_error FROM workflow_triggers WHERE workflow_id = $1")
            .bind(workflow)
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn new_items_start_runs_after_a_baseline() {
    let Some(h) = harness().await else { return };
    let new_only = h.publish("new-only", "new", "value").await;
    let changes = h.publish("changes", "new_or_changed", "value").await;

    // The first check only records what's there.
    assert_eq!(h.poll().await, 0);
    assert_eq!(h.runs(new_only).await, Vec::<Json>::new());

    // A new device starts one run in each.
    h.devices
        .lock()
        .unwrap()
        .push(json!({ "id": "d3", "name": "Tablet" }));
    assert_eq!(h.poll().await, 2);
    let runs = h.runs(new_only).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["change"], "new");
    assert_eq!(runs[0]["item"]["name"], "Tablet");

    // Nothing new: nothing starts.
    assert_eq!(h.poll().await, 0);

    // A changed device only matters to the workflow that asked for changes.
    h.devices.lock().unwrap()[0]["name"] = json!("Laptop (renamed)");
    assert_eq!(h.poll().await, 1);
    assert_eq!(h.runs(new_only).await.len(), 1);
    let runs = h.runs(changes).await;
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1]["change"], "changed");
    assert_eq!(runs[1]["item"]["id"], "d1");
}

#[tokio::test]
async fn a_bad_items_path_is_reported() {
    let Some(h) = harness().await else { return };
    let workflow = h.publish("wrong-path", "new", "items").await;
    assert_eq!(h.poll().await, 0);
    let error = h.last_error(workflow).await.unwrap();
    assert!(error.contains("no list at \"items\""), "{error}");

    // Fixed by publishing the right path: the error clears.
    let workflow = h.publish("right-path", "new", "value").await;
    h.poll().await;
    assert_eq!(h.last_error(workflow).await, None);
}
