//! `approval` steps: approvers are told, the first decision counts, and
//! the run continues with it.

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::approval::{self, ApprovalStatus, DecisionError, Verdict};
use huxley_engine::{Engine, RunStatus, StartRun};
use serde_json::{Value as Json, json};
use uuid::Uuid;

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    org: Uuid,
}

async fn harness() -> Option<Harness> {
    let db = isolated("approvals").await?;
    let pools = db.pools.clone();
    let org = ensure_root(&pools).await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto);
    Some(Harness {
        _db: db,
        pools,
        engine,
        org,
    })
}

#[derive(Debug, sqlx::FromRow)]
struct RunRow {
    status: RunStatus,
    outputs: Option<Json>,
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

    async fn start(&self, approvers: &str, expires: &str) -> Uuid {
        let yaml = format!(
            r#"apiVersion: usehuxley.io/v1
kind: Workflow
metadata: {{ key: ok, name: Approve }}
spec:
  steps:
    - id: ask
      type: approval
      title: "Buy 5 licenses?"
      message: For the new starters.
      approvers: {approvers}
      expires: {expires}
    - id: route
      type: if
      condition: "{{{{ steps.ask.output.decision == 'approved' }}}}"
      then: [ {{ id: buy, type: transform, output: bought }} ]
      else: [ {{ id: skip, type: transform, output: skipped }} ]
  outputs:
    decision: "{{{{ steps.ask.output.decision }}}}"
    by: "{{{{ steps.ask.output.by.name }}}}"
    comment: "{{{{ steps.ask.output.comment }}}}"
"#
        );
        let document = serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap();
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
                    trigger_type: "manual".into(),
                    trigger: json!({}),
                    inputs: json!({}),
                    actor_id: None,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn run(&self, id: Uuid) -> RunRow {
        sqlx::query_as("SELECT status, outputs FROM runs WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }

    async fn step_status(&self, id: Uuid, key: &str) -> String {
        sqlx::query_scalar(
            "SELECT status::text FROM step_runs WHERE run_id = $1 AND step_key = $2 \
              ORDER BY attempt DESC LIMIT 1",
        )
        .bind(id)
        .bind(key)
        .fetch_optional(&self.pools.owner)
        .await
        .unwrap()
        .unwrap_or_else(|| "missing".into())
    }

    async fn until(&self, what: &str, done: impl AsyncFn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done().await {
            assert!(Instant::now() < deadline, "{what} didn't happen");
            self.engine.drain("test", 20).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn approval_of(&self, run: Uuid) -> Uuid {
        sqlx::query_scalar("SELECT id FROM approvals WHERE run_id = $1")
            .bind(run)
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn the_first_decision_continues_the_run() {
    let Some(h) = harness().await else { return };
    let boss = h.user("Boss").await;
    let other = h.user("Other").await;
    let id = h.start(&format!("[\"user:{boss}\"]"), "1h").await;
    h.until("the step parking", async || {
        h.step_status(id, "ask").await == "waiting"
    })
    .await;
    let approval_id = h.approval_of(id).await;

    // The approver was told, with a link to the approval.
    let (title, link): (String, Option<String>) =
        sqlx::query_as("SELECT title, link FROM notifications WHERE user_id = $1")
            .bind(boss)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap();
    assert_eq!(title, "Approval needed: Buy 5 licenses?");
    assert_eq!(
        link.as_deref(),
        Some(format!("/approvals/{approval_id}").as_str())
    );

    // Listed for the approver only.
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let mine = approval::for_approver(&mut tx, boss).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].status, ApprovalStatus::Pending);
    assert!(
        approval::for_approver(&mut tx, other)
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();

    let ctx = RequestContext::system(Via::System);
    let err = h
        .engine
        .decide_approval(&ctx, approval_id, other, true, None)
        .await
        .unwrap_err();
    assert!(matches!(err, DecisionError::NotAnApprover), "{err}");
    let view = h
        .engine
        .decide_approval(&ctx, approval_id, boss, true, Some(" go ahead "))
        .await
        .unwrap();
    assert_eq!(view.status, ApprovalStatus::Approved);
    let again = h
        .engine
        .decide_approval(&ctx, approval_id, boss, false, None)
        .await
        .unwrap_err();
    assert!(matches!(again, DecisionError::Closed), "{again}");

    h.until("the run finishing", async || {
        h.run(id).await.status.is_finished()
    })
    .await;
    let run = h.run(id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    assert_eq!(
        run.outputs.unwrap(),
        json!({ "decision": "approved", "by": "Boss", "comment": "go ahead" })
    );
    assert_eq!(h.step_status(id, "buy").await, "succeeded");
}

#[tokio::test]
async fn a_rejection_takes_the_other_branch() {
    let Some(h) = harness().await else { return };
    let boss = h.user("Boss").await;
    let id = h.start(&format!("\"user:{boss}\""), "1h").await;
    h.until("the step parking", async || {
        h.step_status(id, "ask").await == "waiting"
    })
    .await;
    let ctx = RequestContext::system(Via::System);
    h.engine
        .decide_approval(&ctx, h.approval_of(id).await, boss, false, None)
        .await
        .unwrap();
    h.until("the run finishing", async || {
        h.run(id).await.status.is_finished()
    })
    .await;
    assert_eq!(h.run(id).await.outputs.unwrap()["decision"], "rejected");
    assert_eq!(h.step_status(id, "skip").await, "succeeded");
    assert_eq!(h.step_status(id, "buy").await, "missing");
}

#[tokio::test]
async fn approvals_nobody_decides_expire() {
    let Some(h) = harness().await else { return };
    let boss = h.user("Boss").await;
    let id = h.start(&format!("\"user:{boss}\""), "1s").await;
    h.until("the step parking", async || {
        h.step_status(id, "ask").await == "waiting"
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    h.until("the run finishing", async || {
        h.run(id).await.status.is_finished()
    })
    .await;
    assert_eq!(h.run(id).await.status, RunStatus::Failed);
    let ctx = RequestContext::system(Via::System);
    let late = h
        .engine
        .decide_approval(&ctx, h.approval_of(id).await, boss, true, None)
        .await
        .unwrap_err();
    assert!(matches!(late, DecisionError::Closed));
}

#[tokio::test]
async fn approvers_must_resolve_to_someone() {
    let Some(h) = harness().await else { return };
    let id = h.start("\"role:nobody_has_this\"", "1h").await;
    h.until("the run finishing", async || {
        h.run(id).await.status.is_finished()
    })
    .await;
    assert_eq!(h.run(id).await.status, RunStatus::Failed);
}

impl Harness {
    /// A contact at the harness org with a portal account in `role`.
    async fn portal_contact(&self, first: &str, role: &str) -> (Uuid, Uuid) {
        let contact = Uuid::now_v7();
        sqlx::query("INSERT INTO crm_contacts (id, org_id, first_name, last_name, email) VALUES ($1, $2, $3, 'Client', $4)")
            .bind(contact)
            .bind(self.org)
            .bind(first)
            .bind(format!("{}@client.example", contact.simple()))
            .execute(&self.pools.owner)
            .await
            .unwrap();
        let account = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO portal_accounts (id, contact_id, org_id, email, role, status) VALUES ($1, $2, $3, $4, $5, 'active')",
        )
        .bind(account)
        .bind(contact)
        .bind(self.org)
        .bind(format!("{}@client.example", contact.simple()))
        .bind(role)
        .execute(&self.pools.owner)
        .await
        .unwrap();
        (contact, account)
    }
}

#[tokio::test]
async fn client_contacts_decide_in_the_portal() {
    let Some(h) = harness().await else { return };
    let ctx = RequestContext::system(Via::System);
    let (_, approver) = h.portal_contact("Cara", "client_approver").await;
    let (_, admin) = h.portal_contact("Abe", "client_admin").await;
    let (user_contact, user) = h.portal_contact("Uma", "client_user").await;

    // A client user can't be named: the step fails with a reason.
    let refused = h.start(&format!("\"contact:{user_contact}\""), "1d").await;
    h.until("the refusal", async || {
        h.run(refused).await.status == RunStatus::Failed
    })
    .await;

    // client_approvers: the company's approvers and admins.
    let id = h.start("[client_approvers]", "1d").await;
    h.until("the approval", async || {
        h.step_status(id, "ask").await == "waiting"
    })
    .await;
    let approval_id = h.approval_of(id).await;
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let waiting = approval::portal_approvals(&mut tx, approver).await.unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(
        (waiting[0].title.as_str(), waiting[0].status.as_str()),
        ("Buy 5 licenses?", "pending")
    );
    assert_eq!(
        approval::portal_approvals(&mut tx, admin)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        approval::portal_approvals(&mut tx, user)
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();

    // A staff user isn't on it, nor is the client user.
    let staff = h.user("Staff").await;
    assert!(matches!(
        h.engine
            .decide_approval(&ctx, approval_id, staff, true, None)
            .await,
        Err(DecisionError::NotAnApprover)
    ));
    assert!(matches!(
        h.engine
            .decide_portal_approval(&ctx, approval_id, user, Verdict::Approve, None)
            .await,
        Err(DecisionError::NotAnApprover)
    ));
    h.engine
        .decide_portal_approval(
            &ctx,
            approval_id,
            approver,
            Verdict::Approve,
            Some("OK by us"),
        )
        .await
        .unwrap();
    h.until("the run to finish", async || {
        h.run(id).await.status == RunStatus::Succeeded
    })
    .await;
    let outputs = h.run(id).await.outputs.unwrap();
    assert_eq!(outputs["decision"], "approved");
    assert_eq!(outputs["by"], "Cara Client");
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let decided = approval::portal_approval(&mut tx, admin, approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decided.decided_by_name.as_deref(), Some("Cara Client"));
    assert!(matches!(
        h.engine
            .decide_portal_approval(&ctx, approval_id, admin, Verdict::Reject, None)
            .await,
        Err(DecisionError::Closed)
    ));
}
