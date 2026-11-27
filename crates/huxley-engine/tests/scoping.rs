//! The built-in Presales scoping pack in a real run (ADR-072): with a
//! stand-in AI provider, a scope's untouched sections get the AI's text and
//! citations, sections people wrote stay theirs, and gaps become questions.

use std::time::{Duration, Instant};

use axum::Json as AxumJson;
use axum::routing::post;
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_sales::scoping::{self, DraftState, SourceInput};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const DRAFT: &str =
    include_str!("../../huxley-packs/builtin/presales-scoping/workflows/draft-scope.yaml");

/// Answers like Anthropic's Messages API with a drafted scope; checks the
/// discovery reached it.
async fn mock_anthropic() -> u16 {
    let app = axum::Router::new().route(
        "/v1/messages",
        post(|AxumJson(body): AxumJson<Json>| async move {
            let asked = body.to_string();
            assert!(asked.contains("40 mailboxes"), "the transcript reached the model");
            let answer = json!({
                "sections": [
                    { "key": "in_scope", "body": "- Migrate 40 mailboxes to Microsoft 365", "sources": [1] },
                    { "key": "objectives", "body": "Not the people's words", "sources": [] },
                    { "key": "nonsense", "body": "Ignored", "sources": [9] }
                ],
                "questions": [{ "section": "timeline", "question": "When must the migration be finished?" }]
            });
            AxumJson(json!({ "model": body["model"], "stop_reason": "end_turn",
                             "content": [{ "type": "text", "text": answer.to_string() }],
                             "usage": { "input_tokens": 400, "output_tokens": 100 } }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// An MSP, a client with an opportunity, and its scope: discovery added,
/// objectives written by people, a draft asked for.
async fn world(pools: &huxley_db::testing::TestPools) -> (Uuid, Uuid, Uuid) {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "contracts", "sales"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    let provider = huxley_orgs::create(&mut tx, &ctx, root, huxley_orgs::OrgKind::Provider, "MSP")
        .await
        .unwrap()
        .id;
    let client = huxley_orgs::create(
        &mut tx,
        &ctx,
        provider,
        huxley_orgs::OrgKind::Client,
        "Acme",
    )
    .await
    .unwrap()
    .id;
    let opp = huxley_sales::opportunities::create(
        &mut tx,
        &ctx,
        client,
        &serde_json::from_value(json!({ "name": "Mail migration" })).unwrap(),
        None,
    )
    .await
    .unwrap();
    let scope = scoping::open(&mut tx, &ctx, opp.id, "migration")
        .await
        .unwrap();
    scoping::add_source(
        &mut tx,
        &ctx,
        scope.id,
        &SourceInput {
            kind: "transcript".into(),
            title: "Discovery call".into(),
            body: "They have 40 mailboxes on an old Exchange server and want Microsoft 365.".into(),
        },
    )
    .await
    .unwrap();
    // People wrote the objectives; the draft leaves them be.
    scoping::edit_section(
        &mut tx,
        &ctx,
        scope.id,
        "objectives",
        "Retire the Exchange server.",
    )
    .await
    .unwrap();
    scoping::request_draft(&mut tx, &ctx, scope.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    (provider, client, scope.id)
}

async fn engine_with_ai(pools: &huxley_db::testing::TestPools, provider: Uuid) -> Engine {
    let ctx = RequestContext::system(Via::System);
    let port = mock_anthropic().await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_ai::store::set(
        &mut tx,
        &ctx,
        &crypto,
        provider,
        huxley_ai::store::Use::Chat,
        &serde_json::from_value(json!({
            "kind": "anthropic", "baseUrl": format!("http://localhost:{port}"), "apiKey": "sk-ant-test"
        }))
        .unwrap(),
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
    Engine::with_config(pools.app.clone(), crypto, &config)
}

/// Runs the pack's workflow as `scope.draft_requested` would, until it finishes.
async fn run(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    client: Uuid,
    scope: Uuid,
) -> (RunStatus, Option<Json>) {
    let document = serde_json::to_value(huxley_hwf::parse_yaml(DRAFT).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let run = engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: client,
                workflow_id: None,
                version: None,
                document: Some(document),
                trigger_type: "event".into(),
                trigger: json!({ "event_type": "scope.draft_requested", "subject_type": "scope", "subject_id": scope }),
                inputs: json!({ "scopeId": scope }),
                actor_id: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.drain("scoping-test", 50).await.unwrap();
        let row: (RunStatus, Option<Json>) =
            sqlx::query_as("SELECT status, error FROM runs WHERE id = $1")
                .bind(run)
                .fetch_one(&pools.owner)
                .await
                .unwrap();
        if row.0.is_finished() {
            return row;
        }
        assert!(Instant::now() < deadline, "the run didn't finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_pack_drafts_untouched_sections_with_citations_and_questions() {
    let Some(db) = isolated("scoping").await else {
        return;
    };
    let pools = db.pools.clone();
    let (provider, client, scope_id) = world(&pools).await;
    let engine = engine_with_ai(&pools, provider).await;
    let (status, error) = run(&engine, &pools, client, scope_id).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let s = scoping::get(&mut tx, scope_id).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(s.draft_state, DraftState::Idle);
    assert!(s.drafted_at.is_some());
    let section = |key: &str| s.sections.iter().find(|x| x.key == key).unwrap().clone();
    let in_scope = section("in_scope");
    assert_eq!(in_scope.body, "- Migrate 40 mailboxes to Microsoft 365");
    assert!(in_scope.drafted);
    assert_eq!(in_scope.citations[0].label, "Discovery call");
    let objectives = section("objectives");
    assert_eq!(objectives.body, "Retire the Exchange server.");
    assert!(!objectives.drafted);
    assert_eq!(s.questions.len(), 1);
    assert_eq!(s.questions[0].section_key.as_deref(), Some("timeline"));
    assert!(s.questions[0].from_ai);
}
