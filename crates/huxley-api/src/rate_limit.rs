//! Fixed-window rate limits keyed by client address, shared by every API
//! replica through PostgreSQL (ADR-090). Account lockout (also in the
//! database) still guards each account; these blunt password spraying and
//! abuse of public endpoints from one address.

use std::net::IpAddr;
use std::time::Duration;

use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct RateLimiter {
    db: PgPool,
    /// Kept apart from other limits: `login`, `public_forms`, `oauth`.
    scope: &'static str,
    limit: u32,
    window: Duration,
}

impl RateLimiter {
    pub fn new(db: PgPool, scope: &'static str, limit: u32, window: Duration) -> Self {
        Self {
            db,
            scope,
            limit,
            window,
        }
    }

    /// Records a hit. Returns `false` if the address is over the limit. If
    /// the database can't be reached the hit is let through (and logged):
    /// requests needing the database fail anyway.
    pub async fn check(&self, ip: IpAddr) -> bool {
        match self.hit(ip).await {
            Ok(hits) => u32::try_from(hits).unwrap_or(u32::MAX) <= self.limit,
            Err(e) => {
                tracing::warn!(error = %e, scope = self.scope, "rate limit check failed; allowing");
                true
            }
        }
    }

    async fn hit(&self, ip: IpAddr) -> sqlx::Result<i32> {
        sqlx::query_scalar(
            "INSERT INTO rate_limit_hits (key, window_start, hits) VALUES ($1, now(), 1) \
             ON CONFLICT (key) DO UPDATE SET \
               hits = CASE WHEN rate_limit_hits.window_start <= now() - make_interval(secs => $2) \
                           THEN 1 ELSE rate_limit_hits.hits + 1 END, \
               window_start = CASE WHEN rate_limit_hits.window_start <= now() - make_interval(secs => $2) \
                                   THEN now() ELSE rate_limit_hits.window_start END \
             RETURNING hits",
        )
        .bind(format!("{}:{ip}", self.scope))
        .bind(self.window.as_secs_f64())
        .fetch_one(&self.db)
        .await
    }
}
