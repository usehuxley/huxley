//! The `QuickBooks` Online pack (ADR-077) against a mock `QuickBooks` on
//! localhost: its lists are fetched into datasets for mapping; an approved
//! invoice syncs with its mapped customer, item and tax code and is then
//! sent; `QuickBooks` refusing one puts it in the error queue; an unmapped
//! client fails before `QuickBooks` is asked.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use huxley_billing::invoices::{self, Status};
use huxley_billing::sync::{self, MappingInput, MappingKind, Settings, SyncStatus};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

const SPEC: &str =
    include_str!("../../huxley-packs/builtin/quickbooks/integrations/quickbooks.yaml");
const SYNC: &str =
    include_str!("../../huxley-packs/builtin/quickbooks/workflows/quickbooks-sync-invoice.yaml");
const FETCH: &str =
    include_str!("../../huxley-packs/builtin/quickbooks/workflows/quickbooks-fetch-lists.yaml");
const DATASETS: [(&str, &str); 4] = [
    (
        "quickbooks-customers",
        include_str!("../../huxley-packs/builtin/quickbooks/datasets/quickbooks-customers.json"),
    ),
    (
        "quickbooks-items",
        include_str!("../../huxley-packs/builtin/quickbooks/datasets/quickbooks-items.json"),
    ),
    (
        "quickbooks-tax-codes",
        include_str!("../../huxley-packs/builtin/quickbooks/datasets/quickbooks-tax-codes.json"),
    ),
    (
        "quickbooks-classes",
        include_str!("../../huxley-packs/builtin/quickbooks/datasets/quickbooks-classes.json"),
    ),
];

type Bodies = Arc<Mutex<Vec<Json>>>;

/// `QuickBooks`' query endpoint: a few of each entity.
async fn query(Query(q): Query<std::collections::HashMap<String, String>>) -> axum::Json<Json> {
    let text = q.get("query").cloned().unwrap_or_default();
    let found = if text.contains("from Customer") {
        json!({ "Customer": [{ "Id": "58", "DisplayName": "Acme Corp", "Active": true },
                             { "Id": "99", "DisplayName": "Globex", "Active": true }] })
    } else if text.contains("from Item") {
        json!({ "Item": [{ "Id": "1", "Name": "Services", "Active": true }] })
    } else if text.contains("from TaxCode") {
        json!({ "TaxCode": [{ "Id": "TAX", "Name": "TAX", "Active": true }] })
    } else {
        json!({})
    };
    axum::Json(json!({ "QueryResponse": found }))
}

/// Creates the invoice, except for customer 99 (a validation fault).
async fn invoice(
    State(bodies): State<Bodies>,
    axum::Json(body): axum::Json<Json>,
) -> (StatusCode, axum::Json<Json>) {
    bodies.lock().unwrap().push(body.clone());
    if body["CustomerRef"]["value"] == "99" {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "Fault": { "type": "ValidationFault",
                "Error": [{ "Message": "Duplicate Document Number Error", "code": "6140" }] } })),
        );
    }
    (
        StatusCode::OK,
        axum::Json(
            json!({ "Invoice": { "Id": "130", "DocNumber": body["DocNumber"], "TotalAmt": 200 } }),
        ),
    )
}

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    provider: Uuid,
    bodies: Bodies,
}

