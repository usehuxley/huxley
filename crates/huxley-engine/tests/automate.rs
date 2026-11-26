//! The `ConnectWise` Automate pack against a mock Automate on localhost: a
//! token-exchange login (integrator user name and password for a bearer
//! token, the `ClientID` on every request), and running a command whose
//! output is added to a ticket.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

const SPEC: &str =
    include_str!("../../huxley-packs/builtin/cwautomate/integrations/cwautomate.yaml");
const RUN: &str =
    include_str!("../../huxley-packs/builtin/cwautomate/workflows/automate-run-command.yaml");
const SYNC: &str =
    include_str!("../../huxley-packs/builtin/cwautomate/workflows/automate-sync-computers.yaml");
const DATASET: &str =
    include_str!("../../huxley-packs/builtin/cwautomate/datasets/automate-computers.json");

#[derive(Default)]
struct Mock {
    logins: Vec<Json>,
    /// (path, authorization, clientID) of every call after logging in.
    calls: Vec<(String, String, String)>,
    commands: Vec<Json>,
    polls: usize,
}

type Shared = Arc<Mutex<Mock>>;

fn header(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn login(
    State(m): State<Shared>,
    h: HeaderMap,
    axum::Json(body): axum::Json<Json>,
) -> (StatusCode, axum::Json<Json>) {
    let mut m = m.lock().unwrap();
    m.logins
        .push(json!({ "body": body, "clientID": header(&h, "clientid") }));
    if body["password"] != "s3cret" {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "Message": "bad credentials" })),
        );
    }
    let expires = (time::OffsetDateTime::now_utc() + time::Duration::hours(1))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    (
        StatusCode::OK,
        axum::Json(
            json!({ "AccessToken": "tok-1", "ExpirationDate": expires.trim_end_matches('Z') }),
        ),
    )
}

async fn run_command(
    State(m): State<Shared>,
    Path(id): Path<u64>,
    h: HeaderMap,
    axum::Json(body): axum::Json<Json>,
) -> axum::Json<Json> {
    let mut m = m.lock().unwrap();
    m.calls.push((
        format!("POST /Computers/{id}/CommandExecute"),
        header(&h, "authorization"),
        header(&h, "clientid"),
    ));
    m.commands.push(body);
    axum::Json(json!({ "Id": 77, "Status": "Pending" }))
}

/// Three computers, a page at a time.
async fn computers(
    State(m): State<Shared>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    h: HeaderMap,
) -> axum::Json<Json> {
    let mut m = m.lock().unwrap();
    m.calls.push((
        format!(
            "GET /computers?page={}&pagesize={}",
            q["page"], q["pagesize"]
        ),
        header(&h, "authorization"),
        header(&h, "clientid"),
    ));
    let all = [
        json!({ "Id": 1, "ComputerName": "ACME-PC", "Client": { "Id": 7, "Name": "Acme" },
                "Location": { "Id": 1, "Name": "HQ" }, "Type": "Workstation", "Status": "Online",
                "SerialNumber": "SN-1", "IsVirtualMachine": false }),
        json!({ "Id": 2, "ComputerName": "ACME-SRV", "Client": { "Id": 7, "Name": "Acme" },
                "Type": "Server", "IsVirtualMachine": true }),
        json!({ "Id": 3, "ComputerName": "GLOBEX-PC", "Client": { "Id": 8, "Name": "Globex" } }),
    ];
    let page: usize = q["page"].parse().unwrap();
    let size: usize = q["pagesize"].parse().unwrap();
    axum::Json(json!(
        all.iter()
            .skip((page - 1) * size)
            .take(size)
            .collect::<Vec<_>>()
    ))
}

async fn command_results(
    State(m): State<Shared>,
    Path(id): Path<u64>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    h: HeaderMap,
) -> axum::Json<Json> {
    let mut m = m.lock().unwrap();
    m.calls.push((
        format!("GET /Computers/{id}/CommandExecute?ids={}", q["ids"]),
        header(&h, "authorization"),
        header(&h, "clientid"),
    ));
    m.polls += 1;
    // Executing at first, then done.
    axum::Json(if m.polls < 2 {
        json!([{ "Id": 77, "Status": "Executing" }])
    } else {
        json!([{ "Id": 77, "Status": "Success", "Output": "C:\\Windows\\Temp> hostname\nPC-ADA" }])
    })
}

