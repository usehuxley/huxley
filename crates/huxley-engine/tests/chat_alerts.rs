//! The Service desk chat alerts pack: urgent tickets and missed SLAs posted
//! to Slack (through the Slack integration) and Microsoft Teams (an
//! incoming webhook), against a mock of both on localhost.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use huxley_integrations::connections::{self, ConnectionInput};
use huxley_integrations::store;
use huxley_variables::{VariableInput, VariableType};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const SLACK: &str = include_str!("../../huxley-packs/builtin/slack/integrations/slack.yaml");
const SLACK_ALERTS: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-chat/workflows/slack-ticket-alerts.yaml");
const TEAMS_ALERTS: &str =
    include_str!("../../huxley-packs/builtin/servicedesk-chat/workflows/teams-ticket-alerts.yaml");

/// What the mock saw: (where, authorization, body); and whether Slack
/// refuses.
#[derive(Default)]
struct Seen {
    calls: Vec<(String, String, Json)>,
    slack_refuses: bool,
}

type Shared = Arc<Mutex<Seen>>;

async fn slack(
    State(seen): State<Shared>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Json>,
) -> axum::Json<Json> {
    let mut s = seen.lock().unwrap();
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    s.calls.push(("slack".into(), auth, body));
    axum::Json(if s.slack_refuses {
        json!({ "ok": false, "error": "channel_not_found" })
    } else {
        json!({ "ok": true })
    })
}

async fn teams(State(seen): State<Shared>, axum::Json(body): axum::Json<Json>) -> &'static str {
    seen.lock()
        .unwrap()
        .calls
        .push(("teams".into(), String::new(), body));
    "1"
}

struct Harness {
    pools: TestPools,
    engine: Engine,
    crypto: Crypto,
    org: Uuid,
    port: u16,
    seen: Shared,
}

async fn harness() -> Option<(huxley_db::testing::IsolatedDb, Harness)> {
    let db = isolated("chat_alerts").await?;
    let pools = db.pools.clone();
    let org = ensure_root(&pools).await;
    let seen: Shared = Arc::default();
    let app = axum::Router::new()
        .route("/api/chat.postMessage", post(slack))
        .route("/teams-hook", post(teams))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(pools.app.clone(), crypto.clone(), &config);
    Some((
        db,
        Harness {
            pools,
            engine,
            crypto,
            org,
            port,
            seen,
        },
    ))
}

