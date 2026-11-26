//! Warranties against a real PostgreSQL: the asset's end date follows them,
//! lookups record theirs once, and expiry is announced once each.

use huxley_cmdb::assets::{self, AssetInput};
use huxley_cmdb::warranties::{self, WarrantyInput};
use huxley_cmdb::{CmdbError, types};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn days(n: i64) -> time::Date {
    (OffsetDateTime::now_utc() + Duration::days(n)).date()
}

fn ends(n: i64) -> WarrantyInput {
    WarrantyInput {
        kind: None,
        provider: Some("Dell".into()),
        reference: None,
        starts_on: None,
        ends_on: days(n),
        notes: None,
    }
}

async fn events(tx: &mut sqlx::PgConnection, asset: Uuid, event: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events_outbox WHERE subject_id = $1 AND event_type = $2",
    )
    .bind(asset)
    .bind(event)
    .fetch_one(tx)
    .await
    .unwrap()
}

#[tokio::test]
async fn warranties_follow_record_and_announce() {
    let Some(db) = isolated("warranties").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let ws = types::list(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == "workstation")
        .unwrap()
        .id;
    let laptop = asset(&mut tx, root, ws, "Laptop", "active").await;
    let retired = asset(&mut tx, root, ws, "Old", "retired").await;

    // The asset's end date follows its latest warranty.
    let first = warranties::add(&mut tx, &ctx, laptop, &ends(30))
        .await
        .unwrap();
    assert_eq!(
        assets::get(&mut tx, laptop).await.unwrap().warranty_ends_on,
        Some(days(30))
    );
    let bad = WarrantyInput {
        starts_on: Some(days(40)),
        ..ends(30)
    };
    assert!(matches!(
        warranties::add(&mut tx, &ctx, laptop, &bad).await,
        Err(CmdbError::Invalid { .. })
    ));
    // A lookup records its warranty once, by reference.
    let ext = WarrantyInput {
        kind: Some("extended".into()),
        reference: Some("SVC-1".into()),
        ..ends(400)
    };
    warranties::record(&mut tx, &ctx, laptop, "dell", &ext)
        .await
        .unwrap();
    warranties::record(
        &mut tx,
        &ctx,
        laptop,
        "dell",
        &WarrantyInput {
            ends_on: days(500),
            ..ext.clone()
        },
    )
    .await
    .unwrap();
    let all = warranties::for_asset(&mut tx, laptop).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(
        (all[0].source.as_str(), all[0].ends_on),
        ("dell", days(500))
    );
    assert_eq!(
        assets::get(&mut tx, laptop).await.unwrap().warranty_ends_on,
        Some(days(500))
    );
    assert!(
        warranties::record(&mut tx, &ctx, laptop, "manual", &ext)
            .await
            .is_err()
    );

    announcements(&mut tx, laptop, retired, first.id).await;

    // Removing warranties: the end date follows what's left.
    for w in warranties::for_asset(&mut tx, laptop).await.unwrap() {
        warranties::delete(&mut tx, &ctx, w.id).await.unwrap();
    }
    assert_eq!(
        assets::get(&mut tx, laptop).await.unwrap().warranty_ends_on,
        None
    );
    tx.commit().await.unwrap();
}

async fn asset(tx: &mut sqlx::PgConnection, org: Uuid, ws: Uuid, name: &str, status: &str) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let input: AssetInput = serde_json::from_value(
        json!({ "orgId": org, "typeId": ws, "name": name, "status": status }),
    )
    .unwrap();
    assets::create(tx, &ctx, &input).await.unwrap().id
}

/// Ending within 60 days: announced once; retired assets never. Ended:
/// announced as expired, once. A new end date re-arms it.
async fn announcements(tx: &mut sqlx::PgConnection, laptop: Uuid, retired: Uuid, first: Uuid) {
    let ctx = RequestContext::system(Via::System);
    let tx = &mut *tx;
    // Ending within 60 days: announced once; retired assets never.
    warranties::add(tx, &ctx, retired, &ends(10)).await.unwrap();
    assert_eq!(warranties::sweep(tx).await.unwrap(), 1);
    assert_eq!(warranties::sweep(tx).await.unwrap(), 0);
    assert_eq!(events(tx, laptop, "asset.warranty_expiring").await, 1);
    assert_eq!(events(tx, retired, "asset.warranty_expiring").await, 0);
    // Ended: announced as expired, once. A new end date re-arms it.
    let expired = warranties::update(tx, &ctx, first, &ends(-1))
        .await
        .unwrap();
    assert_eq!(warranties::sweep(tx).await.unwrap(), 1);
    assert_eq!(events(tx, laptop, "asset.warranty_expired").await, 1);
    warranties::update(tx, &ctx, expired.id, &ends(20))
        .await
        .unwrap();
    assert_eq!(warranties::sweep(tx).await.unwrap(), 1);
    assert_eq!(events(tx, laptop, "asset.warranty_expiring").await, 2);
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE subject_id = $1 AND event_type = 'asset.warranty_expiring' ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(laptop)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        (payload["provider"].clone(), payload["name"].clone()),
        (json!("Dell"), json!("Laptop"))
    );
}
