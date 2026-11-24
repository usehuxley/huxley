//! The scheduler against a real PostgreSQL.

use std::time::Duration;

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::Engine;
use serde_json::{Value, json};
use uuid::Uuid;

async fn org(pools: &TestPools) -> Uuid {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Sched"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    org.id
}

fn document(key: &str, triggers: &Value, inputs: Option<Value>) -> Value {
    let mut spec = json!({
        "triggers": triggers,
        "steps": [{ "id": "hello", "type": "transform", "output": "{{ trigger.id }}" }],
    });
    if let Some(inputs) = inputs {
        spec["inputs"] = inputs;
    }
    json!({
        "apiVersion": "usehuxley.io/v1",
        "kind": "Workflow",
        "metadata": { "key": key, "name": "Scheduled" },
        "spec": spec,
    })
}

/// Creates and publishes a workflow; returns its id.
async fn publish(pools: &TestPools, org: Uuid, document: Value) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let expr = huxley_expr::Engine::new();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let created = huxley_workflows::create(&mut tx, &ctx, &expr, org, document)
        .await
        .unwrap();
    huxley_workflows::publish(&mut tx, &ctx, &expr, created.summary.id, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    created.summary.id
}

async fn runs(pools: &TestPools, workflow: Uuid) -> Vec<(String, Value)> {
    sqlx::query_as("SELECT trigger_type, trigger FROM runs WHERE workflow_id = $1 ORDER BY id")
        .bind(workflow)
        .fetch_all(&pools.owner)
        .await
        .unwrap()
}

async fn trigger_times(
    pools: &TestPools,
    workflow: Uuid,
) -> Vec<(
    String,
    Option<time::OffsetDateTime>,
    Option<time::OffsetDateTime>,
)> {
    sqlx::query_as(
        "SELECT trigger_id, next_run_at, last_run_at FROM workflow_triggers \
          WHERE workflow_id = $1 ORDER BY trigger_id",
    )
    .bind(workflow)
    .fetch_all(&pools.owner)
    .await
    .unwrap()
}

/// Makes a schedule due now (instead of waiting for it).
async fn make_due(pools: &TestPools, workflow: Uuid, trigger: &str) {
    sqlx::query(
        "UPDATE workflow_triggers SET next_run_at = now() - interval '1 second' \
          WHERE workflow_id = $1 AND trigger_id = $2",
    )
    .bind(workflow)
    .bind(trigger)
    .execute(&pools.owner)
    .await
    .unwrap();
}

/// Ticks until `trigger` has fired once. Another test's tick may hold the
/// row (SKIP LOCKED) for a moment.
async fn tick_until_fired(engine: &Engine, pools: &TestPools, workflow: Uuid, trigger: &str) {
    for _ in 0..50 {
        huxley_triggers::tick(engine).await.unwrap();
        let times = trigger_times(pools, workflow).await;
        if times.iter().any(|t| t.0 == trigger && t.2.is_some()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{trigger} never fired");
}

#[tokio::test]
async fn due_schedules_start_runs() {
    let Some(pools) = pools().await else { return };
    let org = org(&pools).await;
    let engine = Engine::new(
        pools.app.clone(),
        huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    );
    let key = format!("sched-{}", &Uuid::now_v7().simple().to_string()[20..]);
    let workflow = publish(
        &pools,
        org,
        document(
            &key,
            &json!([
                { "type": "schedule", "id": "often", "interval": "1h" },
                { "type": "schedule", "id": "daily", "cron": "0 7 * * *", "timezone": "Europe/London" },
            ]),
            None,
        ),
    )
    .await;
    let times = trigger_times(&pools, workflow).await;
    assert_eq!(times.len(), 2);
    assert!(
        times
            .iter()
            .all(|(_, next, last)| next.is_some() && last.is_none())
    );
    let daily_next = times[0].1.unwrap();
    assert_eq!(times[0].0, "daily");
    assert_eq!(daily_next.minute(), 0);

    make_due(&pools, workflow, "often").await;
    tick_until_fired(&engine, &pools, workflow, "often").await;
    let started = runs(&pools, workflow).await;
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0].0, "schedule");
    assert_eq!(started[0].1["id"], "often");
    let times = trigger_times(&pools, workflow).await;
    let often = times.iter().find(|t| t.0 == "often").unwrap();
    assert!(
        often.2.is_some() && often.1 > often.2,
        "moved on: {often:?}"
    );
    assert_eq!(
        times.iter().find(|t| t.0 == "daily").unwrap().1,
        Some(daily_next)
    );

    // Archiving removes the triggers; restoring brings them back.
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_workflows::set_archived(&mut tx, &ctx, workflow, true)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(trigger_times(&pools, workflow).await.len(), 0);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_workflows::set_archived(&mut tx, &ctx, workflow, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(trigger_times(&pools, workflow).await.len(), 2);
}

#[tokio::test]
async fn schedules_with_bad_inputs_are_skipped_and_webhook_ids_are_stable() {
    let Some(pools) = pools().await else { return };
    let org = org(&pools).await;
    let engine = Engine::new(
        pools.app.clone(),
        huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    );
    let key = format!("skip-{}", &Uuid::now_v7().simple().to_string()[20..]);
    let schema = json!({ "type": "object", "required": ["who"] });
    let triggers = json!([
        { "type": "schedule", "interval": "1h" },
        { "type": "webhook" },
    ]);
    let workflow = publish(&pools, org, document(&key, &triggers, Some(schema.clone()))).await;
    make_due(&pools, workflow, "schedule").await;
    tick_until_fired(&engine, &pools, workflow, "schedule").await;
    assert_eq!(runs(&pools, workflow).await, vec![], "no `who`, so no run");

    let public = |pools: &TestPools| {
        let pools = pools.owner.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT public_id FROM workflow_triggers WHERE workflow_id = $1 AND type = 'webhook'",
            )
            .bind(workflow)
            .fetch_one(&pools)
            .await
            .unwrap()
        }
    };
    let before = public(&pools).await;
    assert!(before.starts_with("wh_"));
    // Republishing keeps the webhook URL.
    let ctx = RequestContext::system(Via::System);
    let expr = huxley_expr::Engine::new();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let detail = huxley_workflows::get(&mut tx, &expr, workflow)
        .await
        .unwrap();
    let mut changed = detail.draft.clone();
    changed["spec"]["steps"][0]["output"] = json!("v2");
    huxley_workflows::save_draft(
        &mut tx,
        &ctx,
        &expr,
        workflow,
        detail.summary.revision,
        changed,
    )
    .await
    .unwrap();
    huxley_workflows::publish(&mut tx, &ctx, &expr, workflow, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(public(&pools).await, before);
}
