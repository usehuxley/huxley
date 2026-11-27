//! The Xero pack (ADR-077) against a mock Xero on localhost: contacts,
//! items, tax rates and tracking options are fetched for mapping (tracking
//! options as `Category::Option`), and an approved invoice is created in
//! Xero, authorised, naming the organisation in the tenant header; Xero's
//! validation errors reach the error queue.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use huxley_billing::invoices::{self, Status};
use huxley_billing::sync::{self, MappingInput, MappingKind, Settings, SyncStatus};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use serde_json::{Value as Json, json};
use uuid::Uuid;

const SPEC: &str = include_str!("../../huxley-packs/builtin/xero/integrations/xero.yaml");
const SYNC: &str = include_str!("../../huxley-packs/builtin/xero/workflows/xero-sync-invoice.yaml");
const FETCH: &str = include_str!("../../huxley-packs/builtin/xero/workflows/xero-fetch-lists.yaml");
const DATASETS: [(&str, &str); 4] = [
    (
        "xero-customers",
        include_str!("../../huxley-packs/builtin/xero/datasets/xero-customers.json"),
    ),
    (
        "xero-items",
        include_str!("../../huxley-packs/builtin/xero/datasets/xero-items.json"),
    ),
    (
        "xero-tax-codes",
        include_str!("../../huxley-packs/builtin/xero/datasets/xero-tax-codes.json"),
    ),
    (
        "xero-classes",
        include_str!("../../huxley-packs/builtin/xero/datasets/xero-classes.json"),
    ),
];

/// What Xero was sent: (tenant header, body).
type Seen = Arc<Mutex<Vec<(String, Json)>>>;

fn tenant(h: &HeaderMap) -> String {
    h.get("xero-tenant-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn invoices(
    axum::extract::State(seen): axum::extract::State<Seen>,
    h: HeaderMap,
    axum::Json(body): axum::Json<Json>,
) -> (StatusCode, axum::Json<Json>) {
    seen.lock().unwrap().push((tenant(&h), body.clone()));
    let doc = &body["Invoices"][0];
    if doc["Contact"]["ContactID"] == "c-bad" {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "ErrorNumber": 10, "Type": "ValidationException",
                "Message": "A validation exception occurred",
                "Elements": [{ "ValidationErrors": [{ "Message": "Account code '200' is not a valid code for this document." }] }] })),
        );
    }
    (
        StatusCode::OK,
        axum::Json(
            json!({ "Invoices": [{ "InvoiceID": "inv-uuid-1", "InvoiceNumber": doc["InvoiceNumber"] }] }),
        ),
    )
}

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    provider: Uuid,
    seen: Seen,
}