impl Harness {
    /// Runs a workflow for an event: its status and error.
    async fn run(&self, yaml: &str, event: &str, priority: &str) -> (RunStatus, Option<Json>) {
        let document = serde_json::to_value(huxley_hwf::parse_yaml(yaml).unwrap()).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let id = self
            .engine
            .start_run(
                &mut tx,
                StartRun {
                    org_id: self.org,
                    workflow_id: None,
                    version: None,
                    document: Some(document),
                    trigger_type: "event".into(),
                    trigger: json!({ "type": event, "subject_type": "ticket", "subject_id": Uuid::now_v7() }),
                    inputs: json!({ "reference": "T-1042", "title": "Server room too hot", "priority": priority }),
                    actor_id: None,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.engine.drain("chat-alerts", 50).await.unwrap();
            let (status, error): (RunStatus, Option<Json>) =
                sqlx::query_as("SELECT status, error FROM runs WHERE id = $1")
                    .bind(id)
                    .fetch_one(&self.pools.owner)
                    .await
                    .unwrap();
            if status.is_finished() {
                return (status, error);
            }
            assert!(Instant::now() < deadline, "run didn't finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn set(&self, key: &str, kind: VariableType, value: &str) {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let input = VariableInput {
            kind,
            value: json!(value),
            description: None,
            locked: false,
        };
        huxley_variables::set(&mut tx, &ctx, &self.crypto, self.org, key, &input)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// The Slack integration pointed at the mock, with a bot token.
    async fn connect_slack(&self) {
        let ctx = RequestContext::system(Via::System);
        let spec = SLACK.replace(
            "https://slack.com/api",
            &format!("http://localhost:{}/api", self.port),
        );
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let integration = store::create(&mut tx, &ctx, self.org, Some("slack"), &spec)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        connections::create(
            &mut tx,
            &self.crypto,
            &ctx,
            self.org,
            integration.summary.id,
            &ConnectionInput {
                name: "Workspace".into(),
                key: Some("workspace".into()),
                auth_scheme: Some("bot".into()),
                secrets: [("token".to_owned(), "xoxb-test".to_owned())].into(),
                ..ConnectionInput::default()
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    fn calls(&self) -> Vec<(String, String, Json)> {
        std::mem::take(&mut self.seen.lock().unwrap().calls)
    }
}

#[tokio::test]
async fn nothing_is_posted_until_a_channel_is_set() {
    let Some((_db, h)) = harness().await else {
        return;
    };
    for yaml in [SLACK_ALERTS, TEAMS_ALERTS] {
        let (status, error) = h.run(yaml, "ticket.created", "p1").await;
        assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    }
    assert_eq!(h.calls(), Vec::<(String, String, Json)>::new());
}

#[tokio::test]
async fn urgent_tickets_and_missed_slas_are_posted_to_slack_and_teams() {
    let Some((_db, h)) = harness().await else {
        return;
    };
    h.connect_slack().await;
    h.set(
        "servicedesk_slack_channel",
        VariableType::String,
        "#service-desk",
    )
    .await;
    h.set(
        "servicedesk_teams_webhook",
        VariableType::Secret,
        &format!("http://localhost:{}/teams-hook", h.port),
    )
    .await;

    for yaml in [SLACK_ALERTS, TEAMS_ALERTS] {
        let (status, error) = h.run(yaml, "ticket.created", "p1").await;
        assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    }
    let calls = h.calls();
    let (_, auth, slack) = calls.iter().find(|(to, _, _)| to == "slack").unwrap();
    assert_eq!(auth, "Bearer xoxb-test");
    assert_eq!(slack["channel"], "#service-desk");
    assert!(
        slack["text"]
            .as_str()
            .unwrap()
            .contains("New P1: T-1042 Server room too hot"),
        "{slack}"
    );
    let (_, _, card) = calls.iter().find(|(to, _, _)| to == "teams").unwrap();
    let body = &card["attachments"][0]["content"]["body"];
    assert_eq!(
        card["attachments"][0]["contentType"],
        "application/vnd.microsoft.card.adaptive"
    );
    assert_eq!(
        (body[0]["text"].as_str(), body[1]["text"].as_str()),
        (Some("New P1: T-1042"), Some("Server room too hot"))
    );

    // A missed SLA says so.
    let (status, _) = h.run(TEAMS_ALERTS, "ticket.sla_breached", "p3").await;
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(
        h.calls()[0].2["attachments"][0]["content"]["body"][0]["text"],
        "Missed SLA: T-1042"
    );

    // Slack refusing (it answers 200) fails the run, saying why.
    h.seen.lock().unwrap().slack_refuses = true;
    let (status, error) = h.run(SLACK_ALERTS, "ticket.created", "p2").await;
    assert_eq!(status, RunStatus::Failed);
    assert!(error.unwrap().to_string().contains("channel_not_found"));
}

#[test]
fn only_urgent_tickets_start_them() {
    for yaml in [SLACK_ALERTS, TEAMS_ALERTS] {
        let doc = huxley_hwf::parse_yaml(yaml).unwrap();
        let filters: Vec<Option<String>> = doc
            .spec
            .triggers
            .iter()
            .map(|t| match t {
                huxley_hwf::Trigger::Event(e) => e.filter.clone(),
                _ => panic!("event triggers"),
            })
            .collect();
        assert_eq!(
            filters[0].as_deref(),
            Some("{{ event.payload.priority in ['p1', 'p2'] }}")
        );
        assert_eq!(filters[1], None);
    }
}
