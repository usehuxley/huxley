//! Microsoft 365 mailboxes (ADR-038): `core.mail.fetch` reads a Graph
//! mailbox's unread mail through the `msgraph` integration (here pointed at
//! a mock Graph on localhost), keeps each message, and marks it read.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, patch, post};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use huxley_storage::Storage;
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

const MSGRAPH: &str = include_str!("../../huxley-packs/builtin/msgraph/integrations/msgraph.yaml");

/// The mock mailbox: messages by id (raw MIME, read), and the bearer
/// tokens it was called with.
#[derive(Default)]
struct Mailbox {
    messages: BTreeMap<String, (String, bool)>,
    tokens: Vec<String>,
}

type Shared = Arc<Mutex<Mailbox>>;

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn token() -> axum::Json<Json> {
    axum::Json(json!({ "access_token": "graph-token", "expires_in": 3600 }))
}

async fn list(
    State(state): State<Shared>,
    Path((user, folder)): Path<(String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> axum::Json<Json> {
    let mut mailbox = state.lock().unwrap();
    mailbox.tokens.push(bearer(&headers));
    assert_eq!(
        (user.as_str(), folder.as_str()),
        ("help@contoso.example", "inbox")
    );
    assert_eq!(
        query.get("$filter").map(String::as_str),
        Some("isRead eq false")
    );
    let unread: Vec<Json> = mailbox
        .messages
        .iter()
        .filter(|(_, (_, read))| !read)
        .map(|(id, _)| json!({ "id": id }))
        .collect();
    axum::Json(json!({ "value": unread }))
}

async fn mime(State(state): State<Shared>, Path((_, id)): Path<(String, String)>) -> String {
    state.lock().unwrap().messages[&id].0.clone()
}

async fn mark(
    State(state): State<Shared>,
    Path((_, id)): Path<(String, String)>,
    axum::Json(body): axum::Json<Json>,
) -> axum::Json<Json> {
    let mut mailbox = state.lock().unwrap();
    if let Some(m) = mailbox.messages.get_mut(&id) {
        m.1 = body["isRead"] == json!(true);
    }
    axum::Json(json!({ "id": id }))
}

fn message(id: &str, subject: &str) -> String {
    format!(
        "From: Ada <ada@acme.example>\r\nTo: help@contoso.example\r\nSubject: {subject}\r\nMessage-ID: <{id}@acme.example>\r\n\r\nHelp, please.\r\n"
    )
}

/// Runs `core.mail.fetch` at `org` until it finishes: its output.
async fn fetch(engine: &Engine, pools: &huxley_db::testing::TestPools, org: Uuid) -> Json {
    let yaml = "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: { key: t, name: T }\nspec:\n  steps:\n    - id: fetch\n      type: core.mail.fetch\n  outputs:\n    result: \"{{ steps.fetch.output }}\"\n";
    let document = serde_json::to_value(huxley_hwf::parse_yaml(yaml).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = engine
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
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.drain("graph-mail", 50).await.unwrap();
        let (status, outputs, error): (RunStatus, Option<Json>, Option<Json>) =
            sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                .bind(id)
                .fetch_one(&pools.owner)
                .await
                .unwrap();
        if status.is_finished() {
            assert_eq!(status, RunStatus::Succeeded, "{error:?}");
            return outputs.unwrap()["result"].clone();
        }
        assert!(Instant::now() < deadline, "run didn't finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Serves the mock Graph on a free port.
async fn mock_graph(state: Shared) -> u16 {
    let app = axum::Router::new()
        .route("/token", post(token))
        .route(
            "/v1.0/users/{user}/mailFolders/{folder}/messages",
            get(list),
        )
        .route("/v1.0/users/{user}/messages/{id}/$value", get(mime))
        .route("/v1.0/users/{user}/messages/{id}", patch(mark))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// The built-in Microsoft Graph integration at `root`, pointed at the mock
/// on `port`, a connection to it, and a Graph mailbox using that. Each in
/// its own transaction: sealing the connection's secret mustn't wait on an
/// audit lock the test holds.
async fn graph_mailbox(
    pools: &huxley_db::testing::TestPools,
    crypto: &Crypto,
    root: Uuid,
    port: u16,
) -> huxley_servicedesk::mail::Mailbox {
    let ctx = RequestContext::system(Via::System);
    let spec = MSGRAPH
        .replace(
            "https://graph.microsoft.com/v1.0",
            &format!("http://localhost:{port}/v1.0"),
        )
        .replace(
            "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token",
            &format!("http://localhost:{port}/token"),
        );
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let integration = store::create(&mut tx, &ctx, root, Some("msgraph"), &spec)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut settings = Map::new();
    settings.insert("client_id".into(), json!("huxley"));
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    connections::create(
        &mut tx,
        crypto,
        &ctx,
        root,
        integration.summary.id,
        &ConnectionInput {
            name: "Contoso".into(),
            key: Some("contoso".into()),
            auth_scheme: Some("app".into()),
            settings,
            secrets: [("client_secret".to_owned(), "s3cret".to_owned())].into(),
            ..ConnectionInput::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let input: huxley_servicedesk::mail::MailboxInput = serde_json::from_value(json!({
        "name": "Help", "address": "help@contoso.example", "kind": "graph",
        "graph": { "connection": "contoso" },
    }))
    .unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mailbox =
        huxley_servicedesk::mail::create(&mut tx, &ctx, root, Uuid::now_v7(), &input, None)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    mailbox
}

#[tokio::test]
async fn graph_mailboxes_fetch_unread_mail_and_mark_it_read() {
    let Some(db) = isolated("graph_mail").await else {
        return;
    };
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
    // A mock Graph with two unread messages.
    let state: Shared = Arc::default();
    {
        let mut m = state.lock().unwrap();
        m.messages
            .insert("AAA".into(), (message("one", "Printer jam"), false));
        m.messages
            .insert("BBB".into(), (message("two", "VPN down"), false));
    }
    let port = mock_graph(state.clone()).await;
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let mailbox = graph_mailbox(&pools, &crypto, root, port).await;
    let defaults = mailbox
        .graph
        .as_ref()
        .map(|g| (g.user.clone(), g.folder.clone()));
    assert_eq!(
        defaults,
        Some((Some("help@contoso.example".into()), Some("inbox".into())))
    );
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine =
        Engine::with_config(pools.app.clone(), crypto, &config).with_storage(Storage::memory());

    // Both are fetched, kept and marked read, with the connection's token.
    let first = fetch(&engine, &pools, root).await;
    assert_eq!(
        (
            first["mailboxes"][0]["fetched"].as_u64(),
            first["mailboxes"][0]["new"].as_u64()
        ),
        (Some(2), Some(2)),
        "{first}"
    );
    {
        let m = state.lock().unwrap();
        assert!(m.messages.values().all(|(_, read)| *read));
        assert!(
            m.tokens.iter().all(|t| t == "Bearer graph-token"),
            "{:?}",
            m.tokens
        );
    }
    let subjects: Vec<String> = sqlx::query_scalar(
        "SELECT subject FROM sd_inbound_emails WHERE mailbox_id = $1 ORDER BY subject",
    )
    .bind(mailbox.id)
    .fetch_all(&pools.owner)
    .await
    .unwrap();
    assert_eq!(subjects, ["Printer jam", "VPN down"]);

    // Nothing unread now; a new one is fetched on its own.
    state
        .lock()
        .unwrap()
        .messages
        .insert("CCC".into(), (message("three", "New laptop"), false));
    let second = fetch(&engine, &pools, root).await;
    assert_eq!(
        (
            second["mailboxes"][0]["fetched"].as_u64(),
            second["mailboxes"][0]["new"].as_u64()
        ),
        (Some(1), Some(1)),
        "{second}"
    );
}
