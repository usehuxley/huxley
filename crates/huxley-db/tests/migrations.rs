//! Runs against a real PostgreSQL when `HUXLEY_TEST_DATABASE_URL` is set (CI
//! provides one; locally, `just db-up` starts one). Skips otherwise.

use huxley_db::{DatabaseConfig, OrgScope, begin_scoped, connect, migrate, sqlx};
use secrecy::SecretString;
use uuid::Uuid;

fn test_pool() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("HUXLEY_TEST_DATABASE_URL") else {
        // CI sets this so a misconfigured job fails instead of silently skipping.
        assert!(
            std::env::var_os("HUXLEY_REQUIRE_DB_TESTS").is_none(),
            "HUXLEY_REQUIRE_DB_TESTS is set but HUXLEY_TEST_DATABASE_URL is not"
        );
        eprintln!("skipping: HUXLEY_TEST_DATABASE_URL is not set");
        return None;
    };
    let config = DatabaseConfig {
        url: SecretString::from(url),
        max_connections: 2,
        acquire_timeout_secs: 10,
        statement_timeout_ms: 10_000,
    };
    Some(connect(&config).expect("valid test database URL"))
}

async fn in_scope(pool: &sqlx::PgPool, scope: Option<&OrgScope>, org: Uuid) -> bool {
    let mut tx = match scope {
        Some(scope) => begin_scoped(pool, scope).await.unwrap(),
        None => pool.begin().await.unwrap(),
    };
    let (allowed,): (bool,) = sqlx::query_as("SELECT huxley_org_in_scope($1)")
        .bind(org)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    allowed
}

#[tokio::test]
async fn migrations_apply_idempotently_and_rls_denies_by_default() {
    let Some(pool) = test_pool() else { return };

    migrate(&pool).await.expect("migrations apply");
    migrate(&pool)
        .await
        .expect("re-running migrations is a no-op");

    let mine = Uuid::now_v7();
    let other = Uuid::now_v7();

    // No scope set: deny.
    assert!(!in_scope(&pool, None, mine).await);
    // Empty scope: deny.
    assert!(!in_scope(&pool, Some(&OrgScope::Orgs(vec![])), mine).await);
    // System scope: allow.
    assert!(in_scope(&pool, Some(&OrgScope::System), mine).await);
    // Explicit scope: only the listed orgs.
    let scope = OrgScope::Orgs(vec![mine]);
    assert!(in_scope(&pool, Some(&scope), mine).await);
    assert!(!in_scope(&pool, Some(&scope), other).await);
}

#[tokio::test]
async fn scope_does_not_leak_to_the_next_transaction() {
    let Some(pool) = test_pool() else { return };
    migrate(&pool).await.unwrap();

    let org = Uuid::now_v7();
    {
        let tx = begin_scoped(&pool, &OrgScope::System).await.unwrap();
        tx.commit().await.unwrap();
    }
    // The pool has at most two connections, so this very likely reuses the
    // same one. The setting was transaction-local, so it must be gone.
    for _ in 0..4 {
        assert!(!in_scope(&pool, None, org).await);
    }
}
