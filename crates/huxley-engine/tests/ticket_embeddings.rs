//! Search by meaning (Phase 5): `core.ticket.embed` embeds stale tickets
//! with the organization's embeddings provider (a mock OpenAI on
//! localhost), and pgvector finds the nearest in meaning. Needs pgvector in
//! the test database.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::routing::post;
use huxley_ai::ProviderKind;
use huxley_ai::store::{self, ProviderInput, Use};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_servicedesk::embeddings;
use huxley_servicedesk::tickets::{self, NewTicket};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const EMBED_TICKETS: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-search/workflows/embed-tickets.yaml");

/// A toy embedding: how much a text is about printers, networks and email.
fn meaning(text: &str) -> Vec<f32> {
    let t = text.to_lowercase();
    let about = |words: &[&str]| {
        if words.iter().any(|w| t.contains(w)) {
            1.0
        } else {
            0.05
        }
    };
    vec![
        about(&["print", "toner", "paper jam"]),
        about(&["vpn", "network", "wifi"]),
        about(&["mail", "outlook", "inbox"]),
    ]
}

/// The texts each call embedded.
type Calls = Arc<Mutex<Vec<Vec<String>>>>;

async fn openai(
    State(calls): State<Calls>,
    axum::Json(body): axum::Json<Json>,
) -> axum::Json<Json> {
    let texts: Vec<String> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_owned())
        .collect();
    let data: Vec<Json> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| json!({ "index": i, "embedding": meaning(t) }))
        .collect();
    calls.lock().unwrap().push(texts);
    axum::Json(
        json!({ "model": body["model"], "data": data, "usage": { "prompt_tokens": 5, "total_tokens": 5 } }),
    )
}

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    crypto: huxley_crypto::Crypto,
    provider: Uuid,
    client: Uuid,
    url: String,
    calls: Calls,
}

async fn org(pools: &TestPools, parent: Uuid, kind: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organizations (id, parent_id, kind, name, path) VALUES ($1, $2, $3::org_kind, $4, '')",
    )
    .bind(id)
    .bind(parent)
    .bind(kind)
    .bind(unique("embed"))
    .execute(&pools.owner)
    .await
    .unwrap();
    id
}

async fn harness() -> Option<Harness> {
    let db = isolated("ticket_embeddings").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let provider = org(&pools, root, "provider").await;
    let client = org(&pools, provider, "client").await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let calls: Calls = Arc::default();
    let app = axum::Router::new()
        .route("/v1/embeddings", post(openai))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
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
        provider,
        client,
        url,
        calls,
    })
}

