//! The semantic data layer against a real PostgreSQL: every source's
//! fields compile against the schema, scopes and organizations narrow the
//! numbers, and results are cached.

use huxley_analytics::{Cache, QuerySpec, SOURCES, Scope, run};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::json;
use uuid::Uuid;

fn spec(v: serde_json::Value) -> QuerySpec {
    serde_json::from_value(v).unwrap()
}

async fn run_at(pools: &TestPools, org: Uuid, status: &str, hours_ago: i64) {
    sqlx::query(
        "INSERT INTO runs (id, org_id, document, status, trigger_type, created_at, started_at, finished_at) \
         VALUES ($1, $2, '{}', $3::run_status, 'manual', now() - make_interval(hours => $4::int), \
                 now() - make_interval(hours => $4::int), now() - make_interval(hours => $4::int) + interval '90 seconds')",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(status)
    .bind(i32::try_from(hours_ago).unwrap())
    .execute(&pools.owner)
    .await
    .unwrap();
}

#[tokio::test]
async fn every_field_of_every_source_compiles_and_runs() {
    let Some(db) = isolated("analytics_fields").await else {
        return;
    };
    let cache = Cache::default();
    for source in SOURCES {
        for field in source.fields.iter() {
            let mut dimension = json!({ "field": field.key });
            if matches!(
                field.kind,
                huxley_analytics::Kind::Date | huxley_analytics::Kind::Time
            ) {
                dimension["granularity"] = json!("month");
            }
            let q = spec(
                json!({ "source": source.key, "measures": [{ "fn": "count" }], "dimensions": [dimension] }),
            );
            run(&db.pools.app, &cache, &q, &Scope::All)
                .await
                .unwrap_or_else(|e| panic!("{}.{}: {e}", source.key, field.key));
        }
    }
}

#[tokio::test]
async fn scopes_and_organizations_narrow_the_numbers() {
    let Some(db) = isolated("analytics_scope").await else {
        return;
    };
    let pools = &db.pools;
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let acme = huxley_orgs::create(&mut tx, &ctx, root, huxley_orgs::OrgKind::Client, "Acme")
        .await
        .unwrap()
        .id;
    let globex = huxley_orgs::create(&mut tx, &ctx, root, huxley_orgs::OrgKind::Client, "Globex")
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    for (org, status, ago) in [
        (acme, "succeeded", 1),
        (acme, "succeeded", 2),
        (acme, "failed", 3),
        (acme, "succeeded", 24 * 60),
        (globex, "succeeded", 1),
    ] {
        run_at(pools, org, status, ago).await;
    }
    let cache = Cache::default();
    let by_status = spec(json!({
        "source": "runs", "measures": [{ "fn": "count" }, { "fn": "avg", "field": "duration_seconds" }],
        "dimensions": [{ "field": "status" }],
        "filters": [{ "field": "created_at", "op": "within", "value": "last_7_days" }],
    }));
    // Everyone, this week: 3 succeeded (Acme 2, Globex 1; an older Acme run is out), 1 failed.
    let all = run(&pools.app, &cache, &by_status, &Scope::All)
        .await
        .unwrap();
    assert_eq!(
        all.columns
            .iter()
            .map(|c| c.key.as_str())
            .collect::<Vec<_>>(),
        ["status", "count", "avg_duration_seconds"]
    );
    assert_eq!(
        all.rows,
        vec![
            vec![json!("succeeded"), json!(3), json!(90.0)],
            vec![json!("failed"), json!(1), json!(90.0)]
        ]
    );
    assert!(!all.cached);
    // Someone who sees only Globex.
    let globex_only = run(&pools.app, &cache, &by_status, &Scope::Orgs(vec![globex]))
        .await
        .unwrap();
    assert_eq!(
        globex_only.rows,
        vec![vec![json!("succeeded"), json!(1), json!(90.0)]]
    );
    // Asked again: from the cache, for the same scope only.
    assert!(
        run(&pools.app, &cache, &by_status, &Scope::All)
            .await
            .unwrap()
            .cached
    );
    // Narrowed to Acme by the spec.
    let mut acme_only = by_status.clone();
    acme_only.org_id = Some(acme);
    let rows = run(&pools.app, &cache, &acme_only, &Scope::All)
        .await
        .unwrap()
        .rows;
    assert_eq!(
        rows,
        vec![
            vec![json!("succeeded"), json!(2), json!(90.0)],
            vec![json!("failed"), json!(1), json!(90.0)]
        ]
    );

    // Time buckets and limits.
    let by_day = spec(json!({
        "source": "runs", "measures": [{ "fn": "count" }],
        "dimensions": [{ "field": "created_at", "granularity": "year" }], "limit": 1,
        "timezone": "Europe/Berlin",
    }));
    let answer = run(&pools.app, &cache, &by_day, &Scope::All).await.unwrap();
    assert_eq!(answer.rows.len(), 1);
    assert!(
        answer.rows[0][0].as_str().unwrap().ends_with("-01-01"),
        "{:?}",
        answer.rows
    );
}

#[tokio::test]
async fn bad_specs_say_what_is_wrong() {
    let Some(db) = isolated("analytics_bad").await else {
        return;
    };
    let cache = Cache::default();
    for (bad, at) in [
        (
            json!({ "source": "users", "measures": [{ "fn": "count" }] }),
            "source",
        ),
        (
            json!({ "source": "runs", "measures": [{ "fn": "sum", "field": "status" }] }),
            "measures/0/field",
        ),
        (
            json!({ "source": "runs", "dimensions": [{ "field": "status; DROP TABLE runs" }] }),
            "dimensions/0/field",
        ),
        (
            json!({ "source": "runs", "measures": [{ "fn": "count" }], "filters": [{ "field": "created_at", "op": "gt", "value": "soon" }] }),
            "filters/0/value",
        ),
        (
            json!({ "source": "runs", "measures": [{ "fn": "count" }], "filters": [{ "field": "status", "op": "gt", "value": "a" }] }),
            "filters/0/op",
        ),
        (
            json!({ "source": "runs", "measures": [{ "fn": "count" }], "timezone": "Mars/Olympus'" }),
            "timezone",
        ),
        (
            json!({ "source": "runs", "measures": [{ "fn": "count" }], "sort": [{ "by": "nope" }] }),
            "sort/0/by",
        ),
    ] {
        let err = run(&db.pools.app, &cache, &spec(bad.clone()), &Scope::All)
            .await
            .unwrap_err();
        match err {
            huxley_analytics::AnalyticsError::Invalid(where_, _) => assert_eq!(where_, at, "{bad}"),
            other => panic!("{bad}: {other}"),
        }
    }
    // Values are parameters, not SQL.
    let sneaky = spec(json!({
        "source": "runs", "measures": [{ "fn": "count" }],
        "filters": [{ "field": "workflow", "op": "contains", "value": "x' OR 1=1 --" }],
    }));
    let answer = run(&db.pools.app, &cache, &sneaky, &Scope::All)
        .await
        .unwrap();
    assert_eq!(answer.rows, vec![vec![json!(0)]]);
}
