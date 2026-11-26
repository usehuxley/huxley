//! The built-in AI triage workflow in a real run (ADR-047, ADR-048): with
//! a stand-in AI provider, a client's untouched ticket gets the queue,
//! priority and needed skills the model picked from the real options.

use std::time::{Duration, Instant};

use axum::Json as AxumJson;
use axum::routing::post;
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const TRIAGE: &str =
    include_str!("../../huxley-packs/builtin/ai-triage/workflows/triage-ticket.yaml");
const DOCUMENT: &str = include_str!(
    "../../huxley-packs/builtin/ai-auto-documentation/workflows/document-resolution.yaml"
);
const STATUS: &str = include_str!(
    "../../huxley-packs/builtin/project-status-reports/workflows/weekly-status-reports.yaml"
);
const SUMMARIZE: &str =
    include_str!("../../huxley-packs/builtin/ai-ticket-insights/workflows/summarize-ticket.yaml");

/// Answers like Anthropic's Messages API: for triage, the Network queue, p2,
/// and the Networking skill, picked from the schema's enums; for insights,
/// a summary with entities.
async fn mock_anthropic() -> u16 {
    let app = axum::Router::new().route(
        "/v1/messages",
        post(|AxumJson(body): AxumJson<Json>| async move {
            let props = &body["output_config"]["format"]["schema"]["properties"];
            let pick = |list: &Json, prefix: &str| {
                list.as_array()
                    .and_then(|l| {
                        l.iter()
                            .find(|v| v.as_str().is_some_and(|s| s.starts_with(prefix)))
                    })
                    .cloned()
                    .unwrap_or(Json::Null)
            };
            let answer = if props["resolution_note"].is_object() {
                json!({
                    "resolution_note": "The VPN client's certificate had expired; renewed it.",
                    "article_worthwhile": true,
                    "article_title": "VPN fails after the client certificate expires",
                    "article_body": "## Symptoms\nThe VPN won't connect.\n\n## Fix\nRenew the certificate.",
                    "minutes": 45,
                })
            } else if props["highlights"].is_object() {
                json!({
                    "summary": "The firewall was swapped on schedule.",
                    "highlights": ["Firewall swapped"], "risks": [], "next_steps": ["Cut over the VPN"],
                })
            } else if props["summary"].is_object() {
                json!({
                    "summary": "One person's VPN won't connect since this morning.",
                    "people": ["Ada", "ada"], "devices": ["LAPTOP-42"], "applications": ["GlobalProtect"],
                    "errors": ["Gateway unreachable"], "next_step": "Check the VPN gateway's certificate.",
                })
            } else {
                json!({
                    "queue": pick(&props["queue"]["enum"], "Network"),
                    "priority": "p2",
                    "skills": [pick(&props["skills"]["items"]["enum"], "Networking")],
                    "reason": "The VPN is down for one person",
                })
            };
            AxumJson(json!({ "model": body["model"], "stop_reason": "end_turn",
                             "content": [{ "type": "text", "text": answer.to_string() }],
                             "usage": { "input_tokens": 40, "output_tokens": 10 } }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

async fn org(tx: &mut sqlx::PgConnection, parent: Uuid, kind: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, parent_id, kind, name, path) VALUES ($1, $2, $3::org_kind, $4, '')")
        .bind(id)
        .bind(parent)
        .bind(kind)
        .bind(unique("triage"))
        .execute(&mut *tx)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn a_new_ticket_gets_its_queue_priority_and_skills() {
    let Some(db) = isolated("triage").await else {
        return;
    };
    let pools = db.pools.clone();
    let (provider, queues, skill, ticket) = world(&pools).await;
    let engine = engine_with_ai(&pools, provider).await;
    let (status, error) = run_triage(&engine, &pools, provider, ticket).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let (queue, priority): (Option<Uuid>, String) =
        sqlx::query_as("SELECT queue_id, priority FROM sd_tickets WHERE id = $1")
            .bind(ticket)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert_eq!((queue, priority.as_str()), (Some(queues[1]), "p2"));
    let needs: Vec<(Uuid, i16)> = sqlx::query_as(
        "SELECT skill_id, min_level FROM wf_requirements WHERE target_type = 'ticket' AND target_id = $1",
    )
    .bind(ticket)
    .fetch_all(&pools.owner)
    .await
    .unwrap();
    assert_eq!(needs, [(skill, 2)]);
    let applied: bool = sqlx::query_scalar("SELECT applied FROM sd_triage WHERE ticket_id = $1")
        .bind(ticket)
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert!(applied);
}

/// A provider with a client, Hardware and Network queues, a Networking
/// skill, and a client's untouched ticket.
async fn world(pools: &huxley_db::testing::TestPools) -> (Uuid, Vec<Uuid>, Uuid, Uuid) {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let provider = org(&mut tx, root, "provider").await;
    let client = org(&mut tx, provider, "client").await;
    for module in ["crm", "servicedesk", "workforce"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    let mut queues = Vec::new();
    for name in ["Hardware", "Network"] {
        let input: huxley_servicedesk::queues::QueueInput = serde_json::from_value(
            json!({ "name": unique(name), "description": format!("{name} problems") }),
        )
        .unwrap();
        queues.push(
            huxley_servicedesk::queues::create(&mut tx, &ctx, provider, &input)
                .await
                .unwrap()
                .id,
        );
    }
    let skill = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO wf_skills (id, name, description) VALUES ($1, $2, 'VPNs and routers')",
    )
    .bind(skill)
    .bind(unique("Networking"))
    .execute(&mut *tx)
    .await
    .unwrap();
    let ticket = huxley_servicedesk::tickets::create(
        &mut tx,
        &ctx,
        &huxley_servicedesk::tickets::NewTicket {
            org_id: client,
            title: "VPN won't connect".into(),
            ..Default::default()
        },
        "T",
        None,
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();

    (provider, queues, skill, ticket)
}

/// An engine that may reach the stand-in provider, set up at `provider`.
async fn engine_with_ai(pools: &huxley_db::testing::TestPools, provider: Uuid) -> Engine {
    let ctx = RequestContext::system(Via::System);
    let port = mock_anthropic().await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let input: huxley_ai::store::ProviderInput = serde_json::from_value(json!({
        "kind": "anthropic", "baseUrl": format!("http://localhost:{port}"), "apiKey": "sk-ant-test"
    }))
    .unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_ai::store::set(
        &mut tx,
        &ctx,
        &crypto,
        provider,
        huxley_ai::store::Use::Chat,
        &input,
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

/// Runs the pack's workflow as `ticket.created` would, until it finishes.
async fn run_triage(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    provider: Uuid,
    ticket: Uuid,
) -> (RunStatus, Option<Json>) {
    run_pack(engine, pools, provider, ticket, TRIAGE, "ticket.created").await
}

#[tokio::test]
async fn a_ticket_gets_a_summary_with_its_entities() {
    let Some(db) = isolated("insights").await else {
        return;
    };
    let pools = db.pools.clone();
    let (provider, _, _, ticket) = world(&pools).await;
    let engine = engine_with_ai(&pools, provider).await;
    let (status, error) = run_pack(
        &engine,
        &pools,
        provider,
        ticket,
        SUMMARIZE,
        "ticket.insight_requested",
    )
    .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let (summary, entities, next): (String, Json, Option<String>) = sqlx::query_as(
        "SELECT summary, entities, next_step FROM sd_ticket_insights WHERE ticket_id = $1",
    )
    .bind(ticket)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(
        summary,
        "One person's VPN won't connect since this morning."
    );
    assert_eq!(
        entities,
        json!({ "people": ["Ada"], "devices": ["LAPTOP-42"], "applications": ["GlobalProtect"], "errors": ["Gateway unreachable"] })
    );
    assert_eq!(
        next.as_deref(),
        Some("Check the VPN gateway's certificate.")
    );
}

#[tokio::test]
async fn a_resolved_ticket_is_documented() {
    let Some(db) = isolated("autodoc").await else {
        return;
    };
    let pools = db.pools.clone();
    let (provider, _, _, ticket) = world(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let tech = Uuid::now_v7();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "knowledge", true, false)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Tess')")
        .bind(tech)
        .bind(format!("{}@example.com", unique("tess").replace(' ', ".")))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE sd_tickets SET assignee_user_id = $2 WHERE id = $1")
        .bind(ticket)
        .bind(tech)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let engine = engine_with_ai(&pools, provider).await;
    let (status, error) = run_pack(
        &engine,
        &pools,
        provider,
        ticket,
        DOCUMENT,
        "ticket.status_changed",
    )
    .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let note: String = sqlx::query_scalar(
        "SELECT body FROM sd_ticket_notes WHERE ticket_id = $1 AND kind = 'internal'",
    )
    .bind(ticket)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(
        note,
        "Resolution summary (AI draft): The VPN client's certificate had expired; renewed it."
    );
    let (title, state, tags): (String, String, Vec<String>) = sqlx::query_as(
        "SELECT a.title, a.status, a.tags FROM kb_articles a JOIN kb_article_tickets l ON l.article_id = a.id \
          WHERE l.ticket_id = $1",
    )
    .bind(ticket)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(
        (title.as_str(), state.as_str()),
        (
            "VPN fails after the client certificate expires",
            "in_review"
        )
    );
    assert_eq!(tags, ["ai-draft"]);
    let (who, minutes): (Uuid, i32) =
        sqlx::query_as("SELECT user_id, minutes FROM sd_time_suggestions WHERE ticket_id = $1 AND status = 'pending'")
            .bind(ticket)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert_eq!((who, minutes), (tech, 45));
}

/// Runs a pack workflow as `event` about `ticket` would, until it finishes.
async fn run_pack(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    provider: Uuid,
    ticket: Uuid,
    yaml: &str,
    event: &str,
) -> (RunStatus, Option<Json>) {
    let document = serde_json::to_value(huxley_hwf::parse_yaml(yaml).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let run = engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: provider,
                workflow_id: None,
                version: None,
                document: Some(document),
                trigger_type: "event".into(),
                trigger: json!({ "event_type": event, "subject_type": "ticket", "subject_id": ticket }),
                inputs: json!({ "reference": "T-1", "actorId": null }),
                actor_id: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.drain("triage-test", 50).await.unwrap();
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
async fn a_report_is_emailed_as_a_spreadsheet() {
    let Some(db) = isolated("report_send").await else {
        return;
    };
    let pools = db.pools.clone();
    let (provider, _, _, _) = world(&pools).await;
    let reader = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Reggie')")
        .bind(reader)
        .bind(format!(
            "{}@example.com",
            unique("reggie").replace(' ', ".")
        ))
        .execute(&pools.owner)
        .await
        .unwrap();
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let (mailer, sent) = huxley_email::Mailer::memory();
    let engine = Engine::with_config(pools.app.clone(), crypto, &EngineConfig::default())
        .with_mailer(mailer);
    let yaml = format!(
        "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: {{ key: report, name: Report }}\nspec:\n  steps:\n    \
         - id: send\n      type: core.report.send\n      with: {{ dashboard_id: service-desk, format: xlsx, to: \"user:{reader}\", \
         subject: Weekly desk }}\n"
    );
    let (status, error) =
        run_pack(&engine, &pools, provider, Uuid::now_v7(), &yaml, "manual").await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Weekly desk");
    let attached = &sent[0].attachments[0];
    assert_eq!(attached.filename, "service-desk-overview.xlsx");
    assert_eq!(&attached.bytes[..2], b"PK");
}

#[tokio::test]
async fn active_projects_get_a_drafted_status_report() {
    let Some(db) = isolated("status_reports").await else {
        return;
    };
    let pools = &db.pools;
    let (provider, _, _, ticket) = world(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "projects", true, false)
        .await
        .unwrap();
    let client: Uuid = sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
        .bind(ticket)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let project = huxley_servicedesk::projects::create(
        &mut tx,
        &ctx,
        &huxley_servicedesk::projects::NewProject {
            org_id: client,
            name: "Firewall swap".into(),
            start_on: "2026-11-01".into(),
            budget_hours: Some(10.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE prj_projects SET status = 'active' WHERE id = $1")
        .bind(project.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let engine = engine_with_ai(pools, provider).await;
    let (status, error) = run_pack(&engine, pools, provider, project.id, STATUS, "manual").await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let (body, visible, author): (String, bool, String) = sqlx::query_as(
        "SELECT body, client_visible, author FROM prj_status_reports WHERE project_id = $1",
    )
    .bind(project.id)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert!(
        body.starts_with("The firewall was swapped on schedule."),
        "{body}"
    );
    assert!(
        body.contains("**Highlights**\n\n- Firewall swapped"),
        "{body}"
    );
    assert!(!body.contains("Risks"), "no risks, no section: {body}");
    assert!(
        body.contains("**Next steps**\n\n- Cut over the VPN"),
        "{body}"
    );
    assert_eq!((visible, author.as_str()), (false, "AI status reports"));
}
