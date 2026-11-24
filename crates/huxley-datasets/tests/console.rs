//! The SQL console against a real PostgreSQL, through a real login that
//! holds only the reader role: what it can read, and what it can't do.

use huxley_core::{RequestContext, Via};
use huxley_datasets::console;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::json;
use sqlx::PgPool;

/// A login named `name` (password `name`) with `roles`, as a pool.
async fn login(pools: &TestPools, name: &str, roles: &str) -> PgPool {
    // Tests run at once; creating the same role at once would race.
    static SETUP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _setup = SETUP.lock().await;
    let url = std::env::var("HUXLEY_TEST_DATABASE_URL").unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{name}') THEN \
         CREATE ROLE {name} LOGIN PASSWORD '{name}'; END IF; END $$"
    )))
    .execute(&pools.owner)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("GRANT {roles} TO {name}")))
        .execute(&pools.owner)
        .await
        .unwrap();
    let rest = url.split_once('@').unwrap().1;
    let url = format!("postgres://{name}:{name}@{rest}");
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap()
}

#[tokio::test]
async fn logins_that_could_do_more_are_refused() {
    let Some(pools) = pools().await else { return };
    let bad = login(
        &pools,
        "huxley_console_bad",
        "huxley_datasets_reader, huxley_app",
    )
    .await;
    let refused = console::check_login(&bad).await.unwrap_err();
    assert!(refused.contains("huxley_app"), "{refused}");
    let reader = login(&pools, "huxley_console_test", "huxley_datasets_reader").await;
    console::check_login(&reader).await.unwrap();
}

#[tokio::test]
async fn queries_are_bounded() {
    let Some(pools) = pools().await else { return };
    let reader = login(&pools, "huxley_console_test", "huxley_datasets_reader").await;
    let started = std::time::Instant::now();
    let slow = console::run(&reader, "SELECT pg_sleep(30)")
        .await
        .unwrap_err();
    assert!(slow.contains("statement timeout"), "{slow}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    let big = console::run(&reader, "SELECT g FROM generate_series(1, 5000) g")
        .await
        .unwrap();
    assert_eq!(big.rows.len(), console::MAX_ROWS);
    assert!(big.truncated);
}

#[tokio::test]
async fn the_console_reads_datasets_and_nothing_else() {
    let Some(pools) = pools().await else { return };
    let reader = login(&pools, "huxley_console_test", "huxley_datasets_reader").await;

    // A dataset with rows for two organizations.
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Console"),
    )
    .await
    .unwrap()
    .id;
    let client = huxley_orgs::create(
        &mut tx,
        &ctx,
        org,
        huxley_orgs::OrgKind::Client,
        &unique("Console c"),
    )
    .await
    .unwrap()
    .id;
    // Commit the organizations first: creating them takes the audit chain's
    // head, and a dataset's DDL lock taken after it can deadlock with another
    // transaction creating an organization (see `lock_for_ddl`).
    tx.commit().await.unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let input: huxley_datasets::DatasetInput = serde_json::from_value(json!({
        "key": "people", "name": "People",
        "columns": [{ "name": "upn", "type": "text" }, { "name": "seats", "type": "integer" }]
    }))
    .unwrap();
    let dataset = huxley_datasets::create(&mut tx, &ctx, org, &input)
        .await
        .unwrap();
    huxley_datasets::upsert(
        &mut tx,
        &dataset,
        org,
        &[json!({ "upn": "a@msp", "seats": 1 })],
    )
    .await
    .unwrap();
    huxley_datasets::upsert(
        &mut tx,
        &dataset,
        client,
        &[json!({ "upn": "b@client", "seats": 2 })],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let table = format!("ds_{}", dataset.id.simple());

    // Reads every organization's rows, in column order.
    let result = console::run(
        &reader,
        &format!("SELECT seats, upn FROM {table} ORDER BY seats;"),
    )
    .await
    .unwrap();
    assert_eq!(result.columns, ["seats", "upn"]);
    assert_eq!(
        result.rows,
        [
            json!({ "seats": 1, "upn": "a@msp" }),
            json!({ "seats": 2, "upn": "b@client" })
        ]
    );
    assert!(!result.truncated);

    // Nothing outside the datasets schema, nothing written, no escalation.
    for (sql, why) in [
        ("SELECT * FROM public.users", "permission denied"),
        ("SELECT * FROM public.datasets", "permission denied"),
        (&*format!("DELETE FROM {table}"), "syntax error"),
        (
            &*format!("WITH d AS (DELETE FROM {table} RETURNING *) SELECT * FROM d"),
            "WITH",
        ),
        (
            "SELECT set_config('role', 'huxley_app', true)",
            "permission denied",
        ),
        (
            "SELECT set_config('role', 'huxley', true)",
            "permission denied",
        ),
        ("SELECT 1; DROP TABLE public.users", ""),
        (&*format!("SELECT * FROM {table} FOR UPDATE"), ""),
    ] {
        let error = console::run(&reader, sql).await.unwrap_err();
        assert!(error.contains(why), "{sql}: {error}");
    }
    let still: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM datasets.{table}"
    )))
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert_eq!(still, 2);
}
