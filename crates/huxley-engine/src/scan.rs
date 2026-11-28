//! The upload scan loop (ADR-094). Any number of scheduler processes may run
//! it: each claims a due file with `FOR UPDATE SKIP LOCKED` and leases it
//! (pushing its next attempt out) before scanning, so a file is scanned by
//! one process at a time, and again if that process dies mid-scan.

use std::time::Duration;

use huxley_core::{RequestContext, Via};
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_db::{OrgScope, begin_scoped};
use huxley_scan::Verdict;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{Engine, EngineError, EngineResult};

/// Tries before a file is marked `failed` (an admin can scan it again).
pub const MAX_ATTEMPTS: i32 = 10;
/// Files scanned per pass, so one pass doesn't run forever.
const BATCH: usize = 50;

#[derive(Debug, sqlx::FromRow)]
struct Due {
    storage_key: String,
    org_id: Uuid,
    source: String,
    record_id: Uuid,
    filename: String,
    attempts: i32,
}

/// Seconds before trying a failed scan again: 30 s doubling, at most an hour.
fn backoff(attempts: i32) -> i64 {
    let exponent = u32::try_from(attempts.clamp(1, 8) - 1).unwrap_or(0);
    (30_i64 << exponent).min(3600)
}

impl Engine {
    /// Scans the files due, up to a batch. Returns how many were scanned
    /// (or given up on). Nothing to do without a scanner.
    pub async fn scan_uploads(&self) -> EngineResult<usize> {
        let (Some(scanner), Some(storage)) = (self.scanner.as_deref(), self.files.as_ref()) else {
            return Ok(0);
        };
        // Long enough for the slowest scan (MetaDefender's timeout is five
        // minutes by default), so a live scan isn't claimed twice.
        let lease_secs: i64 = 15 * 60;
        let mut done = 0;
        while done < BATCH {
            let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
            let due: Option<Due> = sqlx::query_as(
                "UPDATE file_scans SET next_attempt_at = now() + $1::bigint * interval '1 second', attempts = attempts + 1 \
                  WHERE storage_key = (SELECT storage_key FROM file_scans \
                                        WHERE status = 'pending' AND next_attempt_at <= now() \
                                        ORDER BY next_attempt_at LIMIT 1 FOR UPDATE SKIP LOCKED) \
                 RETURNING storage_key, org_id, source, record_id, filename, attempts",
            )
            .bind(lease_secs)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            let Some(due) = due else { break };
            done += 1;

            let data = match storage.get(&due.storage_key).await {
                Ok(data) => data,
                Err(huxley_storage::StorageError::NotFound) => {
                    // Deleted (or replaced) before it was scanned.
                    let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
                    sqlx::query("DELETE FROM file_scans WHERE storage_key = $1")
                        .bind(&due.storage_key)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;
                    continue;
                }
                Err(e) => {
                    self.scan_failed(&due, &format!("couldn't read the file: {e}"))
                        .await?;
                    continue;
                }
            };
            match scanner.scan(&data, &due.filename).await {
                Ok(verdict) => self.scan_finished(&due, scanner.name(), verdict).await?,
                Err(e) => self.scan_failed(&due, &e.0).await?,
            }
        }
        Ok(done)
    }

    async fn scan_finished(&self, due: &Due, engine: &str, verdict: Verdict) -> EngineResult<()> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let (status, finding) = match &verdict {
            Verdict::Clean => ("clean", None),
            Verdict::Infected(finding) => (
                "infected",
                Some(finding.chars().take(1000).collect::<String>()),
            ),
        };
        sqlx::query(
            "UPDATE file_scans SET status = $2, engine = $3, finding = $4, last_error = NULL, scanned_at = now() \
              WHERE storage_key = $1",
        )
        .bind(&due.storage_key)
        .bind(status)
        .bind(engine)
        .bind(&finding)
        .execute(&mut *tx)
        .await?;
        if let Some(finding) = finding {
            tracing::warn!(source = %due.source, record = %due.record_id, %finding, "upload quarantined by the malware scan");
            let ctx = RequestContext::system(Via::System);
            let details = json!({
                "source": due.source, "record": due.record_id, "filename": due.filename,
                "finding": finding, "engine": engine,
            });
            audit::record(
                &mut tx,
                &ctx,
                AuditRecord::new("file.quarantined")
                    .org(due.org_id)
                    .target(&due.source, due.record_id)
                    .details(details.clone()),
            )
            .await
            .map_err(EngineError::from)?;
            outbox::emit(
                &mut tx,
                DomainEvent {
                    event_type: "file.quarantined",
                    org_id: Some(due.org_id),
                    subject_type: &due.source,
                    subject_id: due.record_id,
                    payload: details,
                },
            )
            .await
            .map_err(EngineError::from)?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn scan_failed(&self, due: &Due, error: &str) -> EngineResult<()> {
        let error: String = error.chars().take(1000).collect();
        let give_up = due.attempts >= MAX_ATTEMPTS;
        tracing::warn!(source = %due.source, record = %due.record_id, attempts = due.attempts, %error, give_up, "upload scan failed");
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        sqlx::query(
            "UPDATE file_scans SET last_error = $2, \
                status = CASE WHEN $3 THEN 'failed' ELSE status END, \
                next_attempt_at = now() + $4::bigint * interval '1 second' \
              WHERE storage_key = $1",
        )
        .bind(&due.storage_key)
        .bind(&error)
        .bind(give_up)
        .bind(backoff(due.attempts))
        .execute(&mut *tx)
        .await?;
        if give_up {
            let ctx = RequestContext::system(Via::System);
            audit::record(
                &mut tx,
                &ctx,
                AuditRecord::new("file.scan_failed")
                    .org(due.org_id)
                    .target(&due.source, due.record_id)
                    .details(json!({ "filename": due.filename, "error": error, "attempts": due.attempts })),
            )
            .await
            .map_err(EngineError::from)?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Scans uploads every couple of seconds until `shutdown`.
pub async fn run_scanner(engine: Engine, shutdown: CancellationToken) {
    if engine.scanner.is_none() {
        return;
    }
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
        if let Err(e) = engine.scan_uploads().await {
            tracing::warn!(error = %e, "scanning uploads failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_an_hour() {
        assert_eq!(backoff(1), 30);
        assert_eq!(backoff(2), 60);
        assert_eq!(backoff(4), 240);
        assert_eq!(backoff(8), 3600);
        assert_eq!(backoff(50), 3600);
    }
}
