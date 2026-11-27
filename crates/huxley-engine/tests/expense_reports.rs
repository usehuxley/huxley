//! The Expense approval pack (ADR-081): a technician's report goes to
//! their team lead; changes asked for send the expenses back to change and
//! submit again; approved locks them.

use std::time::{Duration, Instant};

use huxley_core::{Actor, RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::approval::Verdict;
use huxley_engine::{Engine, RunStatus, StartRun};
use huxley_expenses::entries::{self, ExpenseInput, Status};
use huxley_expenses::reports::{self, ReportStatus, SubmitInput};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const WORKFLOW: &str =
    include_str!("../../huxley-packs/builtin/expense-approval/workflows/approve-expenses.yaml");

async fn person(pools: &TestPools, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("{}@example.com", id.simple()))
        .bind(name)
        .execute(&pools.owner)
        .await
        .unwrap();
    id
}

/// Runs the pack on `payload`; `lead` gives `verdict` (with a comment).
async fn review(
    engine: &Engine,
    pools: &TestPools,
    msp: Uuid,
    payload: Json,
    lead: Uuid,
    verdict: Verdict,
) {
    let document = serde_json::to_value(huxley_hwf::parse_yaml(WORKFLOW).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let run = engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: msp,
                workflow_id: None,
                version: None,
                document: Some(document),
                trigger_type: "event".into(),
                trigger: json!({}),
                inputs: payload,
                actor_id: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let approval = loop {
        engine.drain("expenses", 20).await.unwrap();
        let pending: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM approvals WHERE run_id = $1 AND status = 'pending'")
                .bind(run)
                .fetch_optional(&pools.owner)
                .await
                .unwrap();
        if let Some(a) = pending {
            break a;
        }
        assert!(Instant::now() < deadline, "no approval asked for");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let system = RequestContext::system(Via::System);
    engine
        .decide_approval_as(
            &system,
            approval,
            lead,
            verdict,
            Some("Add the hotel receipt"),
        )
        .await
        .unwrap();
    loop {
        engine.drain("expenses", 20).await.unwrap();
        let status: RunStatus = sqlx::query_scalar("SELECT status FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
        if status.is_finished() {
            assert_eq!(status, RunStatus::Succeeded);
            return;
        }
        assert!(Instant::now() < deadline, "the run didn't finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn submitted(pools: &TestPools, report: Uuid) -> Json {
    sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE event_type = 'expense_report.submitted' AND subject_id = $1",
    )
    .bind(report)
    .fetch_one(&pools.owner)
    .await
    .unwrap()
}

/// A technician and their team lead at an MSP with a client:
/// `(msp, client, tech, lead, the travel category)`.
async fn world(pools: &TestPools) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
    let root = ensure_root(pools).await;
    let system = RequestContext::system(Via::System);
    let (tech, lead) = (person(pools, "Tech").await, person(pools, "Lead").await);
    let team = Uuid::now_v7();
    sqlx::query("INSERT INTO teams (id, name) VALUES ($1, 'Field')")
        .bind(team)
        .execute(&pools.owner)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO team_members (team_id, user_id, lead) VALUES ($1, $2, false), ($1, $3, true)",
    )
    .bind(team)
    .bind(tech)
    .bind(lead)
    .execute(&pools.owner)
    .await
    .unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &system, "expenses", true, true)
        .await
        .unwrap();
    let msp = huxley_orgs::create(
        &mut tx,
        &system,
        root,
        huxley_orgs::OrgKind::Provider,
        "MSP",
    )
    .await
    .unwrap()
    .id;
    let client = huxley_orgs::create(&mut tx, &system, msp, huxley_orgs::OrgKind::Client, "Acme")
        .await
        .unwrap()
        .id;
    let travel: Uuid =
        sqlx::query_scalar("SELECT id FROM exp_categories WHERE name = 'Travel and lodging'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    (msp, client, tech, lead, travel)
}

#[tokio::test]
async fn a_lead_sends_a_report_back_then_approves_it() {
    let Some(db) = isolated("expense_reports").await else {
        return;
    };
    let pools = db.pools.clone();
    let system = RequestContext::system(Via::System);
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto);
    let (msp, client, tech, lead, travel) = world(&pools).await;
    let ctx = RequestContext {
        actor: Actor::User(tech),
        ..system.clone()
    };
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let hotel: ExpenseInput = serde_json::from_value(json!({
        "categoryId": travel, "spentOn": today, "description": "Hotel", "amount": "180", "orgId": client
    }))
    .unwrap();
    let e = entries::create(&mut tx, &ctx, tech, &hotel).await.unwrap();
    let r = reports::submit(
        &mut tx,
        &ctx,
        tech,
        &SubmitInput {
            title: None,
            entry_ids: vec![e.id],
        },
    )
    .await
    .unwrap();
    assert_eq!(r.org_id, msp, "the report sits with the MSP");
    assert_eq!(r.reviewers, [format!("user:{lead}")]);
    assert_eq!(r.flags.get("no_receipt"), Some(&1));
    // Submitted: locked meanwhile.
    assert!(entries::update(&mut tx, &ctx, e.id, &hotel).await.is_err());
    tx.commit().await.unwrap();

    // Changes asked for: back to the technician, who can change it.
    review(
        &engine,
        &pools,
        msp,
        submitted(&pools, r.id).await,
        lead,
        Verdict::RequestChanges,
    )
    .await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let back = reports::get(&mut tx, r.id).await.unwrap();
    assert_eq!(back.status, ReportStatus::ChangesRequested);
    assert_eq!(back.comment.as_deref(), Some("Add the hotel receipt"));
    assert_eq!(back.entries[0].status, Status::Rejected);
    entries::update(&mut tx, &ctx, e.id, &hotel).await.unwrap();
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'expense.reviewed'",
    )
    .bind(tech)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(told, 1);
    let again = reports::submit(
        &mut tx,
        &ctx,
        tech,
        &SubmitInput {
            title: Some("Acme trip".into()),
            entry_ids: vec![e.id],
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Approved: locked for good.
    review(
        &engine,
        &pools,
        msp,
        submitted(&pools, again.id).await,
        lead,
        Verdict::Approve,
    )
    .await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let done = reports::get(&mut tx, again.id).await.unwrap();
    assert_eq!(done.status, ReportStatus::Approved);
    assert_eq!(done.entries[0].status, Status::Approved);
    assert_eq!(done.totals[0].reimbursable.normalize().to_string(), "180");
    assert!(entries::update(&mut tx, &ctx, e.id, &hotel).await.is_err());
}
