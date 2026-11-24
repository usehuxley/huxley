//! Datasets against a real PostgreSQL: managed tables, typed rows, schema
//! changes, and row-level security by organization.

use huxley_core::{RequestContext, Via};
use huxley_datasets::{Column, ColumnType, DatasetError, DatasetInput, Query};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::json;
use uuid::Uuid;

fn col(name: &str, kind: ColumnType) -> Column {
    Column {
        name: name.into(),
        kind,
    }
}

fn users() -> DatasetInput {
    DatasetInput {
        key: "m365-users".into(),
        name: "M365 users".into(),
        description: None,
        columns: vec![
            col("upn", ColumnType::Text),
            col("licensed", ColumnType::Boolean),
            col("seats", ColumnType::Integer),
        ],
        primary_key: vec!["upn".into()],
    }
}

/// A provider with a client below it.
async fn orgs(pools: &TestPools) -> (Uuid, Uuid) {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let msp = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Ds MSP"),
    )
    .await
    .unwrap()
    .id;
    let client = huxley_orgs::create(
        &mut tx,
        &ctx,
        msp,
        huxley_orgs::OrgKind::Client,
        &unique("Ds client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    (msp, client)
}

/// A users dataset at a provider, with rows for its client and itself.
async fn seeded(pools: &TestPools) -> (huxley_datasets::Dataset, Uuid, Uuid) {
    let (msp, client) = orgs(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let dataset = huxley_datasets::create(&mut tx, &ctx, msp, &users())
        .await
        .unwrap();
    let rows = vec![
        json!({ "upn": "ada@client.test", "licensed": true, "seats": 2 }),
        json!({ "upn": "bob@client.test", "licensed": false }),
    ];
    assert_eq!(
        huxley_datasets::upsert(&mut tx, &dataset, client, &rows)
            .await
            .unwrap(),
        2
    );
    huxley_datasets::upsert(
        &mut tx,
        &dataset,
        msp,
        &[json!({ "upn": "tech@msp.test", "licensed": true })],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (dataset, msp, client)
}

#[tokio::test]
async fn datasets_keep_typed_rows() {
    let Some(pools) = pools().await else { return };
    let (dataset, msp, client) = seeded(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    // Same key twice at one organization: refused.
    let again = huxley_datasets::create(&mut tx, &ctx, msp, &users())
        .await
        .unwrap_err();
    assert!(matches!(again, DatasetError::KeyTaken(_)), "{again:?}");
    drop(tx);

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    // Same key: updated, not added.
    huxley_datasets::upsert(
        &mut tx,
        &dataset,
        client,
        &[json!({ "upn": "bob@client.test", "licensed": true })],
    )
    .await
    .unwrap();
    // Values are typed by the table; unknown columns and missing keys are refused.
    for bad in [
        json!({ "upn": "x@y", "seats": "lots" }),
        json!({ "upn": "x@y", "nope": 1 }),
        json!({ "licensed": true }),
    ] {
        let mut sp = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
        let error = huxley_datasets::upsert(&mut sp, &dataset, client, std::slice::from_ref(&bad))
            .await
            .unwrap_err();
        assert!(
            matches!(error, DatasetError::Invalid(..)),
            "{bad}: {error:?}"
        );
    }
    tx.commit().await.unwrap();

    // Filters are typed too.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let licensed = Query {
        filters: [("licensed".into(), json!(true))].into(),
        ..Query::default()
    };
    let found = huxley_datasets::query(&mut tx, &dataset, &licensed)
        .await
        .unwrap();
    assert_eq!(found.total, 3);
    let mine = Query {
        org_id: Some(client),
        order_by: Some("-upn".into()),
        ..Query::default()
    };
    let found = huxley_datasets::query(&mut tx, &dataset, &mine)
        .await
        .unwrap();
    let upns: Vec<_> = found
        .rows
        .iter()
        .map(|r| r["upn"].as_str().unwrap())
        .collect();
    assert_eq!(upns, ["bob@client.test", "ada@client.test"]);
    assert_eq!(found.rows[1]["seats"], 2);
    assert_eq!(found.rows[0]["_org_id"], json!(client));
    let injected = Query {
        filters: [("upn\" OR 1=1 --".into(), json!(1))].into(),
        ..Query::default()
    };
    assert!(
        huxley_datasets::query(&mut tx, &dataset, &injected)
            .await
            .is_err()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn rows_are_limited_by_organization() {
    let Some(pools) = pools().await else { return };
    let (dataset, msp, client) = seeded(&pools).await;
    // The client's scope sees only its own rows (row-level security).
    let mut tx = begin_scoped(&pools.app, &OrgScope::Orgs(vec![client]))
        .await
        .unwrap();
    let seen = huxley_datasets::query(&mut tx, &dataset, &Query::default())
        .await
        .unwrap();
    assert_eq!(seen.total, 2);
    assert!(seen.rows.iter().all(|r| r["_org_id"] == json!(client)));
    // ...and can't write rows for the provider.
    assert!(
        huxley_datasets::upsert(&mut tx, &dataset, msp, &[json!({ "upn": "evil@x" })])
            .await
            .is_err()
    );
    drop(tx);

    // Rows must belong to the dataset's organization or below it.
    let (other, _) = orgs(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let elsewhere =
        huxley_datasets::upsert(&mut tx, &dataset, other, &[json!({ "upn": "x@y" })]).await;
    assert!(
        matches!(elsewhere, Err(DatasetError::Invalid(..))),
        "{elsewhere:?}"
    );
    drop(tx);

    // A full refresh replaces one organization's rows only.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_datasets::replace(
        &mut tx,
        &dataset,
        client,
        &[json!({ "upn": "new@client.test" })],
    )
    .await
    .unwrap();
    let all = huxley_datasets::query(&mut tx, &dataset, &Query::default())
        .await
        .unwrap();
    assert_eq!(all.total, 2);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn schema_changes_migrate_the_table() {
    let Some(pools) = pools().await else { return };
    let (msp, client) = orgs(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let dataset = huxley_datasets::create(&mut tx, &ctx, msp, &users())
        .await
        .unwrap();
    huxley_datasets::upsert(
        &mut tx,
        &dataset,
        client,
        &[json!({ "upn": "ada@x", "seats": 3, "licensed": true })],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Add a column, drop one, retype one (integer → text converts).
    let mut changed = users();
    changed.columns = vec![
        col("upn", ColumnType::Text),
        col("seats", ColumnType::Text),
        col("department", ColumnType::Text),
    ];
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let dataset = huxley_datasets::update(&mut tx, &ctx, dataset.id, &changed)
        .await
        .unwrap();
    let rows = huxley_datasets::query(&mut tx, &dataset, &Query::default())
        .await
        .unwrap()
        .rows;
    assert_eq!(rows[0]["seats"], "3");
    assert!(rows[0].get("licensed").is_none());
    assert_eq!(rows[0]["department"], json!(null));
    huxley_datasets::upsert(
        &mut tx,
        &dataset,
        client,
        &[json!({ "upn": "bob@x", "seats": "many" })],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Back to integer: "many" doesn't convert, so nothing changes.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let error = huxley_datasets::update(&mut tx, &ctx, dataset.id, &users())
        .await
        .unwrap_err();
    assert!(matches!(error, DatasetError::Invalid(..)), "{error:?}");
    drop(tx);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let still = huxley_datasets::get(&mut tx, dataset.id).await.unwrap();
    assert_eq!(still.columns, changed.columns);

    // Deleting drops the table.
    huxley_datasets::delete(&mut tx, &ctx, dataset.id)
        .await
        .unwrap();
    let exists: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT to_regclass('datasets.ds_{}')::text",
        dataset.id.simple()
    )))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(exists, None);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn the_database_rechecks_definitions() {
    let Some(pools) = pools().await else { return };
    let (msp, _) = orgs(&pools).await;
    // A definition written straight to the table, past the app's checks:
    // the DDL function still refuses it.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO datasets (id, org_id, key, name, columns) VALUES ($1, $2, 'evil', 'Evil', $3)",
    )
    .bind(id)
    .bind(msp)
    .bind(json!([{ "name": "x\" text); DROP TABLE users; --", "type": "text" }]))
    .execute(&mut *tx)
    .await
    .unwrap();
    let refused = sqlx::query("SELECT huxley_dataset_apply($1)")
        .bind(id)
        .execute(&mut *tx)
        .await;
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("invalid column name")
    );
    drop(tx);
    // And it only acts on datasets the caller's scope can see.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query(
        "INSERT INTO datasets (id, org_id, key, name, columns) VALUES ($1, $2, 'fine', 'Fine', $3)",
    )
    .bind(id)
    .bind(msp)
    .bind(json!([{ "name": "x", "type": "text" }]))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (stranger, _) = orgs(&pools).await;
    let mut tx = begin_scoped(&pools.app, &OrgScope::Orgs(vec![stranger]))
        .await
        .unwrap();
    let hidden = sqlx::query("SELECT huxley_dataset_apply($1)")
        .bind(id)
        .execute(&mut *tx)
        .await;
    assert!(hidden.unwrap_err().to_string().contains("no dataset"));
}
