//! Helpers for integration tests that need a real PostgreSQL.
//!
//! Tests call [`pools`], which returns `None` (skip) when
//! `HUXLEY_TEST_DATABASE_URL` is unset, unless `HUXLEY_REQUIRE_DB_TESTS` is set,
//! in which case it panics so a misconfigured CI job cannot silently skip.
//!
//! All tests share one database, so they must not assume it is empty: create
//! uniquely named data and use [`ensure_root`] for the single root org.
//!
//! Tests that run a job queue use [`isolated`] instead: test binaries run in
//! separate processes, and a process that exits while holding another test's
//! job leaves it leased until the lease runs out.

use secrecy::SecretString;
use sqlx::{Connection, PgPool};
use uuid::Uuid;

use crate::{DatabaseConfig, OrgScope, begin_scoped, connect, connect_owner, migrate};

/// Pools for a test: `owner` bypasses RLS (setup, tampering); `app` runs as
/// `huxley_app` like the server does.
#[derive(Debug, Clone)]
pub struct TestPools {
    pub owner: PgPool,
    pub app: PgPool,
}

/// The test server's URL, or `None` to skip the test.
fn admin_url() -> Option<String> {
    let Ok(url) = std::env::var("HUXLEY_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("HUXLEY_REQUIRE_DB_TESTS").is_none(),
            "HUXLEY_REQUIRE_DB_TESTS is set but HUXLEY_TEST_DATABASE_URL is not"
        );
        eprintln!("skipping: HUXLEY_TEST_DATABASE_URL is not set");
        return None;
    };
    Some(url)
}

async fn migrated(url: String) -> TestPools {
    let config = DatabaseConfig {
        url: SecretString::from(url),
        max_connections: 4,
        acquire_timeout_secs: 10,
        statement_timeout_ms: 10_000,
        read_url: None,
        direct_url: None,
        pooled: false,
    };
    let owner = connect_owner(&config).expect("valid test database URL");
    migrate(&owner).await.expect("migrations apply");
    let app = connect(&config).expect("valid test database URL");
    TestPools { owner, app }
}

pub async fn pools() -> Option<TestPools> {
    Some(migrated(admin_url()?).await)
}

/// A migrated database of the test's own, dropped when this is dropped.
/// Needs a `HUXLEY_TEST_DATABASE_URL` role that may create databases.
#[derive(Debug)]
pub struct IsolatedDb {
    pub pools: TestPools,
    admin_url: String,
    name: String,
}

/// Creates a database named `huxley_<prefix>_<uuid>` next to the test
/// database, or returns `None` to skip (see [`pools`]).
pub async fn isolated(prefix: &str) -> Option<IsolatedDb> {
    let admin_url = admin_url()?;
    let name = format!("huxley_{prefix}_{}", Uuid::now_v7().simple());
    let mut conn = sqlx::PgConnection::connect(&admin_url)
        .await
        .expect("test database reachable");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&mut conn)
        .await
        .expect("test role may create databases");
    let (base, _) = admin_url.rsplit_once('/').expect("a database URL");
    let pools = migrated(format!("{base}/{name}")).await;
    Some(IsolatedDb {
        pools,
        admin_url,
        name,
    })
}

impl Drop for IsolatedDb {
    fn drop(&mut self) {
        // Best effort, on a fresh runtime: the test's runtime may be gone.
        let admin = self.admin_url.clone();
        let name = self.name.clone();
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                if let Ok(mut conn) = sqlx::PgConnection::connect(&admin).await {
                    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                        "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
                    )))
                    .execute(&mut conn)
                    .await;
                }
            });
        })
        .join();
    }
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
