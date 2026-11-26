//! `core.asset.set_warranty`: a lookup's warranty recorded for an asset of
//! the run's organization (or below), never another's.

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{Engine, EngineConfig, RunStatus, StartRun};
use serde_json::{Value as Json, json};
use uuid::Uuid;

async fn run(
    engine: &Engine,
    pools: &TestPools,
    org: Uuid,
    asset: Uuid,
) -> (RunStatus, Option<Json>, Option<Json>) {
    let yaml = format!(
        "apiVersion: usehuxley.io/v1\nkind: Workflow\nmetadata: {{ key: t, name: T }}\nspec:\n  steps:\n    - id: w\n      type: core.asset.set_warranty\n      with:\n        asset_id: \"{asset}\"\n        source: dell\n        reference: SVC-9\n        provider: Dell\n        kind: extended\n        ends_on: \"2031-01-31\"\n  outputs:\n    w: \"{{{{ steps.w.output }}}}\"\n"
    );
    let document = serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: org,
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
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.drain("asset-steps", 50).await.unwrap();
        let row: (RunStatus, Option<Json>, Option<Json>) =
            sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                .bind(id)
                .fetch_one(&pools.owner)
                .await
                .unwrap();
        if row.0.is_finished() {
            return row;
        }
        assert!(Instant::now() < deadline, "run didn't finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn org(tx: &mut sqlx::PgConnection, parent: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, parent_id, kind, name, path) VALUES ($1, $2, 'client', $3, '')")
        .bind(id)
        .bind(parent)
        .bind(name)
        .execute(tx)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn lookups_record_warranties_within_their_organization() {
    let Some(db) = isolated("asset_steps").await else {
        return;
    };
    let pools = db.pools.clone();
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "crm", true, false)
        .await
        .unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "cmdb", true, false)
        .await
        .unwrap();
    let acme = org(&mut tx, root, "Acme").await;
    let other = org(&mut tx, root, "Other").await;
    let ws = huxley_cmdb::types::list(&mut tx)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.key == "workstation")
        .unwrap()
        .id;
    let mk = |org: Uuid| -> huxley_cmdb::assets::AssetInput {
        serde_json::from_value(json!({ "orgId": org, "typeId": ws, "name": "Laptop" })).unwrap()
    };
    let laptop = huxley_cmdb::assets::create(&mut tx, &ctx, &mk(acme))
        .await
        .unwrap()
        .id;
    let theirs = huxley_cmdb::assets::create(&mut tx, &ctx, &mk(other))
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::with_config(pools.app.clone(), crypto, &EngineConfig::default());

    let (status, outputs, error) = run(&engine, &pools, acme, laptop).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(outputs.unwrap()["w"]["endsOn"], "2031-01-31");
    // Again: the same warranty, updated rather than added.
    run(&engine, &pools, acme, laptop).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM cmdb_warranties WHERE asset_id = $1")
        .bind(laptop)
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(count, 1);
    // Another client's asset is out of reach.
    let (status, _, error) = run(&engine, &pools, acme, theirs).await;
    assert_eq!(status, RunStatus::Failed);
    assert!(error.unwrap().to_string().contains("no such asset"));
}
