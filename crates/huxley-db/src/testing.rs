//! Helpers for integration tests that need a real PostgreSQL.
//!
//! Tests call [`pools`], which returns `None` (skip) when
//! `HUXLEY_TEST_DATABASE_URL` is unset, unless `HUXLEY_REQUIRE_DB_TESTS` is set,
//! in which case it panics so a misconfigured CI job cannot silently skip.
//!
//! All tests share one database, so they must not assume it is empty: create
//! uniquely named data and use [`ensure_root`] for the single root org.

use secrecy::SecretString;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{DatabaseConfig, OrgScope, begin_scoped, connect, connect_owner, migrate};

/// Pools for a test: `owner` bypasses RLS (setup, tampering); `app` runs as
/// `huxley_app` like the server does.
#[derive(Debug, Clone)]
pub struct TestPools {
    pub owner: PgPool,
    pub app: PgPool,
}

pub async fn pools() -> Option<TestPools> {
    let Ok(url) = std::env::var("HUXLEY_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("HUXLEY_REQUIRE_DB_TESTS").is_none(),
            "HUXLEY_REQUIRE_DB_TESTS is set but HUXLEY_TEST_DATABASE_URL is not"
        );
        eprintln!("skipping: HUXLEY_TEST_DATABASE_URL is not set");
        return None;
    };
    let config = DatabaseConfig {
        url: SecretString::from(url),
        max_connections: 4,
        acquire_timeout_secs: 10,
        statement_timeout_ms: 10_000,
    };
    let owner = connect_owner(&config).expect("valid test database URL");
    migrate(&owner).await.expect("migrations apply");
    let app = connect(&config).expect("valid test database URL");
    Some(TestPools { owner, app })
}

/// A short unique suffix for test data names.
pub fn unique(prefix: &str) -> String {
    let id = Uuid::now_v7().simple().to_string();
    format!("{prefix} {}", &id[20..])
}

/// Returns the instance's root org, creating it if this is the first test to
/// need it. Safe to call from concurrent tests.
pub async fn ensure_root(pools: &TestPools) -> Uuid {
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    // Serialize creators so two tests don't race to insert the root.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('huxley_test_root'))")
        .execute(&mut *tx)
        .await
        .unwrap();
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM organizations WHERE parent_id IS NULL")
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    let id = if let Some((id,)) = existing {
        id
    } else {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO organizations (id, kind, name, path) VALUES ($1, 'provider', 'Test root', '')",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
        id
    };
    tx.commit().await.unwrap();
    id
}
