//! Audit log (design doc §12).
//!
//! Records are written inside the caller's transaction, so an action and its
//! audit record commit or roll back together. The database assigns the
//! sequence number, timestamp, and hash chain in a trigger, so callers cannot
//! forge or reorder them.

use huxley_core::RequestContext;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::DbError;

/// One audit record.
#[derive(Debug, Clone)]
pub struct AuditRecord<'a> {
    /// Dotted action name, e.g. `org.created`, `auth.login_failed`.
    pub action: &'a str,
    /// The organization the action concerns, if any.
    pub org_id: Option<Uuid>,
    /// The entity acted on: (type, id).
    pub target: Option<(&'a str, Uuid)>,
    /// Action-specific details. For changes, use `{"before": ..., "after": ...}`.
    /// Never include secrets.
    pub details: serde_json::Value,
}

impl<'a> AuditRecord<'a> {
    pub fn new(action: &'a str) -> Self {
        Self {
            action,
            org_id: None,
            target: None,
            details: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    #[must_use]
    pub fn org(mut self, org_id: Uuid) -> Self {
        self.org_id = Some(org_id);
        self
    }

    #[must_use]
    pub fn target(mut self, kind: &'a str, id: Uuid) -> Self {
        self.target = Some((kind, id));
        self
    }

    #[must_use]
    pub fn details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }
}

/// Appends an audit record. Returns its id.
pub async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    record: AuditRecord<'_>,
) -> Result<Uuid, DbError> {
    let id = huxley_core::new_id();
    let (target_type, target_id) = record.target.unzip();
    // seq, prev_hash and hash are filled in when the record is sealed (ADR-086).
    sqlx::query(
        "INSERT INTO audit_events \
            (id, seq, actor_type, actor_id, via, ip, user_agent, request_id, \
             org_id, action, target_type, target_id, details, prev_hash, hash) \
         VALUES ($1, 0, $2, $3, $4, $5::inet, $6, $7, $8, $9, $10, $11, $12, '\\x', '\\x')",
    )
    .bind(id)
    .bind(ctx.actor.kind())
    .bind(ctx.actor.id())
    .bind(ctx.via.as_str())
    .bind(ctx.ip.map(|ip| ip.to_string()))
    .bind(ctx.user_agent_truncated())
    .bind(ctx.request_id.as_deref())
    .bind(record.org_id)
    .bind(record.action)
    .bind(target_type)
    .bind(target_id)
    .bind(&record.details)
    .execute(conn)
    .await?;
    Ok(id)
}

/// Result of verifying the audit hash chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainStatus {
    /// Every record and the chain head are consistent.
    Intact { records: i64 },
    /// The first record (by sequence) where the chain breaks.
    Broken { seq: i64, reason: &'static str },
}

const VERIFY_BATCH: i64 = 1000;

/// (seq, `prev_hash`, hash, recomputed hash)
type ChainRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>);

/// Seals committed audit records into the hash chain (ADR-086): oldest
/// first, until none are left or another sealer holds the chain. Returns how
/// many it sealed.
pub async fn seal(conn: &mut PgConnection) -> Result<u64, DbError> {
    let mut total = 0u64;
    loop {
        let n: i32 = sqlx::query_scalar("SELECT huxley_audit_seal(1000)")
            .fetch_one(&mut *conn)
            .await?;
        total += u64::try_from(n).unwrap_or(0);
        if n < 1000 {
            return Ok(total);
        }
    }
}

/// Recomputes the whole hash chain from one consistent snapshot, so audit
/// writes happening meanwhile don't look like tampering. Seals what's
/// waiting first. Use an owner pool: RLS would hide records from the
/// application role.
pub async fn verify_chain(pool: &PgPool) -> Result<ChainStatus, DbError> {
    {
        let mut conn = pool.acquire().await?;
        seal(&mut conn).await?;
    }
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let status = verify_chain_in(&mut tx).await;
    tx.rollback().await?;
    status
}

/// Verifies the chain on an existing connection or transaction. The caller is
/// responsible for snapshot consistency (see [`verify_chain`]).
pub async fn verify_chain_in(conn: &mut PgConnection) -> Result<ChainStatus, DbError> {
    let mut expected_prev: Vec<u8> = vec![0; 32];
    let mut expected_seq: i64 = 1;
    let mut last_seq: i64 = 0;

    loop {
        let rows: Vec<ChainRow> = sqlx::query_as(
            "SELECT e.seq, e.prev_hash, e.hash, huxley_audit_hash(e.prev_hash, e) \
               FROM audit_events e WHERE e.seq > $1 ORDER BY e.seq LIMIT $2",
        )
        .bind(last_seq)
        .bind(VERIFY_BATCH)
        .fetch_all(&mut *conn)
        .await?;

        if rows.is_empty() {
            break;
        }
        for (seq, prev_hash, hash, recomputed) in rows {
            if seq != expected_seq {
                return Ok(ChainStatus::Broken {
                    seq: expected_seq,
                    reason: "record missing from sequence",
                });
            }
            if prev_hash != expected_prev {
                return Ok(ChainStatus::Broken {
                    seq,
                    reason: "previous-hash link does not match",
                });
            }
            if hash != recomputed {
                return Ok(ChainStatus::Broken {
                    seq,
                    reason: "record content does not match its hash",
                });
            }
            expected_prev = hash;
            expected_seq += 1;
            last_seq = seq;
        }
    }

    let (head_seq, head_hash): (i64, Vec<u8>) =
        sqlx::query_as("SELECT seq, hash FROM audit_chain_head WHERE id = 1")
            .fetch_one(&mut *conn)
            .await?;
    if head_seq != last_seq || head_hash != expected_prev {
        return Ok(ChainStatus::Broken {
            seq: last_seq + 1,
            reason: "chain head does not match the last record (records deleted from the end?)",
        });
    }

    Ok(ChainStatus::Intact { records: last_seq })
}
