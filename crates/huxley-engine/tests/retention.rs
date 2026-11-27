//! Data retention (ADR-085): finished runs older than the retention go,
//! with their spilled payloads; recent and unfinished runs stay; old
//! published events, read notifications and AI call records go.

use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::Engine;
use uuid::Uuid;

/// A run with an id from `months` ago, in `status`.
async fn run_from(
    pools: &huxley_db::testing::TestPools,
    org: Uuid,
    months: i32,
    status: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO runs (id, org_id, document, status, trigger_type) \
         VALUES ((left(huxley_uuid7_floor(now() - make_interval(months => $2))::text, 14) \
                  || substr(gen_random_uuid()::text, 15))::uuid, $1, '{}', $3::run_status, 'manual') RETURNING id",
    )
    .bind(org)
    .bind(months)
    .bind(status)
    .fetch_one(&pools.owner)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_sweep_removes_what_is_past_its_retention() {
    let Some(db) = isolated("retention").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let storage = huxley_storage::Storage::memory();
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto).with_storage(storage.clone());

    let old = run_from(&pools, root, 14, "succeeded").await;
    let old_waiting = run_from(&pools, root, 14, "running").await;
    let recent = run_from(&pools, root, 1, "failed").await;
    storage
        .put(
            &format!("workflow-payloads/{root}/{old}/a.json"),
            "{}".into(),
        )
        .await
        .unwrap();
    storage
        .put(
            &format!("workflow-payloads/{root}/{recent}/a.json"),
            "{}".into(),
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO events_outbox (id, event_type, subject_type, subject_id, payload, occurred_at, published_at) \
         VALUES (gen_random_uuid(), 'x.old', 'x', gen_random_uuid(), '{}', now() - interval '60 days', now() - interval '60 days'), \
                (gen_random_uuid(), 'x.new', 'x', gen_random_uuid(), '{}', now(), now())",
    )
    .execute(&pools.owner)
    .await
    .unwrap();

    let swept = engine.sweep_retention().await.unwrap();
    assert_eq!(swept.runs, 1, "{swept:?}");
    assert_eq!(swept.payload_objects, 1);
    assert!(swept.events >= 1);
    let left: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM runs WHERE id = ANY($1)")
        .bind(vec![old, old_waiting, recent])
        .fetch_all(&pools.owner)
        .await
        .unwrap();
    assert!(!left.contains(&old));
    assert!(left.contains(&old_waiting), "unfinished runs stay");
    assert!(left.contains(&recent));
    assert!(
        storage
            .get(&format!("workflow-payloads/{root}/{recent}/a.json"))
            .await
            .is_ok()
    );
    let new_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events_outbox WHERE event_type = 'x.new'")
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert_eq!(new_events, 1);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let state = huxley_engine::retention::get(&mut tx).await.unwrap();
    assert_eq!(state.last_sweep.unwrap().runs, 1);
}