async fn harness() -> Option<Harness> {
    let db = isolated("quickbooks").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let bodies: Bodies = Arc::default();
    let app = axum::Router::new()
        .route("/v3/company/123/query", get(query))
        .route("/v3/company/123/invoice", post(invoice))
        .with_state(bodies.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // The mock stands in for `QuickBooks`; a bearer token stands in for signing in.
    let start = SPEC.find("  securitySchemes:").unwrap();
    let end = SPEC.find("  parameters:").unwrap();
    let spec = format!(
        "{}  securitySchemes:\n    intuit: {{ type: http, scheme: bearer }}\n{}",
        &SPEC[..start],
        &SPEC[end..]
    )
    .replace(
        "https://{host}/v3/company/{company}",
        &format!("{base}/v3/company/{{company}}"),
    );

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "billing", true, true)
        .await
        .unwrap();
    let provider = huxley_orgs::create(&mut tx, &ctx, root, huxley_orgs::OrgKind::Provider, "MSP")
        .await
        .unwrap()
        .id;
    for (key, definition) in DATASETS {
        let mut d: Json = serde_json::from_str(definition).unwrap();
        d["key"] = json!(key);
        huxley_datasets::create(&mut tx, &ctx, provider, &serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    let integration = store::create(&mut tx, &ctx, root, Some("quickbooks"), &spec)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mut variables = Map::new();
    variables.insert("company".into(), json!("123"));
    connections::create(
        &mut tx,
        &crypto,
        &ctx,
        root,
        integration.summary.id,
        &ConnectionInput {
            name: "QuickBooks".into(),
            key: Some("quickbooks".into()),
            auth_scheme: Some("intuit".into()),
            server_variables: variables,
            secrets: [("token".to_owned(), "qbo-token".to_owned())].into(),
            shared: true,
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
    Some(Harness {
        _db: db,
        engine: Engine::with_config(pools.app.clone(), crypto, &config),
        pools,
        provider,
        bodies,
    })
}

impl Harness {
    async fn tx(&self) -> sqlx::Transaction<'static, sqlx::Postgres> {
        begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap()
    }

    async fn run(&self, org: Uuid, workflow: &str, inputs: Json) -> RunStatus {
        let document = serde_json::to_value(huxley_hwf::parse_yaml(workflow).unwrap()).unwrap();
        let mut tx = self.tx().await;
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
                    inputs,
                    actor_id: None,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.engine.drain("quickbooks", 50).await.unwrap();
            let status: RunStatus = sqlx::query_scalar("SELECT status FROM runs WHERE id = $1")
                .bind(id)
                .fetch_one(&self.pools.owner)
                .await
                .unwrap();
            if status.is_finished() {
                if status != RunStatus::Succeeded {
                    let error: Option<Json> =
                        sqlx::query_scalar("SELECT error FROM runs WHERE id = $1")
                            .bind(id)
                            .fetch_one(&self.pools.owner)
                            .await
                            .unwrap();
                    eprintln!("run {status:?}: {error:?}");
                }
                return status;
            }
            assert!(Instant::now() < deadline, "the run didn't finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A client mapped (or not) to a `QuickBooks` customer, with an approved
    /// invoice of two hours at 100: `(client, invoice)`.
    async fn approved(&self, name: &str, customer: Option<&str>) -> (Uuid, Uuid) {
        let ctx = RequestContext::system(Via::System);
        let mut tx = self.tx().await;
        let client = huxley_orgs::create(
            &mut tx,
            &ctx,
            self.provider,
            huxley_orgs::OrgKind::Client,
            name,
        )
        .await
        .unwrap()
        .id;
        if let Some(c) = customer {
            let m = MappingInput {
                kind: MappingKind::Customer,
                local_key: client.to_string(),
                remote_id: c.into(),
                remote_name: None,
            };
            sync::set_mapping(&mut tx, &ctx, self.provider, "quickbooks", &m)
                .await
                .unwrap();
        }
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO billing_invoices (id, org_id) VALUES ($1, $2)")
            .bind(id)
            .bind(client)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO billing_invoice_lines (id, invoice_id, org_id, position, kind, description, quantity, unit_price) \
             VALUES ($1, $2, $3, 0, 'time', 'Support \"remote\"', 2, 100)",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(client)
        .execute(&mut *tx)
        .await
        .unwrap();
        let i = invoices::get(&mut tx, id).await.unwrap();
        invoices::approve(&mut tx, &ctx, id, i.version, false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (client, id)
    }

    /// The `invoice.sync_requested` payload for an invoice.
    async fn requested(&self, invoice: Uuid) -> Option<Json> {
        sqlx::query_scalar(
            "SELECT payload FROM events_outbox WHERE event_type = 'invoice.sync_requested' AND subject_id = $1 \
              ORDER BY occurred_at DESC LIMIT 1",
        )
        .bind(invoice)
        .fetch_optional(&self.pools.owner)
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn lists_are_fetched_and_approved_invoices_sync() {
    let Some(h) = harness().await else { return };
    let ctx = RequestContext::system(Via::System);

    // Lists come in for mapping.
    assert_eq!(
        h.run(h.provider, FETCH, json!({})).await,
        RunStatus::Succeeded
    );
    let mut tx = h.tx().await;
    let settings = Settings {
        target: "quickbooks".into(),
        enabled: true,
        on_approval: true,
    };
    sync::set_settings(&mut tx, &ctx, h.provider, &settings)
        .await
        .unwrap();
    let customers = sync::remote_options(&mut tx, h.provider, "quickbooks", MappingKind::Customer)
        .await
        .unwrap();
    assert_eq!(customers.len(), 2);
    assert_eq!(customers[0].name, "Acme Corp");
    for (kind, local, remote) in [
        (MappingKind::Item, "kind:time", "1"),
        (MappingKind::TaxCode, "default", "TAX"),
    ] {
        let m = MappingInput {
            kind,
            local_key: local.into(),
            remote_id: remote.into(),
            remote_name: None,
        };
        sync::set_mapping(&mut tx, &ctx, h.provider, "quickbooks", &m)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    // Approved, it's asked for; the workflow creates it in `QuickBooks`.
    let (acme, id) = h.approved("Acme", Some("58")).await;
    let payload = h.requested(id).await.expect("a sync was asked for");
    assert_eq!(payload["customer"]["id"], "58");
    assert_eq!(payload["lines"][0]["amount"], "200");
    assert_eq!(h.run(acme, SYNC, payload).await, RunStatus::Succeeded);
    let body = h.bodies.lock().unwrap()[0].clone();
    assert_eq!(body["CustomerRef"]["value"], "58");
    let line = &body["Line"][0];
    assert_eq!(line["Amount"], 200);
    assert_eq!(line["Description"], "Support \"remote\"");
    assert_eq!(line["SalesItemLineDetail"]["ItemRef"]["value"], "1");
    assert_eq!(line["SalesItemLineDetail"]["TaxCodeRef"]["value"], "TAX");
    assert!(line["SalesItemLineDetail"].get("ClassRef").is_none());
    let mut tx = h.tx().await;
    let s = sync::state(&mut tx, id).await.unwrap().unwrap();
    assert_eq!(s.status, SyncStatus::Synced);
    assert_eq!(s.remote_id.as_deref(), Some("130"));
    assert_eq!(
        s.remote_url.as_deref(),
        Some("https://app.qbo.intuit.com/app/invoice?txnId=130")
    );
    assert_eq!(
        invoices::get(&mut tx, id).await.unwrap().status,
        Status::Sent
    );
    tx.commit().await.unwrap();

    // `QuickBooks` refuses one: it's in the error queue, with `QuickBooks`' reason.
    let (globex, refused) = h.approved("Globex", Some("99")).await;
    h.run(globex, SYNC, h.requested(refused).await.unwrap())
        .await;
    let mut tx = h.tx().await;
    let s = sync::state(&mut tx, refused).await.unwrap().unwrap();
    assert_eq!(s.status, SyncStatus::Failed);
    assert!(
        s.error
            .as_deref()
            .unwrap_or_default()
            .contains("Duplicate Document Number"),
        "{s:?}"
    );
    assert_eq!(
        invoices::get(&mut tx, refused).await.unwrap().status,
        Status::Approved
    );
    let queue = sync::queue(&mut tx, &sync::QueueFilter::default())
        .await
        .unwrap();
    assert!(queue.iter().any(|q| q.invoice_id == refused));
    tx.commit().await.unwrap();

    // An unmapped client fails before `QuickBooks` is asked.
    let (_, unmapped) = h.approved("Initech", None).await;
    assert!(h.requested(unmapped).await.is_none());
    let mut tx = h.tx().await;
    let s = sync::state(&mut tx, unmapped).await.unwrap().unwrap();
    assert_eq!(s.status, SyncStatus::Failed);
    assert!(s.error.unwrap().contains("isn't matched to a customer"));
}
