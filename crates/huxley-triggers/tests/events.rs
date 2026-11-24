//! Event triggers against a real PostgreSQL. Each test has a database of its
//! own, since dispatching handles every pending event in the outbox.

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, isolated, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::Engine;
use huxley_orgs::OrgKind;
use serde_json::{Value, json};
use uuid::Uuid;

async fn publish(pools: &TestPools, org: Uuid, key: &str, trigger: Value) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let expr = huxley_expr::Engine::new();
    let document = json!({
        "apiVersion": "usehuxley.io/v1",
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

struct Setup {
    pools: TestPools,
    engine: Engine,
    crypto: huxley_crypto::Crypto,
    msp: Uuid,
}

async fn setup(db: &huxley_db::testing::IsolatedDb) -> Setup {
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto.clone());
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let msp = huxley_orgs::create(&mut tx, &ctx, root, OrgKind::Provider, &unique("Events"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Setup {
        pools,
        engine,
        crypto,
        msp: msp.id,
    }
}

/// Creates client orgs under `parent`, which writes `org.created` events.
async fn clients(pools: &TestPools, parent: Uuid, n: usize) -> Vec<Uuid> {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mut ids = Vec::new();
    for _ in 0..n {
        let org = huxley_orgs::create(&mut tx, &ctx, parent, OrgKind::Client, &unique("Client"))
            .await
            .unwrap();
        ids.push(org.id);
    }
    tx.commit().await.unwrap();
    ids
}

async fn pending(pools: &TestPools) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events_outbox WHERE published_at IS NULL")
        .fetch_one(&pools.owner)
        .await
        .unwrap()
}

async fn run_orgs(pools: &TestPools, workflow: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar("SELECT org_id FROM runs WHERE workflow_id = $1 ORDER BY org_id")
        .bind(workflow)
        .fetch_all(&pools.owner)
        .await
        .unwrap()
}

#[tokio::test]
async fn domain_events_start_matching_workflows() {
    let Some(db) = isolated("events").await else {
        return;
    };
    let Setup {
        pools,
        engine,
        crypto,
        msp,
    } = setup(&db).await;
    let ctx = RequestContext::system(Via::System);

    let clients_only = publish(
        &pools,
        msp,
        "new-clients",
        json!({ "type": "event", "event": "org.created", "filter": "{{ event.payload.kind == 'client' }}" }),
    )
    .await;
    let everything = publish(
        &pools,
        msp,
        "all-org-events",
        json!({ "type": "event", "event": "org.*" }),
    )
    .await;

    // A client and a provider appear under the MSP.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx, msp, OrgKind::Client, &unique("Client"))
        .await
        .unwrap();
    let division = huxley_orgs::create(&mut tx, &ctx, msp, OrgKind::Provider, &unique("Division"))
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

#[tokio::test]
async fn publishing_a_trigger_does_not_replay_history() {
    let Some(db) = isolated("events").await else {
        return;
    };
    let s = setup(&db).await;
    // Created before the trigger exists, and not dispatched yet.
    let before = clients(&s.pools, s.msp, 1).await;
    let workflow = publish(
        &s.pools,
        s.msp,
        "late",
        json!({ "type": "event", "event": "org.created" }),
    )
    .await;
    let after = clients(&s.pools, s.msp, 1).await;
    while pending(&s.pools).await > 0 {
        huxley_triggers::events::dispatch(&s.engine, &s.crypto)
            .await
            .unwrap();
    }
    assert_eq!(run_orgs(&s.pools, workflow).await, after);
    assert_ne!(before, after);
}

#[tokio::test]
async fn concurrent_dispatchers_start_each_run_once() {
    let Some(db) = isolated("events").await else {
        return;
    };
    let s = setup(&db).await;
    let workflow = publish(
        &s.pools,
        s.msp,
        "each-once",
        json!({ "type": "event", "event": "org.created" }),
    )
    .await;
    let mut created = clients(&s.pools, s.msp, 250).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while pending(&s.pools).await > 0 {
        let dispatchers: Vec<_> = (0..4)
            .map(|_| {
                let engine = s.engine.clone();
                let crypto = s.crypto.clone();
                tokio::spawn(
                    async move { huxley_triggers::events::dispatch(&engine, &crypto).await },
                )
            })
            .collect();
        for d in dispatchers {
            d.await.unwrap().unwrap();
        }
        assert!(Instant::now() < deadline, "events weren't dispatched");
    }
    created.sort();
    assert_eq!(run_orgs(&s.pools, workflow).await, created);
}
