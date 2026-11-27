//! The built-in Invoice review pack (ADR-076): an invoice changed by hand
//! needs review; submitted, it goes to its reviewers, and an approved
//! review approves the invoice and tells whoever submitted it.

use std::time::{Duration, Instant};

use huxley_billing::invoices::{self, Status};
use huxley_billing::review::{self, ReviewStatus};
use huxley_core::{Actor, RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::approval::Verdict;
use huxley_engine::{Engine, RunStatus, StartRun};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const WORKFLOW: &str =
    include_str!("../../huxley-packs/builtin/invoice-review/workflows/review-invoice.yaml");

/// Two people (a biller and the reviewer the rules name), a client, and a
/// draft invoice for it: `(client, biller, reviewer, invoice)`, in `tx`.
async fn world(
    pools: &huxley_db::testing::TestPools,
    tx: &mut sqlx::PgConnection,
) -> (Uuid, Uuid, Uuid, Uuid) {
    let root = ensure_root(pools).await;
    let system = RequestContext::system(Via::System);
    let mut people = Vec::new();
    for name in ["Biller", "Reviewer"] {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(format!("{}@example.com", id.simple()))
            .bind(name)
            .execute(&pools.owner)
            .await
            .unwrap();
        people.push(id);
    }
    let (biller, reviewer) = (people[0], people[1]);

    huxley_modules::set_enabled(&mut *tx, &system, "billing", true, true)
        .await
        .unwrap();
    let provider = huxley_orgs::create(
        &mut *tx,
        &system,
        root,
        huxley_orgs::OrgKind::Provider,
        "MSP",
    )
    .await
    .unwrap()
    .id;
    let client = huxley_orgs::create(
        &mut *tx,
        &system,
        provider,
        huxley_orgs::OrgKind::Client,
        "Acme",
    )
    .await
    .unwrap()
    .id;
    // The out-of-the-box rule names admins; this one names the reviewer.
    sqlx::query("UPDATE billing_review_rules SET reviewers = $1")
        .bind(format!("user:{reviewer}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO billing_invoices (id, org_id) VALUES ($1, $2)")
        .bind(id)
        .bind(client)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO billing_invoice_lines (id, invoice_id, org_id, position, kind, description, quantity, unit_price) \
         VALUES ($1, $2, $3, 0, 'time', 'Support', 2, 100)",
    )
    .bind(Uuid::now_v7())
    .bind(id)
    .bind(client)
    .execute(&mut *tx)
    .await
    .unwrap();
    (client, biller, reviewer, id)
}

/// Runs the pack as `invoice.submitted_for_review` would; `reviewer` approves.
async fn review_with_pack(
    engine: &Engine,
    pools: &huxley_db::testing::TestPools,
    client: Uuid,
    payload: Json,
    reviewer: Uuid,
) {
    let system = RequestContext::system(Via::System);
    // The pack asks the reviewer, who approves.
    let document = serde_json::to_value(huxley_hwf::parse_yaml(WORKFLOW).unwrap()).unwrap();
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
                trigger: json!({}),
                inputs: payload,
                actor_id: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let pending = || async {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM approvals WHERE run_id = $1 AND status = 'pending'",
        )
        .bind(run)
        .fetch_optional(&pools.owner)
        .await
        .unwrap()
    };
    let approval = loop {
        engine.drain("test", 20).await.unwrap();
        if let Some(a) = pending().await {
            break a;
        }
        assert!(Instant::now() < deadline, "the review didn't wait");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    engine
        .decide_approval_as(&system, approval, reviewer, Verdict::Approve, Some("Fine"))
        .await
        .unwrap();
    loop {
        engine.drain("test", 20).await.unwrap();
        let status: RunStatus = sqlx::query_scalar("SELECT status FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
        if status.is_finished() {
            assert_eq!(status, RunStatus::Succeeded);
            break;
        }
        assert!(Instant::now() < deadline, "the run didn't finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_invoice_changed_by_hand_is_reviewed_then_approved() {
    let Some(db) = isolated("invoice_review").await else {
        return;
    };
    let pools = db.pools.clone();
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto);
    let system = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let (client, biller, reviewer, id) = world(&pools, &mut tx).await;
    review::refresh(&mut tx, id).await.unwrap();
    let i = invoices::get(&mut tx, id).await.unwrap();
    assert_eq!(i.review_status, ReviewStatus::NotNeeded);

    // The biller lowers the price by hand: now it needs review.
    let ctx = RequestContext {
        actor: Actor::User(biller),
        ..system.clone()
    };
    let edited: invoices::InvoiceInput = serde_json::from_value(json!({
        "version": i.version,
        "lines": [{ "id": i.lines[0].id, "description": "Support", "quantity": "2", "unitPrice": "80" }]
    }))
    .unwrap();
    let i = invoices::update(&mut tx, &ctx, id, &edited).await.unwrap();
    assert_eq!(i.review_status, ReviewStatus::Needed);
    let err = invoices::approve(&mut tx, &ctx, id, i.version, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("needs review"), "{err}");
    let state = review::submit(&mut tx, &ctx, id, Some(i.version))
        .await
        .unwrap();
    assert_eq!(state.status, ReviewStatus::InReview);
    assert_eq!(state.reasons[0].detail, "1 line changed by hand: Support");
    let payload: Json = sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE event_type = 'invoice.submitted_for_review' AND subject_id = $1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(payload["total"], "160");
    tx.commit().await.unwrap();

    review_with_pack(&engine, &pools, client, payload, reviewer).await;

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let i = invoices::get(&mut tx, id).await.unwrap();
    assert_eq!(i.status, Status::Approved);
    assert_eq!(i.review_status, ReviewStatus::Approved);
    assert!(i.due_on.is_some());
    let told: String = sqlx::query_scalar(
        "SELECT title FROM notifications WHERE user_id = $1 AND kind = 'invoice.reviewed'",
    )
    .bind(biller)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(told.ends_with("for Acme was approved"), "{told}");
}
