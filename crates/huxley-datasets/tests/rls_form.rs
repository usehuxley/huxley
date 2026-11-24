//! Row-level security stays fast as organizations grow: every policy reads
//! the scope through subqueries Postgres evaluates once per query, never
//! `huxley_org_in_scope()` per row (quadratic in the size of the scope).

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::json;

#[tokio::test]
async fn no_policy_checks_the_scope_row_by_row() {
    let Some(pools) = pools().await else { return };
    // Including the policies of a dataset table made now.
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Rls"),
    )
    .await
    .unwrap()
    .id;
    // Commit the organizations first: creating them takes the audit chain's
    // head, and a dataset's DDL lock taken after it can deadlock with another
    // transaction creating an organization (see `lock_for_ddl`).
    tx.commit().await.unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let input: huxley_datasets::DatasetInput = serde_json::from_value(
        json!({ "key": "rls", "name": "RLS", "columns": [{ "name": "x", "type": "text" }] }),
    )
    .unwrap();
    let dataset = huxley_datasets::create(&mut tx, &ctx, org, &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let slow: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname || '.' || p.polname FROM pg_policy p JOIN pg_class c ON c.oid = p.polrelid \
          WHERE coalesce(pg_get_expr(p.polqual, p.polrelid), '') || coalesce(pg_get_expr(p.polwithcheck, p.polrelid), '') \
                LIKE '%huxley_org_in_scope(%'",
    )
    .fetch_all(&pools.owner)
    .await
    .unwrap();
    assert!(
        slow.is_empty(),
        "policies checking the scope row by row: {slow:?}"
    );
    let table = format!("ds_{}", dataset.id.simple());
    let fast: String = sqlx::query_scalar(
        "SELECT pg_get_expr(p.polqual, p.polrelid) FROM pg_policy p JOIN pg_class c ON c.oid = p.polrelid \
          WHERE c.relname = $1 AND p.polname = 'rows_in_scope'",
    )
    .bind(&table)
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    assert!(fast.contains("huxley_scope_ids"), "{fast}");
}
