//! The `ImmyBot` pack against a mock `ImmyBot` and a mock Entra token endpoint
//! on localhost: client-credentials sign-in with the instance's own scope,
//! syncing computers page by page into assets of the matching client, and
//! running maintenance whose outcome is added to a ticket.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

const SPEC: &str = include_str!("../../huxley-packs/builtin/immybot/integrations/immybot.yaml");
const SYNC: &str =
    include_str!("../../huxley-packs/builtin/immybot/workflows/immybot-sync-computers.yaml");
const RUN: &str =
    include_str!("../../huxley-packs/builtin/immybot/workflows/immybot-run-maintenance.yaml");
const DATASET: &str =
    include_str!("../../huxley-packs/builtin/immybot/datasets/immybot-computers.json");

#[derive(Default)]
struct Mock {
    /// The token requests' form fields (and the tenant in the path).
    tokens: Vec<HashMap<String, String>>,
    /// Calls to `ImmyBot`: (what, authorization).
    calls: Vec<(String, String)>,
    started: Vec<Json>,
    polls: usize,
}

type Shared = Arc<Mutex<Mock>>;

fn auth(h: &HeaderMap) -> String {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn token(
    State(m): State<Shared>,
    Path(tenant): Path<String>,
    axum::Form(mut form): axum::Form<HashMap<String, String>>,
) -> axum::Json<Json> {
    form.insert("tenant".into(), tenant);
    m.lock().unwrap().tokens.push(form);
    axum::Json(json!({ "access_token": "immy-token", "token_type": "Bearer", "expires_in": 3600 }))
}

/// Three computers (two Acme's, tenant 7; one Globex's, tenant 8).
async fn computers(
    State(m): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> axum::Json<Json> {
    let mut m = m.lock().unwrap();
    let skip: usize = q.get("skip").map_or(0, |s| s.parse().unwrap());
    let take: usize = q.get("take").map_or(50, |s| s.parse().unwrap());
    m.calls
        .push((format!("computers skip={skip} take={take}"), auth(&h)));
    let all = [
        json!({ "id": 11, "computerName": "ACME-PC", "tenantId": 7, "tenantName": "Acme",
                "isOnline": true, "serialNumber": "SN-11", "manufacturer": "Dell", "hasPendingReboot": false }),
        json!({ "id": 12, "computerName": "ACME-LT", "tenantId": 7, "tenantName": "Acme",
                "isOnline": false, "serialNumber": "SN-12", "hasPendingReboot": true }),
        json!({ "id": 13, "computerName": "GLOBEX-PC", "tenantId": 8, "tenantName": "Globex" }),
    ];
    let page: Vec<&Json> = all.iter().skip(skip).take(take).collect();
    axum::Json(json!({ "count": all.len(), "results": page }))
}

async fn run_service(
    State(m): State<Shared>,
    h: HeaderMap,
    axum::Json(body): axum::Json<Json>,
) -> axum::Json<Json> {
    let mut m = m.lock().unwrap();
    m.calls.push(("run-immy-service".into(), auth(&h)));
    m.started.push(body);
    axum::Json(
        json!({ "sessionsStarted": [{ "computerId": 11, "sessionId": 501 }], "sessionsAlreadyInProgress": [] }),
    )
}

async fn session(State(m): State<Shared>, Path(id): Path<u64>, h: HeaderMap) -> axum::Json<Json> {
    let mut m = m.lock().unwrap();
    m.calls.push((format!("session {id}"), auth(&h)));
    m.polls += 1;
    let status = if m.polls < 3 { "Running" } else { "Passed" };
    axum::Json(json!({ "id": id, "computerId": 11, "sessionStatus": status }))
}

async fn logs(State(m): State<Shared>, Path(id): Path<u64>, h: HeaderMap) -> axum::Json<Json> {
    m.lock()
        .unwrap()
        .calls
        .push((format!("logs {id}"), auth(&h)));
    axum::Json(json!([
        { "id": 1, "message": "Detecting 7-Zip" },
        { "id": 2, "message": null },
        { "id": 3, "message": "7-Zip is compliant" },
    ]))
}

struct Harness {
    pools: TestPools,
    engine: Engine,
    root: Uuid,
    mock: Shared,
}

async fn harness() -> Option<(huxley_db::testing::IsolatedDb, Harness)> {
    let db = isolated("immybot").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk", "cmdb"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let mock: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/{tenant}/oauth2/v2.0/token", post(token))
        .route("/api/v1/computers/paged", get(computers))
        .route("/api/v1/run-immy-service", post(run_service))
        .route("/api/v1/maintenance-sessions/{id}", get(session))
        .route("/api/v1/maintenance-sessions/{id}/logs", get(logs))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    // The mocks stand in for Entra and the instance; the scope keeps the
    // instance's name.
    let spec = SPEC
        .replace(
            "  - url: https://{instance}.immy.bot",
            &format!("  - url: {base}"),
        )
        .replace("https://login.microsoftonline.com", &base);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let integration = store::create(&mut tx, &ctx, root, Some("immybot"), &spec)
        .await
        .unwrap();
    // Committed first: sealing the secret audits on its own connection.
    tx.commit().await.unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mut settings = Map::new();
    settings.insert("client_id".into(), json!("app-123"));
    let mut variables = Map::new();
    variables.insert("instance".into(), json!("acme"));
    variables.insert("tenant".into(), json!("contoso.onmicrosoft.com"));
    connections::create(
        &mut tx,
        &crypto,
        &ctx,
        root,
        integration.summary.id,
        &ConnectionInput {
            name: "ImmyBot".into(),
            key: Some("immybot".into()),
            auth_scheme: Some("entra".into()),
            server_variables: variables,
            settings,
            secrets: [("client_secret".to_owned(), "shh".to_owned())].into(),
            ..ConnectionInput::default()
        },
    )
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
    Some((
        db,
        Harness {
            pools,
            engine,
            root,
            mock,
        },
    ))
}

impl Harness {
    async fn run(&self, workflow: &str, inputs: Json) -> (RunStatus, Option<Json>, Option<Json>) {
        let document = serde_json::to_value(huxley_hwf::parse_yaml(workflow).unwrap()).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let id = self
            .engine
            .start_run(
                &mut tx,
                StartRun {
                    org_id: self.root,
                    workflow_id: None,
                    version: None,
                    document: Some(document),
                    trigger_type: "manual".into(),
                    trigger: json!({}),
                    inputs,
                    actor_id: None,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            self.engine.drain("immybot", 50).await.unwrap();
            let row: (RunStatus, Option<Json>, Option<Json>) =
                sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                    .bind(id)
                    .fetch_one(&self.pools.owner)
                    .await
                    .unwrap();
            if row.0.is_finished() {
                return row;
            }
            assert!(Instant::now() < deadline, "run didn't finish");
            // Waits are timers: let them come due.
            sqlx::query("UPDATE jobs SET run_at = now() WHERE run_id = $1 AND run_at > now()")
                .bind(id)
                .execute(&self.pools.owner)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn signed_in_properly(&self) {
        let m = self.mock.lock().unwrap();
        assert_eq!(m.tokens.len(), 1, "one token, reused: {:?}", m.tokens);
        let t = &m.tokens[0];
        assert_eq!(t["tenant"], "contoso.onmicrosoft.com");
        assert_eq!(t["grant_type"], "client_credentials");
        assert_eq!(t["scope"], "https://acme.immy.bot/.default");
        assert!(
            m.calls.iter().all(|(_, a)| a == "Bearer immy-token"),
            "{:?}",
            m.calls
        );
    }
}

#[tokio::test]
async fn computers_sync_into_assets_of_their_clients() {
    let Some((_db, h)) = harness().await else {
        return;
    };
    let ctx = RequestContext::system(Via::System);
    let mut definition: Json = serde_json::from_str(DATASET).unwrap();
    definition["key"] = json!("immybot-computers");
    let input: huxley_datasets::DatasetInput = serde_json::from_value(definition).unwrap();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let ds = huxley_datasets::create(&mut tx, &ctx, h.root, &input)
        .await
        .unwrap();
    let acme = huxley_orgs::create(&mut tx, &ctx, h.root, huxley_orgs::OrgKind::Client, "Acme")
        .await
        .unwrap()
        .id;
    sqlx::query(
        "INSERT INTO org_variables (id, org_id, key, type, value) \
         VALUES ($1, $2, 'immybot_tenant_id', 'number', '7')",
    )
    .bind(Uuid::now_v7())
    .bind(acme)
    .execute(&mut *tx)
    .await
    .unwrap();
    let ws = huxley_cmdb::types::list(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == "workstation")
        .unwrap()
        .id;
    let mapping: huxley_cmdb::mappings::MappingInput = serde_json::from_value(json!({
        "datasetId": ds.id, "name": "ImmyBot", "source": "immybot", "typeId": ws,
        "matchKeys": [{ "field": "external_id", "column": "id" }],
        "fields": { "name": "name", "serial_number": "serial", "manufacturer": "manufacturer", "external_id": "id" },
        "orgMatch": { "column": "tenant_id", "variable": "immybot_tenant_id" },
    }))
    .unwrap();
    huxley_cmdb::mappings::create(&mut tx, &ctx, &mapping)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (status, outputs, error) = h.run(SYNC, json!({ "pagesize": 2 })).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let outputs = outputs.unwrap();
    assert_eq!(outputs["computers"], 3);
    assert_eq!(outputs["assets"]["created"], 2, "{outputs}");
    assert_eq!(outputs["assets"]["skipped"], 1, "{outputs}");
    let pages: Vec<String> = h
        .mock
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|c| c.0.clone())
        .collect();
    assert_eq!(
        pages,
        ["computers skip=0 take=2", "computers skip=2 take=2"]
    );
    h.signed_in_properly();

    let placed: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT name, org_id FROM assets ORDER BY name")
            .fetch_all(&h.pools.owner)
            .await
            .unwrap();
    assert_eq!(
        placed,
        [("ACME-LT".to_owned(), acme), ("ACME-PC".to_owned(), acme)]
    );
}

#[tokio::test]
async fn maintenance_runs_and_its_outcome_lands_on_the_ticket() {
    let Some((_db, h)) = harness().await else {
        return;
    };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let ticket = huxley_servicedesk::tickets::create(
        &mut tx,
        &ctx,
        &huxley_servicedesk::tickets::NewTicket {
            org_id: h.root,
            title: "Install 7-Zip".into(),
            ..Default::default()
        },
        "T",
        None,
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();

    let (status, outputs, error) = h
        .run(
            RUN,
            json!({ "computer_id": 11, "mode": "detection", "ticket_id": ticket }),
        )
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let outputs = outputs.unwrap();
    assert_eq!(
        (
            &outputs["session_id"],
            &outputs["status"],
            &outputs["passed"]
        ),
        (&json!(501), &json!("Passed"), &json!(true))
    );
    {
        let m = h.mock.lock().unwrap();
        let body = &m.started[0];
        assert_eq!(body["computers"], json!([{ "computerId": 11 }]));
        assert_eq!(
            (
                &body["detectionOnly"],
                &body["inventoryOnly"],
                &body["fullMaintenance"]
            ),
            (&json!(true), &json!(false), &json!(false))
        );
        assert_eq!(body["rebootPreference"], "Suppress");
    }
    h.signed_in_properly();
    let note: String = sqlx::query_scalar(
        "SELECT body FROM sd_ticket_notes WHERE ticket_id = $1 AND kind = 'internal'",
    )
    .bind(ticket)
    .fetch_one(&h.pools.owner)
    .await
    .unwrap();
    assert!(
        note.contains("detection maintenance on computer 11 (session 501): Passed"),
        "{note}"
    );
    assert!(note.contains("7-Zip is compliant"), "{note}");
}
