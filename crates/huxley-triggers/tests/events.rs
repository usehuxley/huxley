//! Event triggers against a real PostgreSQL.

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::Engine;
use huxley_orgs::OrgKind;
use serde_json::{Value, json};
use uuid::Uuid;

async fn publish(pools: &TestPools, org: Uuid, key: &str, trigger: Value) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let expr = huxley_expr::Engine::new();
    let document = json!({
        "apiVersion": "usehuxley.com/v1",
        "kind": "Workflow",
        "metadata": { "key": key, "name": key },
        "spec": {
            "triggers": [trigger],
            "steps": [{ "id": "seen", "type": "transform", "output": "{{ trigger.payload.name }}" }]
        }
    });
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

#[tokio::test]
async fn domain_events_start_matching_workflows() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto.clone());
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let msp = huxley_orgs::create(&mut tx, &ctx, root, OrgKind::Provider, &unique("Events"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let clients_only = publish(
        &pools,
        msp.id,
        "new-clients",
        json!({ "type": "event", "event": "org.created", "filter": "{{ event.payload.kind == 'client' }}" }),
    )
    .await;
    let everything = publish(
        &pools,
        msp.id,
        "all-org-events",
        json!({ "type": "event", "event": "org.*" }),
    )
    .await;

    // A client and a provider appear under the MSP; one is renamed.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx, msp.id, OrgKind::Client, &unique("Client"))
        .await
        .unwrap();
    let division = huxley_orgs::create(
        &mut tx,
        &ctx,
        msp.id,
        OrgKind::Provider,
        &unique("Division"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        huxley_triggers::events::dispatch(&engine, &crypto)
            .await
            .unwrap();
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events_outbox WHERE subject_id = ANY($1) AND published_at IS NULL",
        )
        .bind(vec![client.id, division.id])
        .fetch_one(&pools.owner)
        .await
        .unwrap();
        if pending == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "events weren't dispatched");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let runs = |workflow: Uuid| {
        let pools = pools.owner.clone();
        async move {
            sqlx::query_as::<_, (Uuid, String, Value)>(
                "SELECT org_id, trigger_type, trigger FROM runs WHERE workflow_id = $1 ORDER BY id",
            )
            .bind(workflow)
            .fetch_all(&pools)
            .await
            .unwrap()
        }
    };
    let client_runs = runs(clients_only).await;
    assert_eq!(client_runs.len(), 1, "only the client matched the filter");
    let (org, kind, trigger) = &client_runs[0];
    assert_eq!(*org, client.id, "runs in the event's org");
    assert_eq!(kind, "event");
    assert_eq!(trigger["type"], "org.created");
    assert_eq!(trigger["payload"]["kind"], "client");
    assert_eq!(
        runs(everything).await.len(),
        2,
        "both creations match org.*"
    );
}
