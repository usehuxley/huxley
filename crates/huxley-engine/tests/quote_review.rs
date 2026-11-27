//! The built-in Quote review pack: a quote submitted for review goes to
//! its reviewers, whose decision (including "changes requested") lands on
//! the quote.

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::approval::{DecisionError, Verdict};
use huxley_engine::{Engine, RunStatus, StartRun};
use huxley_sales::quotes::{self, LineInput, QuoteInput};
use huxley_sales::review::{self, ReviewStatus};
use serde_json::{Value as Json, json};
use uuid::Uuid;

const WORKFLOW: &str =
    include_str!("../../huxley-packs/builtin/quote-review/workflows/review-quote.yaml");

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    client: Uuid,
}

async fn harness() -> Option<Harness> {
    let db = isolated("quote_review").await?;
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto);
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
    tx.commit().await.unwrap();
    Some(Harness {
        _db: db,
        pools,
        engine,
        client,
    })
}

impl Harness {
    async fn user(&self, name: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(format!("{}@example.com", id.simple()))
            .bind(name)
            .execute(&self.pools.owner)
            .await
            .unwrap();
        id
    }

    async fn tx(&self) -> sqlx::Transaction<'static, sqlx::Postgres> {
        begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap()
    }

    /// Starts the pack's workflow as the event would, with its payload.
    async fn start(&self, payload: Json) -> Uuid {
        let document = serde_json::to_value(huxley_hwf::parse_yaml(WORKFLOW).unwrap()).unwrap();
        let mut tx = self.tx().await;
        let id = self
            .engine
            .start_run(
                &mut tx,
                StartRun {
                    org_id: self.client,
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
        id
    }

    async fn until(&self, what: &str, done: impl AsyncFn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done().await {
            assert!(Instant::now() < deadline, "{what} didn't happen");
            self.engine.drain("test", 20).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn approval(&self, run: Uuid) -> Option<Uuid> {
        sqlx::query_scalar("SELECT id FROM approvals WHERE run_id = $1 AND status = 'pending'")
            .bind(run)
            .fetch_optional(&self.pools.owner)
            .await
            .unwrap()
    }

    async fn status(&self, run: Uuid) -> RunStatus {
        sqlx::query_scalar("SELECT status FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }

    /// The latest `quote.submitted_for_review` payload for a quote.
    async fn submitted(&self, quote: Uuid) -> Json {
        sqlx::query_scalar(
            "SELECT payload FROM events_outbox WHERE event_type = 'quote.submitted_for_review' \
              AND subject_id = $1 ORDER BY occurred_at DESC, id DESC LIMIT 1",
        )
        .bind(quote)
        .fetch_one(&self.pools.owner)
        .await
        .unwrap()
    }
}

impl Harness {
    /// The title of what `user` was told about a review.
    async fn told(&self, user: Uuid) -> String {
        sqlx::query_scalar(
            "SELECT title FROM notifications WHERE user_id = $1 AND kind = 'quote.reviewed'",
        )
        .bind(user)
        .fetch_one(&self.pools.owner)
        .await
        .unwrap()
    }

    /// Discounts over 20% need `reviewer`.
    async fn discount_rule(&self, reviewer: Uuid) {
        let ctx = RequestContext::system(Via::System);
        let mut tx = self.tx().await;
        review::create_rule(
            &mut tx,
            &ctx,
            &serde_json::from_value(json!({
                "name": "Deep discounts", "kind": "discount_above", "threshold": "20",
                "reviewers": format!("user:{reviewer}")
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

fn line(name: &str, price: &str, discount: &str) -> LineInput {
    serde_json::from_value(json!({
        "name": name, "quantity": 1, "unitCost": "50", "unitPrice": price, "discountPct": discount
    }))
    .unwrap()
}

#[tokio::test]
async fn reviewers_send_a_quote_back_for_changes() {
    let Some(h) = harness().await else { return };
    let owner = h.user("Seller").await;
    let reviewer = h.user("Reviewer").await;
    let ctx = RequestContext::system(Via::System);

    h.discount_rule(reviewer).await;
    let mut tx = h.tx().await;
    let input = QuoteInput {
        owner_id: Some(owner),
        lines: vec![line("Laptop", "1000", "30")],
        ..serde_json::from_value(json!({ "title": "Laptops" })).unwrap()
    };
    let q = quotes::create(&mut tx, &ctx, h.client, &input)
        .await
        .unwrap();
    assert_eq!(q.review_status, ReviewStatus::Needed);
    // It can't go out unreviewed.
    let err = quotes::send(&mut tx, &ctx, q.id, q.version, false)
        .await
        .unwrap_err();
    assert!(
        matches!(err, huxley_sales::SalesError::Conflict(_)),
        "{err}"
    );
    let state = review::submit(&mut tx, &ctx, q.id, q.version)
        .await
        .unwrap();
    assert_eq!(state.status, ReviewStatus::InReview);
    assert_eq!(state.reasons[0].detail, "Laptop is 30% off (over 20%)");
    tx.commit().await.unwrap();

    let run = h.start(h.submitted(q.id).await).await;
    h.until("the review waiting", async || {
        h.approval(run).await.is_some()
    })
    .await;
    let approval = h.approval(run).await.unwrap();
    let ask = h
        .engine
        .decide_approval_as(
            &ctx,
            approval,
            reviewer,
            Verdict::RequestChanges,
            Some("Keep it to 20%"),
        )
        .await
        .unwrap();
    assert_eq!(
        ask.status,
        huxley_engine::approval::ApprovalStatus::ChangesRequested
    );
    h.until("the run finishing", async || {
        h.status(run).await.is_finished()
    })
    .await;
    assert_eq!(h.status(run).await, RunStatus::Succeeded);

    let mut tx = h.tx().await;
    let state = review::state(&mut tx, q.id).await.unwrap();
    assert_eq!(state.status, ReviewStatus::ChangesRequested);
    assert_eq!(
        state.history[0].decision.as_deref(),
        Some("changes_requested")
    );
    assert_eq!(state.history[0].comment.as_deref(), Some("Keep it to 20%"));
    assert!(!state.can_send);
    // The owner heard.
    let told = h.told(owner).await;
    assert!(told.starts_with("Changes requested on Q-"), "{told}");

    // Fixed and resubmitted: the reviewer sees what changed.
    let now = quotes::get(&mut tx, q.id).await.unwrap();
    let mut fixed = line("Laptop", "1000", "20");
    fixed.id = Some(now.lines[0].id);
    let input = QuoteInput {
        lines: vec![fixed],
        version: Some(now.version),
        ..input
    };
    let q = quotes::update(&mut tx, &ctx, q.id, &input).await.unwrap();
    let state = review::state(&mut tx, q.id).await.unwrap();
    assert_eq!(state.status, ReviewStatus::NotNeeded);
    let changes = state.changes.unwrap();
    assert_eq!(changes.changed[0].fields[0].field, "discount %");
    assert_eq!(changes.changed[0].fields[0].after, "20");
    // No rule matches now, so it can just go out.
    assert!(state.can_send);
    tx.commit().await.unwrap();

    // A stale review can't be decided: the decide step fails.
    let mut tx = h.tx().await;
    let err = review::decide(
        &mut tx,
        &ctx,
        state.history[0].id,
        review::Decision::Approved,
        "Someone",
        None,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, huxley_sales::SalesError::Conflict(_)),
        "{err}"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn approvers_ask_for_changes_only_where_the_step_allows() {
    let Some(h) = harness().await else { return };
    let boss = h.user("Boss").await;
    let yaml = format!(
        r#"apiVersion: usehuxley.io/v1
kind: Workflow
metadata: {{ key: plain, name: Plain }}
spec:
  steps:
    - {{ id: ask, type: approval, title: "OK?", approvers: "user:{boss}", expires: 1h }}
"#
    );
    let document = serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap();
    let mut tx = h.tx().await;
    let run = h
        .engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: h.client,
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
    h.until("the step waiting", async || h.approval(run).await.is_some())
        .await;
    let ctx = RequestContext::system(Via::System);
    let err = h
        .engine
        .decide_approval_as(
            &ctx,
            h.approval(run).await.unwrap(),
            boss,
            Verdict::RequestChanges,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DecisionError::ChangesNotAllowed), "{err}");
}
