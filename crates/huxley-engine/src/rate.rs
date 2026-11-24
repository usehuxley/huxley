//! Rate limits from integration overlays: a fixed window of calls per
//! connection, counted in the database so every worker shares it.

use huxley_db::{OrgScope, begin_scoped};
use huxley_integrations::overlay::RateLimit;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::EngineResult;

/// Counts a call against `connection_id`'s window. `Err(at)`: over the
/// limit until `at`.
pub(crate) async fn acquire(
    db: &sqlx::PgPool,
    connection_id: Uuid,
    limit: RateLimit,
) -> EngineResult<Result<(), OffsetDateTime>> {
    let mut tx = begin_scoped(db, &OrgScope::System).await?;
    let per = f64::from(limit.per_seconds);
    let (used, window_start): (i32, OffsetDateTime) = sqlx::query_as(
        "INSERT INTO connection_rate_windows (connection_id, window_start, used) \
         VALUES ($1, to_timestamp(floor(extract(epoch FROM now()) / $2) * $2), 1) \
         ON CONFLICT (connection_id, window_start) \
         DO UPDATE SET used = connection_rate_windows.used + 1 \
         RETURNING used, window_start",
    )
    .bind(connection_id)
    .bind(per)
    .fetch_one(&mut *tx)
    .await?;
    // Old windows are done with.
    sqlx::query(
        "DELETE FROM connection_rate_windows \
          WHERE connection_id = $1 AND window_start < now() - interval '1 day'",
    )
    .bind(connection_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if u32::try_from(used).unwrap_or(u32::MAX) > limit.requests {
        Ok(Err(
            window_start + time::Duration::seconds(i64::from(limit.per_seconds))
        ))
    } else {
        Ok(Ok(()))
    }
}