async fn harness() -> Option<Harness> {
    let db = isolated("xero").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let seen: Seen = Arc::default();
    let app = axum::Router::new()
        .route("/api.xro/2.0/Contacts", get(|| async {
            axum::Json(json!({ "Contacts": [{ "ContactID": "c-acme", "Name": "Acme Corp", "IsCustomer": true }] }))
        }))
        .route("/api.xro/2.0/Items", get(|| async {
            axum::Json(json!({ "Items": [{ "ItemID": "x", "Code": "SUPPORT", "Name": "Support", "IsSold": true },
                                          { "ItemID": "y", "Name": "No code" }] }))
        }))
        .route("/api.xro/2.0/TaxRates", get(|| async {
            axum::Json(json!({ "TaxRates": [{ "TaxType": "OUTPUT", "Name": "20% (VAT on Income)" }] }))
        }))
        .route("/api.xro/2.0/TrackingCategories", get(|| async {
            axum::Json(json!({ "TrackingCategories": [{ "Name": "Region", "Options": [
                { "Name": "North", "Status": "ACTIVE" }, { "Name": "South", "Status": "ARCHIVED" }] }] }))
        }))
        .route("/api.xro/2.0/Invoices", axum::routing::put(invoices))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let start = SPEC.find("  securitySchemes:").unwrap();
    let end = SPEC.find("  parameters:").unwrap();
    let spec = format!(
        "{}  securitySchemes:\n    xero: {{ type: http, scheme: bearer }}\n{}",
        &SPEC[..start],
        &SPEC[end..]
    )
    .replace("https://api.xero.com", &base);

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "billing", true, true)
        .await
        .unwrap();
    let provider = huxley_orgs::create(&mut tx, &ctx, root, huxley_orgs::OrgKind::Provider, "MSP")
        .await
        .unwrap()
        .id;
    sqlx::query(
        "INSERT INTO org_variables (id, org_id, key, type, value) VALUES ($1, $2, 'xero_tenant_id', 'string', '\"tenant-42\"')",
    )
    .bind(Uuid::now_v7())
    .bind(root)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (key, definition) in DATASETS {
        let mut d: Json = serde_json::from_str(definition).unwrap();
        d["key"] = json!(key);
        huxley_datasets::create(&mut tx, &ctx, provider, &serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    let integration = store::create(&mut tx, &ctx, root, Some("xero"), &spec)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    connections::create(
        &mut tx,
        &crypto,
        &ctx,
        root,
        integration.summary.id,
        &ConnectionInput {
            name: "Xero".into(),
            key: Some("xero".into()),
            auth_scheme: Some("xero".into()),
            secrets: [("token".to_owned(), "xero-token".to_owned())].into(),
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
        seen,
    })
}

impl Harness {
    async fn tx(&self) -> sqlx::Transaction<'static, sqlx::Postgres> {
        begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap()
    }

    async fn run(&self, org: Uuid, workflow: &str, inputs: Json) -> (RunStatus, Option<Json>) {
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
            self.engine.drain("xero", 50).await.unwrap();
            let row: (RunStatus, Option<Json>) =
                sqlx::query_as("SELECT status, error FROM runs WHERE id = $1")
                    .bind(id)
                    .fetch_one(&self.pools.owner)
                    .await
                    .unwrap();
            if row.0.is_finished() {
                return row;
            }
            assert!(Instant::now() < deadline, "the run didn't finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn map(&self, kind: MappingKind, local: &str, remote: &str) {
        let ctx = RequestContext::system(Via::System);
        let mut tx = self.tx().await;
        let m = MappingInput {
            kind,
            local_key: local.into(),
            remote_id: remote.into(),
            remote_name: None,
        };
        sync::set_mapping(&mut tx, &ctx, self.provider, "xero", &m)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// An approved invoice of 1.5 hours at 120 for a client mapped to
    /// `contact`: `(client, invoice, the sync_requested payload)`.
    async fn approved(&self, name: &str, contact: &str) -> (Uuid, Uuid, Json) {
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
        tx.commit().await.unwrap();
        self.map(MappingKind::Customer, &client.to_string(), contact)
            .await;
        let mut tx = self.tx().await;
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO billing_invoices (id, org_id) VALUES ($1, $2)")
            .bind(id)
            .bind(client)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO billing_invoice_lines (id, invoice_id, org_id, position, kind, description, quantity, unit_price) \
             VALUES ($1, $2, $3, 0, 'time', 'Support', 1.5, 120)",
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
        let payload: Json = sqlx::query_scalar(
            "SELECT payload FROM events_outbox WHERE event_type = 'invoice.sync_requested' AND subject_id = $1",
        )
        .bind(id)
        .fetch_one(&self.pools.owner)
        .await
        .unwrap();
        (client, id, payload)
    }
}

#[tokio::test]
async fn lists_are_fetched_and_approved_invoices_sync_to_xero() {
    let Some(h) = harness().await else { return };
    let ctx = RequestContext::system(Via::System);
    let (status, error) = h.run(h.provider, FETCH, json!({})).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let mut tx = h.tx().await;
    let settings = Settings {
        target: "xero".into(),
        enabled: true,
        on_approval: true,
        credit_hold_days: None,
        credit_hold_amount: None,
    };
    sync::set_settings(&mut tx, &ctx, h.provider, &settings)
        .await
        .unwrap();
    let items = sync::remote_options(&mut tx, h.provider, "xero", MappingKind::Item)
        .await
        .unwrap();
    assert_eq!(items.len(), 1, "only items with a code");
    assert_eq!(items[0].id, "SUPPORT");
    let classes = sync::remote_options(&mut tx, h.provider, "xero", MappingKind::Class)
        .await
        .unwrap();
    assert_eq!(classes[0].id, "Region::North");
    assert_eq!(classes[0].name, "Region: North");
    tx.commit().await.unwrap();
    h.map(MappingKind::Item, "default", "SUPPORT").await;
    h.map(MappingKind::TaxCode, "default", "OUTPUT").await;
    h.map(MappingKind::Class, "default", "Region::North").await;

    let (acme, id, payload) = h.approved("Acme", "c-acme").await;
    let (status, error) = h.run(acme, SYNC, payload).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let (tenant, body) = h.seen.lock().unwrap()[0].clone();
    assert_eq!(tenant, "tenant-42");
    let doc = &body["Invoices"][0];
    assert_eq!(doc["Type"], "ACCREC");
    assert_eq!(doc["Status"], "AUTHORISED");
    assert_eq!(doc["Contact"]["ContactID"], "c-acme");
    let line = &doc["LineItems"][0];
    assert_eq!(line["Quantity"], 1.5);
    assert_eq!(line["UnitAmount"], 120);
    assert_eq!(line["ItemCode"], "SUPPORT");
    assert_eq!(line["TaxType"], "OUTPUT");
    assert_eq!(
        line["Tracking"],
        json!([{ "Name": "Region", "Option": "North" }])
    );
    let mut tx = h.tx().await;
    let s = sync::state(&mut tx, id).await.unwrap().unwrap();
    assert_eq!(s.status, SyncStatus::Synced);
    assert_eq!(
        s.remote_url.as_deref(),
        Some("https://go.xero.com/AccountsReceivable/View.aspx?InvoiceID=inv-uuid-1")
    );
    assert_eq!(
        invoices::get(&mut tx, id).await.unwrap().status,
        Status::Sent
    );
    tx.commit().await.unwrap();

    let (bad, refused, payload) = h.approved("Globex", "c-bad").await;
    h.run(bad, SYNC, payload).await;
    let mut tx = h.tx().await;
    let s = sync::state(&mut tx, refused).await.unwrap().unwrap();
    assert_eq!(s.status, SyncStatus::Failed);
    assert_eq!(
        s.error.as_deref(),
        Some("Xero: Account code '200' is not a valid code for this document.")
    );
}
