//! Scheduled dataset syncs against a real PostgreSQL.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::Engine;
use huxley_orgs::OrgKind;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn due_syncs_start_a_run_per_client_and_reschedule() {
    // Its own database: tick() starts every due sync.
    let Some(db) = isolated("dataset_sync").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let msp = huxley_orgs::create(&mut tx, &ctx, root, OrgKind::Provider, &unique("Sync MSP"))
        .await
        .unwrap()
        .id;
    let mut clients = Vec::new();
    for _ in 0..2 {
        clients.push(
            huxley_orgs::create(&mut tx, &ctx, msp, OrgKind::Client, &unique("Sync client"))
                .await
                .unwrap()
                .id,
        );
    }
    let input: huxley_datasets::DatasetInput = serde_json::from_value(json!({
        "key": "users", "name": "Users", "columns": [{ "name": "upn", "type": "text" }], "primaryKey": ["upn"]
    }))
    .unwrap();
    let dataset = huxley_datasets::create(&mut tx, &ctx, msp, &input)
        .await
        .unwrap();
    let config: huxley_datasets::sync::SyncConfig = serde_json::from_value(json!({
        "action": "msgraph/listUsers", "map": { "upn": "userPrincipalName" },
        "orgs": "clients", "schedule": { "interval": "1h" }
    }))
    .unwrap();
    // Due a minute ago.
    let past = time::OffsetDateTime::now_utc() - time::Duration::minutes(1);
    huxley_datasets::sync::set(&mut tx, &ctx, &dataset, Some(&config), Some(past))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto);
    assert_eq!(huxley_triggers::datasets::tick(&engine).await.unwrap(), 2);
    let mut orgs: Vec<Uuid> = sqlx::query_scalar(
        "SELECT org_id FROM runs WHERE trigger_type = 'dataset_sync' AND trigger->>'dataset' = $1",
    )
    .bind(dataset.id.to_string())
    .fetch_all(&pools.owner)
    .await
    .unwrap();
    orgs.sort();
    clients.sort();
    assert_eq!(orgs, clients);
    let (next, last): (time::OffsetDateTime, Option<time::OffsetDateTime>) =
        sqlx::query_as("SELECT sync_next_at, sync_last_at FROM datasets WHERE id = $1")
            .bind(dataset.id)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert!(
        next > time::OffsetDateTime::now_utc() + time::Duration::minutes(50),
        "{next}"
    );
    assert!(last.is_some());
    // Nothing more is due.
    assert_eq!(huxley_triggers::datasets::tick(&engine).await.unwrap(), 0);
}
