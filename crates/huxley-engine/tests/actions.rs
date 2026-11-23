//! `integration.action` steps against a mock API on localhost.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json as AxumJson;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

/// Requests the mock API saw: method, path, query, auth and body.
#[derive(Default)]
struct Api {
    seen: Vec<Json>,
}

type Shared = Arc<Mutex<Api>>;

fn auth(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .or_else(|| headers.get("x-key"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn list(
    State(api): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> AxumJson<Json> {
    api.lock().unwrap().seen.push(json!({
        "op": "list", "auth": auth(&headers), "query": query,
    }));
    AxumJson(json!([{ "id": 1, "subject": "Printer" }]))
}

async fn create(
    State(api): State<Shared>,
    headers: HeaderMap,
    AxumJson(body): AxumJson<Json>,
) -> (StatusCode, AxumJson<Json>) {
    api.lock().unwrap().seen.push(json!({
        "op": "create", "auth": auth(&headers), "body": body,
    }));
    (
        StatusCode::CREATED,
        AxumJson(json!({ "id": 42, "subject": body["subject"] })),
    )
}

async fn one(
    State(api): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AxumJson<Json> {
    api.lock()
        .unwrap()
        .seen
        .push(json!({ "op": "one", "auth": auth(&headers), "id": id }));
    AxumJson(json!({ "id": id }))
}

async fn token() -> AxumJson<Json> {
    AxumJson(json!({ "access_token": "oauth-token", "expires_in": 3600 }))
}

fn spec(port: u16, list_id: &str) -> String {
    format!(
        r#"
openapi: 3.0.3
info: {{ title: Tickets, version: "1" }}
servers: [{{ url: "http://localhost:{port}/api" }}]
components:
  securitySchemes:
    key: {{ type: apiKey, in: header, name: X-Key }}
    app:
      type: oauth2
      flows:
        clientCredentials: {{ tokenUrl: "http://localhost:{port}/token", scopes: {{}} }}
paths:
  /tickets:
    get:
      operationId: {list_id}
      parameters: [ {{ name: status, in: query, schema: {{ type: string }} }} ]
      responses: {{ "200": {{ description: ok, content: {{ application/json: {{ schema: {{ type: array }} }} }} }} }}
    post:
      operationId: createTicket
      requestBody:
        required: true
        content:
          application/json:
            schema: {{ type: object, required: [subject], properties: {{ subject: {{ type: string }} }} }}
      responses: {{ "201": {{ description: made }} }}
  /tickets/{{id}}:
    get:
      operationId: getTicket
      parameters: [ {{ name: id, in: path, required: true, schema: {{ type: string }} }} ]
      responses: {{ "200": {{ description: ok }} }}
"#
    )
}

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    crypto: Crypto,
    msp: Uuid,
    client: Uuid,
    integration: Uuid,
    port: u16,
    api: Shared,
}

async fn org(pools: &TestPools, parent: Uuid, kind: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organizations (id, parent_id, kind, name, path) VALUES ($1, $2, $3::org_kind, $4, '')",
    )
    .bind(id)
    .bind(parent)
    .bind(kind)
    .bind(unique("org"))
    .execute(&pools.owner)
    .await
    .unwrap();
    id
}

async fn harness() -> Option<Harness> {
    let db = isolated("actions").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let msp = org(&pools, root, "provider").await;
    let client = org(&pools, msp, "client").await;

    let api: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/api/tickets", get(list).post(create))
        .route("/api/tickets/{id}", get(one))
        .route("/token", post(token))
        .with_state(api.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let integration = store::create(
        &mut tx,
        &ctx,
        msp,
        Some("tickets"),
        &spec(port, "listTickets"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(pools.app.clone(), crypto.clone(), &config);
    Some(Harness {
        _db: db,
        pools,
        engine,
        crypto,
        msp,
        client,
        integration: integration.summary.id,
        port,
        api,
    })
}

#[derive(Debug, sqlx::FromRow)]
struct RunRow {
    status: RunStatus,
    outputs: Option<Json>,
    // Shown when an assertion about the run fails.
    #[allow(dead_code)]
    error: Option<Json>,
}

impl Harness {
    async fn connect(&self, org: Uuid, key: &str, scheme: &str, secret: &str, shared: bool) {
        let (settings, secrets) = if scheme == "app" {
            (
                json!({ "client_id": "huxley" }),
                [("client_secret".to_owned(), secret.to_owned())],
            )
        } else {
            (json!({}), [("key".to_owned(), secret.to_owned())])
        };
        let input = ConnectionInput {
            name: key.into(),
            key: Some(key.into()),
            auth_scheme: Some(scheme.into()),
            settings: match settings {
                Json::Object(map) => map,
                _ => Map::new(),
            },
            secrets: secrets.into(),
            shared,
            ..ConnectionInput::default()
        };
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        connections::create(&mut tx, &self.crypto, &ctx, org, self.integration, &input)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn start(&self, org: Uuid, steps: &str) -> Uuid {
        let yaml = format!(
            "apiVersion: usehuxley.com/v1\nkind: Workflow\nmetadata: {{ key: t, name: T }}\nspec:\n  steps:\n{steps}"
        );
        let document = serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let id = self
            .engine
            .start_run(
                &mut tx,
                StartRun {
                    org_id: org,
                    workflow_id: None,
                    version: None,
                    document: Some(document),
                    trigger_type: "manual".into(),
                    trigger: json!({}),
                    inputs: json!({}),
                    actor_id: None,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn finish(&self, id: Uuid) -> RunRow {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.engine.drain("test", 50).await.unwrap();
            let run: RunRow =
                sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                    .bind(id)
                    .fetch_one(&self.pools.owner)
                    .await
                    .unwrap();
            if run.status.is_finished() {
                return run;
            }
            assert!(Instant::now() < deadline, "run didn't finish: {run:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn step_error(&self, run: Uuid, key: &str) -> Json {
        sqlx::query_scalar(
            "SELECT error FROM step_runs WHERE run_id = $1 AND step_key = $2 ORDER BY attempt DESC LIMIT 1",
        )
        .bind(run)
        .bind(key)
        .fetch_one(&self.pools.owner)
        .await
        .unwrap()
    }

    fn seen(&self) -> Vec<Json> {
        std::mem::take(&mut self.api.lock().unwrap().seen)
    }
}

#[tokio::test]
async fn actions_build_authenticated_requests() {
    let Some(h) = harness().await else { return };
    h.connect(h.msp, "main", "key", "k-123", false).await;
    let id = h
        .start(
            h.msp,
            r#"    - { id: open, type: integration.action, action: tickets/listTickets, with: { status: open } }
    - id: made
      type: integration.action
      action: tickets/createTicket
      with: { body: { subject: "{{ steps.open.output[0].subject }} jam" } }
    - { id: one, type: integration.action, action: tickets/getTicket, with: { id: "a b/c" } }
  outputs:
    made: "{{ steps.made.output.id }}"
    one: "{{ steps.one.output.id }}"
"#,
        )
        .await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    assert_eq!(run.outputs.unwrap(), json!({ "made": 42, "one": "a b/c" }));
    let seen = h.seen();
    assert_eq!(seen[0]["auth"], "k-123");
    assert_eq!(seen[0]["query"], json!({ "status": "open" }));
    assert_eq!(seen[1]["body"], json!({ "subject": "Printer jam" }));
    assert_eq!(seen[2]["id"], "a b/c", "path parameters are encoded");
}

#[tokio::test]
async fn oauth_connections_send_their_token() {
    let Some(h) = harness().await else { return };
    h.connect(h.msp, "app", "app", "s3cret", false).await;
    let id = h
        .start(
            h.msp,
            "    - { id: open, type: integration.action, action: tickets/listTickets }\n",
        )
        .await;
    assert_eq!(h.finish(id).await.status, RunStatus::Succeeded);
    assert_eq!(h.seen()[0]["auth"], "Bearer oauth-token");
}

#[tokio::test]
async fn connections_come_from_the_org_tree() {
    let Some(h) = harness().await else { return };
    let steps = "    - { id: open, type: integration.action, action: tickets/listTickets }\n";

    // The MSP's own connection isn't shared: the client can't use it.
    h.connect(h.msp, "private", "key", "msp-private", false)
        .await;
    let run = h.start(h.client, steps).await;
    assert_eq!(h.finish(run).await.status, RunStatus::Failed);
    assert_eq!(h.step_error(run, "open").await["kind"], "connection");

    // Shared: the client uses it.
    h.connect(h.msp, "shared", "key", "msp-shared", true).await;
    let run = h.start(h.client, steps).await;
    assert_eq!(h.finish(run).await.status, RunStatus::Succeeded);
    assert_eq!(h.seen()[0]["auth"], "msp-shared");

    // The client's own connection wins over the shared one.
    h.connect(h.client, "own", "key", "client-own", false).await;
    let run = h.start(h.client, steps).await;
    assert_eq!(h.finish(run).await.status, RunStatus::Succeeded);
    assert_eq!(h.seen()[0]["auth"], "client-own");

    // Naming one picks it.
    let named = "    - { id: open, type: integration.action, action: tickets/listTickets, connection: shared }\n";
    let run = h.start(h.client, named).await;
    assert_eq!(h.finish(run).await.status, RunStatus::Succeeded);
    assert_eq!(h.seen()[0]["auth"], "msp-shared");
}

/// Version 2 of the spec, without the list operation at all.
fn without_list(port: u16) -> String {
    let full = spec(port, "listTickets");
    let start = full
        .find("    get:\n      operationId: listTickets")
        .unwrap();
    let end = full.find("    post:").unwrap();
    format!("{}{}", &full[..start], &full[end..])
}

async fn update(h: &Harness, spec: &str) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    store::update(&mut tx, &ctx, h.integration, spec, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn runs_keep_the_version_they_started_with() {
    let Some(h) = harness().await else { return };
    h.connect(h.msp, "main", "key", "k", false).await;
    let steps = r"    - { id: pause, type: wait, duration: 300ms }
    - { id: open, type: integration.action, action: tickets/listTickets }
";
    // Started on version 1, then the vendor drops the list operation.
    let pinned = h.start(h.msp, steps).await;
    h.engine.drain("test", 10).await.unwrap();
    update(&h, &without_list(h.port)).await;

    // The running run still calls version 1's listTickets.
    let run = h.finish(pinned).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    let (pins,): (Json,) = sqlx::query_as("SELECT integration_pins FROM runs WHERE id = $1")
        .bind(pinned)
        .fetch_one(&h.pools.owner)
        .await
        .unwrap();
    assert_eq!(pins["tickets"]["version"], 1);

    // A run started now is on version 2, which has no such action.
    let later = h.start(h.msp, steps).await;
    assert_eq!(h.finish(later).await.status, RunStatus::Failed);
    assert_eq!(h.step_error(later, "open").await["kind"], "definition");
}

#[tokio::test]
async fn renamed_actions_keep_working() {
    let Some(h) = harness().await else { return };
    h.connect(h.msp, "main", "key", "k", false).await;
    // The vendor renames listTickets to searchTickets (same method and path).
    update(&h, &spec(h.port, "searchTickets")).await;
    for key in ["listTickets", "searchTickets"] {
        let run = h
            .start(
                h.msp,
                &format!("    - {{ id: open, type: integration.action, action: tickets/{key} }}\n"),
            )
            .await;
        let finished = h.finish(run).await;
        assert_eq!(finished.status, RunStatus::Succeeded, "{key}: {finished:?}");
    }
    // And again: an alias follows the next rename.
    update(&h, &spec(h.port, "findTickets")).await;
    let run = h
        .start(
            h.msp,
            "    - { id: open, type: integration.action, action: tickets/listTickets }\n",
        )
        .await;
    assert_eq!(h.finish(run).await.status, RunStatus::Succeeded);
}

#[tokio::test]
async fn inputs_and_missing_actions_fail_clearly() {
    let Some(h) = harness().await else { return };
    h.connect(h.msp, "main", "key", "k", false).await;
    let run = h
        .start(
            h.msp,
            r"    - { id: bad, type: integration.action, action: tickets/createTicket, with: { body: { subject: 7 } }, continue_on_error: true }
    - { id: gone, type: integration.action, action: tickets/nothingHere, continue_on_error: true }
    - { id: other, type: integration.action, action: nope/listTickets, continue_on_error: true }
",
        )
        .await;
    assert_eq!(h.finish(run).await.status, RunStatus::Succeeded);
    let bad = h.step_error(run, "bad").await;
    assert_eq!(bad["kind"], "inputs", "{bad}");
    assert!(
        bad["message"].as_str().unwrap().contains("/body/subject"),
        "{bad}"
    );
    assert_eq!(h.step_error(run, "gone").await["kind"], "definition");
    assert_eq!(h.step_error(run, "other").await["kind"], "definition");
    assert!(h.seen().is_empty(), "nothing was sent");
}
