//! Uploads waiting for, or past, a malware scan (ADR-094). Upload code
//! queues a file here when scanning is on; the scheduler's scan loop
//! (`huxley_engine::scan`) checks it; downloads ask [`status`] first.

use sqlx::PgConnection;
use uuid::Uuid;

use crate::DbError;

/// What a scanned file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Attachment,
    Receipt,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attachment => "attachment",
            Self::Receipt => "receipt",
        }
    }
}

/// A new upload to scan.
#[derive(Debug, Clone)]
pub struct NewScan<'a> {
    pub storage_key: &'a str,
    pub org_id: Uuid,
    pub source: Source,
    pub record_id: Uuid,
    pub filename: &'a str,
    pub size_bytes: i64,
}

/// Queues a file for scanning (in the upload's transaction).
pub async fn queue(conn: &mut PgConnection, scan: NewScan<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO file_scans (id, storage_key, org_id, source, record_id, filename, size_bytes) \
         VALUES ($7, $1, $2, $3, $4, $5, $6) ON CONFLICT (storage_key) DO NOTHING",
    )
    .bind(scan.storage_key)
    .bind(scan.org_id)
    .bind(scan.source.as_str())
    .bind(scan.record_id)
    .bind(scan.filename)
    .bind(scan.size_bytes)
    .bind(huxley_core::new_id())
    .execute(conn)
    .await?;
    Ok(())
}

/// Where a file stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Uploaded with scanning off: never scanned.
    Unscanned,
    Pending,
    Clean,
    /// Quarantined: the finding.
    Infected(String),
    /// The scanner couldn't check it after repeated tries.
    Failed(String),
}

impl Status {
    /// The API's word for it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unscanned => "unscanned",
            Self::Pending => "pending",
            Self::Clean => "clean",
            Self::Infected(_) => "infected",
            Self::Failed(_) => "failed",
        }
    }

    /// Whether the file may be served. Infected files never are; pending
    /// and failed ones only when `hold` is off.
    pub fn servable(&self, hold: bool) -> bool {
        match self {
            Self::Unscanned | Self::Clean => true,
            Self::Infected(_) => false,
            Self::Pending | Self::Failed(_) => !hold,
        }
    }

    pub fn from_row(status: &str, finding: Option<String>, last_error: Option<String>) -> Self {
        match status {
            "clean" => Self::Clean,
            "infected" => Self::Infected(finding.unwrap_or_default()),
            "failed" => Self::Failed(last_error.unwrap_or_default()),
            _ => Self::Pending,
        }
    }
}

/// The scan status of the file stored at `storage_key`.
pub async fn status(conn: &mut PgConnection, storage_key: &str) -> Result<Status, DbError> {
    let row: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT status, finding, last_error FROM file_scans WHERE storage_key = $1")
            .bind(storage_key)
            .fetch_optional(conn)
            .await?;
    Ok(row.map_or(Status::Unscanned, |(s, f, e)| Status::from_row(&s, f, e)))
}

/// A scanned upload, for the admin page.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileScan {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    /// `attachment` or `receipt`.
    pub source: String,
    pub record_id: Uuid,
    pub filename: String,
    pub size_bytes: i64,
    /// `pending`, `clean`, `infected` or `failed`.
    pub status: String,
    pub engine: Option<String>,
    pub finding: Option<String>,
    pub attempts: i32,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub scanned_at: Option<time::OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: time::OffsetDateTime,
    #[serde(skip)]
    pub storage_key: String,
}

const SELECT: &str = "SELECT s.id, s.org_id, o.name AS org_name, s.source, s.record_id, s.filename, s.size_bytes, \
    s.status, s.engine, s.finding, s.attempts, s.last_error, s.scanned_at, s.created_at, s.storage_key \
    FROM file_scans s JOIN organizations o ON o.id = s.org_id";

/// Uploads in a status (newest first), up to `limit`.
pub async fn list(
    conn: &mut PgConnection,
    status: &str,
    limit: i64,
) -> Result<Vec<FileScan>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE s.status = $1 ORDER BY s.created_at DESC LIMIT $2"
    )))
    .bind(status)
    .bind(limit.clamp(1, 500))
    .fetch_all(conn)
    .await?)
}

/// How many uploads are in each status.
pub async fn counts(conn: &mut PgConnection) -> Result<Vec<(String, i64)>, DbError> {
    Ok(
        sqlx::query_as("SELECT status, count(*) FROM file_scans GROUP BY status")
            .fetch_all(conn)
            .await?,
    )
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> Result<Option<FileScan>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE s.id = $1")))
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Queues a file to be scanned again, from scratch.
pub async fn rescan(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE file_scans SET status = 'pending', attempts = 0, finding = NULL, last_error = NULL, \
            engine = NULL, scanned_at = NULL, next_attempt_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Forgets a file's scan (its file was deleted).
pub async fn forget(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM file_scans WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_may_be_served() {
        let infected = Status::Infected("Eicar".into());
        assert!(!infected.servable(false) && !infected.servable(true));
        assert!(Status::Pending.servable(false) && !Status::Pending.servable(true));
        assert!(!Status::Failed(String::new()).servable(true));
        assert!(Status::Clean.servable(true) && Status::Unscanned.servable(true));
    }
}