impl Harness {
    async fn ticket(&self, title: &str, description: &str) -> Uuid {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let input = NewTicket {
            org_id: self.client,
            title: title.into(),
            description: Some(description.into()),
            ..NewTicket::default()
        };
        let ticket = tickets::create(&mut tx, &ctx, &input, "T", None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        ticket.id
    }

    /// The provider's embeddings provider: OpenAI, pointed at the mock.
    async fn set_provider(&self, model: &str) {
        let ctx = RequestContext::system(Via::System);
        // The org's first data key audits on its own: make it first.
        self.crypto
            .encrypt(
                self.provider,
                huxley_crypto::Binding {
                    purpose: "test",
                    record: self.provider,
                },
                b"x",
            )
            .await
            .unwrap();
        let input = ProviderInput {
            kind: ProviderKind::Openai,
            model: Some(model.into()),
            base_url: Some(self.url.clone()),
            deployment: None,
            api_key: Some("sk-embed".into()),
            monthly_token_budget: None,
        };
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        store::set(
            &mut tx,
            &ctx,
            &self.crypto,
            self.provider,
            Use::Embeddings,
            &input,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    /// Runs the built-in Embed tickets workflow at the provider: its output.
    async fn embed(&self) -> Json {
        let document =
            serde_json::to_value(huxley_hwf::parse_yaml(EMBED_TICKETS).unwrap()).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let id = self
            .engine
            .start_run(
                &mut tx,
                StartRun {
                    org_id: self.provider,
                    workflow_id: None,
                    version: None,
                    document: Some(document),
                    trigger_type: "schedule".into(),
                    trigger: json!({}),
                    inputs: json!({}),
                    actor_id: None,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.engine.drain("embed-test", 50).await.unwrap();
            let (status, outputs, error): (RunStatus, Option<Json>, Option<Json>) =
                sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                    .bind(id)
                    .fetch_one(&self.pools.owner)
                    .await
                    .unwrap();
            if status.is_finished() {
                assert_eq!(status, RunStatus::Succeeded, "{error:?}");
                return outputs.unwrap();
            }
            assert!(Instant::now() < deadline, "run didn't finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn search(&self, scope: &OrgScope, model: &str, q: &str) -> Vec<embeddings::MeaningHit> {
        let mut tx = begin_scoped(&self.pools.app, scope).await.unwrap();
        let hits = embeddings::search(&mut tx, model, &meaning(q), 10)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        hits
    }
}

#[tokio::test]
async fn tickets_are_embedded_and_found_by_meaning() {
    let Some(h) = harness().await else { return };
    let printer = h.ticket("Can't print", "The toner light is on").await;
    let vpn = h
        .ticket("Working from home", "The VPN drops every hour")
        .await;
    let outlook = h.ticket("Outlook", "Inbox won't sync").await;

    // Without an embeddings provider, nothing is embedded (or called).
    assert_eq!(h.embed().await["embedded"], 0);
    assert!(h.calls.lock().unwrap().is_empty());

    // With one, every ticket is: in one call, title then description.
    h.set_provider("text-embedding-3-small").await;
    assert_eq!(h.embed().await["embedded"], 3);
    {
        let calls = h.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains(&"Can't print\n\nThe toner light is on".to_owned()));
    }
    // Nothing is stale now.
    assert_eq!(h.embed().await["embedded"], 0);

    // The nearest in meaning comes first, scored.
    let hits = h
        .search(
            &OrgScope::System,
            "text-embedding-3-small",
            "the printer is out of paper",
        )
        .await;
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].id, printer);
    assert!(hits[0].score > 0.99 && hits[1].score < 0.5, "{hits:?}");
    let hits = h
        .search(&OrgScope::System, "text-embedding-3-small", "wifi is slow")
        .await;
    assert_eq!(hits[0].id, vpn);
    // Only tickets the scope can see; and only those of the model searched.
    let elsewhere = OrgScope::Orgs(vec![h.provider]);
    assert!(
        h.search(&elsewhere, "text-embedding-3-small", "outlook")
            .await
            .is_empty()
    );
    assert!(
        h.search(&OrgScope::System, "another-model", "outlook")
            .await
            .is_empty()
    );

    // A changed ticket is embedded again, alone.
    sqlx::query("UPDATE sd_tickets SET title = 'Outlook and the printer' WHERE id = $1")
        .bind(outlook)
        .execute(&h.pools.owner)
        .await
        .unwrap();
    assert_eq!(h.embed().await["embedded"], 1);
    assert_eq!(h.calls.lock().unwrap().last().unwrap().len(), 1);

    // A new model re-embeds every ticket under its name.
    h.set_provider("text-embedding-3-large").await;
    assert_eq!(h.embed().await["embedded"], 3);
    assert_eq!(
        h.search(&OrgScope::System, "text-embedding-3-large", "toner")
            .await[0]
            .id,
        printer
    );
}

#[tokio::test]
async fn without_pgvector_nothing_is_embedded() {
    let Some(h) = harness().await else { return };
    h.ticket("Can't print", "The toner light is on").await;
    h.set_provider("text-embedding-3-small").await;
    sqlx::query("DROP EXTENSION vector")
        .execute(&h.pools.owner)
        .await
        .unwrap();
    let out = h.embed().await;
    assert_eq!(
        (out["available"].clone(), out["embedded"].clone()),
        (json!(false), json!(0))
    );
    assert!(h.calls.lock().unwrap().is_empty());
}
