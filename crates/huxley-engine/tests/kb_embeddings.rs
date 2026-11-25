//! Knowledge search by meaning: `core.kb.embed` embeds published articles
//! with the organization's embeddings provider (a mock OpenAI on
//! localhost); pgvector finds those nearest a query, and those nearest a
//! ticket by its own embedding. Needs pgvector in the test database.

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
use huxley_knowledge::articles::{self, ArticleChanges, ArticleInput};
use huxley_knowledge::embeddings;
use huxley_servicedesk::tickets::{self, NewTicket};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const EMBED_TICKETS: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-search/workflows/embed-tickets.yaml");
const EMBED_ARTICLES: &str =
    include_str!("../../huxley-packs/builtin/knowledge-search/workflows/embed-articles.yaml");

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
    let db = isolated("kb_embeddings").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let provider = org(&pools, root, "provider").await;
    let client = org(&pools, provider, "client").await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk", "knowledge"] {
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

    /// Runs a built-in embedding workflow at the provider: its output.
    async fn embed(&self, yaml: &str) -> Json {
        let document = serde_json::to_value(huxley_hwf::parse_yaml(yaml).unwrap()).unwrap();
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

    async fn search(&self, model: &str, q: &str) -> Vec<embeddings::MeaningHit> {
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let hits = embeddings::search(&mut tx, model, &meaning(q), 10)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        hits
    }

    /// A published article at the client.
    async fn article(&self, title: &str, body: &str) -> Uuid {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let input = ArticleInput {
            org_id: self.client,
            title: title.into(),
            body: Some(body.into()),
            category_id: None,
            visibility: None,
            tags: None,
        };
        let a = articles::create(&mut tx, &ctx, &input).await.unwrap();
        articles::publish(&mut tx, &ctx, a.summary.id, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        a.summary.id
    }
}

#[tokio::test]
async fn published_articles_are_embedded_and_found_by_meaning() {
    let Some(h) = harness().await else { return };
    let printer = h
        .article(
            "Clearing a paper jam",
            "Open the tray and pull the toner out",
        )
        .await;
    let vpn = h.article("VPN drops", "Reinstall the VPN client").await;
    // Drafts aren't embedded.
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let input = ArticleInput {
        org_id: h.client,
        title: "Outlook draft".into(),
        body: Some("inbox".into()),
        category_id: None,
        visibility: None,
        tags: None,
    };
    articles::create(&mut tx, &ctx, &input).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(h.embed(EMBED_ARTICLES).await["embedded"], 0);
    h.set_provider("text-embedding-3-small").await;
    assert_eq!(h.embed(EMBED_ARTICLES).await["embedded"], 2);
    assert_eq!(h.embed(EMBED_ARTICLES).await["embedded"], 0);
    let hits = h
        .search("text-embedding-3-small", "the printer is out of paper")
        .await;
    assert_eq!(hits[0].id, printer);
    assert!(hits[0].score > 0.99, "{hits:?}");

    // A republished article is embedded again; an unpublished draft change isn't.
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let a = articles::get(&mut tx, vpn).await.unwrap();
    articles::update(
        &mut tx,
        &ctx,
        vpn,
        &ArticleChanges {
            version: a.version,
            body: Some("Now about email and Outlook".into()),
            ..ArticleChanges::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.embed(EMBED_ARTICLES).await["embedded"], 0);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    articles::publish(&mut tx, &ctx, vpn, None).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.embed(EMBED_ARTICLES).await["embedded"], 1);
    assert_eq!(
        h.search("text-embedding-3-small", "outlook inbox").await[0].id,
        vpn
    );

    // Archived articles aren't found.
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    articles::archive(&mut tx, &ctx, printer).await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        h.search("text-embedding-3-small", "toner")
            .await
            .iter()
            .all(|x| x.id != printer)
    );
}

#[tokio::test]
async fn articles_near_a_ticket_come_from_its_own_embedding() {
    let Some(h) = harness().await else { return };
    let printer = h
        .article("Clearing a paper jam", "Pull the toner out")
        .await;
    h.article("VPN drops", "Reinstall the VPN client").await;
    let ticket = h
        .ticket("Printing is broken", "Nothing comes out of the printer")
        .await;
    h.set_provider("text-embedding-3-small").await;
    h.embed(EMBED_ARTICLES).await;
    h.embed(EMBED_TICKETS).await;
    let calls = h.calls.lock().unwrap().len();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let near = embeddings::near_ticket(&mut tx, ticket, 5).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(near[0].id, printer, "{near:?}");
    assert!(near[0].score > 0.99 && near[1].score < 0.3, "{near:?}");
    // No AI call was made for it.
    assert_eq!(h.calls.lock().unwrap().len(), calls);
}

#[tokio::test]
async fn without_pgvector_no_article_is_embedded() {
    let Some(h) = harness().await else { return };
    h.article("Clearing a paper jam", "Pull the toner out")
        .await;
    h.set_provider("text-embedding-3-small").await;
    sqlx::query("DROP EXTENSION vector")
        .execute(&h.pools.owner)
        .await
        .unwrap();
    let out = h.embed(EMBED_ARTICLES).await;
    assert_eq!(
        (out["available"].clone(), out["embedded"].clone()),
        (json!(false), json!(0))
    );
    assert!(h.calls.lock().unwrap().is_empty());
}