struct Harness {
    pools: TestPools,
    engine: Engine,
    root: Uuid,
    mock: Shared,
}

async fn harness(password: &str) -> Option<(huxley_db::testing::IsolatedDb, Harness)> {
    let db = isolated("automate").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let mock: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/cwa/api/v1/apitoken", post(login))
        .route("/cwa/api/v1/computers", axum::routing::get(computers))
        .route(
            "/cwa/api/v1/Computers/{id}/CommandExecute",
            post(run_command).get(command_results),
        )
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    // The mock speaks plain HTTP.
    let spec = SPEC.replace("https://{server}", "http://{server}");
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let integration = store::create(&mut tx, &ctx, root, Some("cwautomate"), &spec)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut settings = Map::new();
    settings.insert("username".into(), json!("huxley-integrator"));
    settings.insert("client_id".into(), json!("client-123"));
    let mut variables = Map::new();
    variables.insert("server".into(), json!(format!("localhost:{port}")));
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    connections::create(
        &mut tx,
        &crypto,
        &ctx,
        root,
        integration.summary.id,
        &ConnectionInput {
            name: "Automate".into(),
            key: Some("automate".into()),
            auth_scheme: Some("integrator".into()),
            server_variables: variables,
            settings,
            secrets: [("password".to_owned(), password.to_owned())].into(),
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
    /// The pack's Automate computers dataset, at the MSP.
    async fn dataset(&self) -> huxley_datasets::Dataset {
        let ctx = RequestContext::system(Via::System);
        let mut definition: Json = serde_json::from_str(DATASET).unwrap();
        definition["key"] = json!("automate-computers");
        let input: huxley_datasets::DatasetInput = serde_json::from_value(definition).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let ds = huxley_datasets::create(&mut tx, &ctx, self.root, &input)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        ds
    }

    async fn ticket(&self) -> Uuid {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let t = huxley_servicedesk::tickets::create(
            &mut tx,
            &ctx,
            &huxley_servicedesk::tickets::NewTicket {
                org_id: self.root,
                title: "Slow PC".into(),
                ..Default::default()
            },
            "T",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        t.id
    }

    async fn run(&self, inputs: Json) -> (RunStatus, Option<Json>, Option<Json>) {
        self.run_workflow(RUN, inputs).await
    }

    async fn run_workflow(
        &self,
        workflow: &str,
        inputs: Json,
    ) -> (RunStatus, Option<Json>, Option<Json>) {
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
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            self.engine.drain("automate", 50).await.unwrap();
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
}

#[tokio::test]
async fn commands_run_through_an_exchanged_token_and_land_on_the_ticket() {
    let Some((_db, h)) = harness("s3cret").await else {
        return;
    };
    let ticket = h.ticket().await;
    let (status, outputs, error) = h
        .run(json!({ "computer_id": 42, "command": "hostname", "ticket_id": ticket }))
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let outputs = outputs.unwrap();
    assert_eq!(outputs["status"], "Success");
    assert!(outputs["output"].as_str().unwrap().contains("PC-ADA"));

    {
        let m = h.mock.lock().unwrap();
        // One login, with the integrator's credentials and the ClientID.
        assert_eq!(m.logins.len(), 1, "{:?}", m.logins);
        assert_eq!(
            m.logins[0]["body"],
            json!({ "username": "huxley-integrator", "password": "s3cret" })
        );
        assert_eq!(m.logins[0]["clientID"], "client-123");
        // Every call carries the token and the ClientID.
        assert!(m.calls.len() >= 3, "{:?}", m.calls);
        assert!(
            m.calls
                .iter()
                .all(|(_, auth, client)| auth == "Bearer tok-1" && client == "client-123"),
            "{:?}",
            m.calls
        );
        assert_eq!(m.calls[1].0, "GET /Computers/42/CommandExecute?ids=77");
        // A PowerShell command line, as command 2.
        assert_eq!(m.commands[0]["Command"]["Id"], 2);
        assert_eq!(m.commands[0]["ComputerId"], 42);
        assert_eq!(
            m.commands[0]["Parameters"][0],
            "powershell.exe!!! -NonInteractive -Command hostname"
        );
    }

    let note: String = sqlx::query_scalar(
        "SELECT body FROM sd_ticket_notes WHERE ticket_id = $1 AND kind = 'internal'",
    )
    .bind(ticket)
    .fetch_one(&h.pools.owner)
    .await
    .unwrap();
    assert!(
        note.contains("Ran on Automate computer 42: Success") && note.contains("PC-ADA"),
        "{note}"
    );
}

#[tokio::test]
async fn a_refused_login_fails_the_run_saying_why() {
    let Some((_db, h)) = harness("wrong").await else {
        return;
    };
    let (status, _, error) = h
        .run(json!({ "computer_id": 42, "command": "hostname" }))
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert!(error.unwrap().to_string().contains("bad credentials"));
}

#[tokio::test]
async fn computers_sync_into_the_dataset_page_by_page() {
    let Some((_db, h)) = harness("s3cret").await else {
        return;
    };
    let ctx = RequestContext::system(Via::System);
    let mut definition: Json = serde_json::from_str(DATASET).unwrap();
    definition["key"] = json!("automate-computers");
    let input: huxley_datasets::DatasetInput = serde_json::from_value(definition).unwrap();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let ds = huxley_datasets::create(&mut tx, &ctx, h.root, &input)
        .await
        .unwrap();
    // A computer since removed from Automate.
    huxley_datasets::upsert(
        &mut tx,
        &ds,
        h.root,
        &[json!({ "id": 99, "name": "GONE-PC" })],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let (status, outputs, error) = h.run_workflow(SYNC, json!({ "pagesize": 2 })).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(outputs.unwrap()["computers"], 3);
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
        [
            "GET /computers?page=1&pagesize=2",
            "GET /computers?page=2&pagesize=2"
        ]
    );

    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let rows = huxley_datasets::query(
        &mut tx,
        &ds,
        &huxley_datasets::Query {
            order_by: Some("id".into()),
            ..huxley_datasets::Query::default()
        },
    )
    .await
    .unwrap();
    let summary: Vec<Json> = rows
        .rows
        .iter()
        .map(|r| json!([r["id"], r["name"], r["client_id"], r["type"], r["virtual"]]))
        .collect();
    assert_eq!(
        summary,
        [
            json!([1, "ACME-PC", 7, "Workstation", false]),
            json!([2, "ACME-SRV", 7, "Server", true]),
            json!([3, "GLOBEX-PC", 8, null, null]),
        ]
    );
}

#[tokio::test]
async fn synced_computers_become_assets_of_their_clients() {
    let Some((_db, h)) = harness("s3cret").await else {
        return;
    };
    let ds = h.dataset().await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "cmdb", true, false)
        .await
        .unwrap();
    let acme = huxley_orgs::create(&mut tx, &ctx, h.root, huxley_orgs::OrgKind::Client, "Acme")
        .await
        .unwrap()
        .id;
    sqlx::query(
        "INSERT INTO org_variables (id, org_id, key, type, value) \
         VALUES ($1, $2, 'automate_client_id', 'number', '7')",
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
        "datasetId": ds.id, "name": "Automate", "source": "automate", "typeId": ws,
        "matchKeys": [{ "field": "external_id", "column": "id" }],
        "fields": { "name": "name", "serial_number": "serial", "external_id": "id" },
        "orgMatch": { "column": "client_id", "variable": "automate_client_id" },
    }))
    .unwrap();
    huxley_cmdb::mappings::create(&mut tx, &ctx, &mapping)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (status, outputs, error) = h.run_workflow(SYNC, json!({})).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let assets = &outputs.unwrap()["assets"];
    // Acme's two; Globex (client 8) has no organization here.
    assert_eq!(
        (assets["mappings"].as_u64(), assets["created"].as_u64()),
        (Some(1), Some(2)),
        "{assets}"
    );
    assert_eq!(assets["skipped"], 1, "{assets}");
    let placed: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT name, org_id FROM assets ORDER BY name")
            .fetch_all(&h.pools.owner)
            .await
            .unwrap();
    assert_eq!(
        placed,
        [("ACME-PC".to_owned(), acme), ("ACME-SRV".to_owned(), acme)]
    );
}
