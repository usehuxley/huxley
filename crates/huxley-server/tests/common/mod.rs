//! Shared by the process-level tests: a throwaway database and worker processes.

#![allow(dead_code)]

use std::process::{Child, Command, Stdio};

use sqlx::Connection as _;
use uuid::Uuid;

/// The fixed test KEK (`huxley_crypto::testing`).
pub const TEST_KEK: &str = "aHV4bGV5LXRlc3Qta2VrLW5vdC1mb3ItcmVhbC11c2U=";

pub struct Database {
    admin_url: String,
    name: String,
    pub url: String,
}

impl Database {
    pub async fn create(prefix: &str) -> Option<Self> {
        let Ok(admin_url) = std::env::var("HUXLEY_TEST_DATABASE_URL") else {
            assert!(
                std::env::var_os("HUXLEY_REQUIRE_DB_TESTS").is_none(),
                "HUXLEY_REQUIRE_DB_TESTS is set but HUXLEY_TEST_DATABASE_URL is not"
            );
            return None;
        };
        let name = format!("huxley_{prefix}_{}", Uuid::now_v7().simple());
        let (base, _) = admin_url.rsplit_once('/').expect("a database URL");
        let url = format!("{base}/{name}");
        let mut conn = sqlx::PgConnection::connect(&admin_url).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut conn)
            .await
            .unwrap();
        Some(Self {
            admin_url,
            name,
            url,
        })
    }

    pub async fn drop(self) {
        let mut conn = sqlx::PgConnection::connect(&self.admin_url).await.unwrap();
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        )))
        .execute(&mut conn)
        .await;
    }
}

/// Starts `huxley serve --roles worker` against `url`.
pub fn worker(url: &str, concurrency: usize) -> Child {
    Command::new(env!("CARGO_BIN_EXE_huxley"))
        .args(["serve", "--roles", "worker"])
        .env("HUXLEY_DATABASE__URL", url)
        .env("HUXLEY_CRYPTO__KEK", TEST_KEK)
        .env("HUXLEY_ENGINE__LEASE_SECS", "2")
        .env("HUXLEY_ENGINE__CONCURRENCY", concurrency.to_string())
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the huxley binary starts")
}
