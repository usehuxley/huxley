//! Agreement renewals (design doc §8.7). Once per term, `renewal_notice_days`
//! before an active agreement ends, `contract.renewal_due` is announced, so
//! workflows can remind account managers or open a renewal opportunity.
//! Agreements that renew automatically roll on to a term of the same length
//! once they end, announcing `contract.renewed`.

use huxley_core::{RequestContext, Via};
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde_json::json;
use sqlx::PgConnection;
use time::Date;
use uuid::Uuid;

use crate::{ContractsError, ContractsResult};

#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    org_id: Uuid,
    name: String,
    kind: String,
    ends_on: Date,
    auto_renew: bool,
    days_left: i32,
}

#[derive(sqlx::FromRow)]
struct Renewed {
    id: Uuid,
    org_id: Uuid,
    name: String,
    starts_on: Date,
    ends_on: Date,
}

/// What a sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub announced: usize,
    pub renewed: usize,
}

/// Announces renewals coming up and renews agreements that renew
/// automatically and have ended (system scope). Each is claimed once, so
/// several processes can sweep.
pub async fn sweep(conn: &mut PgConnection, today: Date) -> ContractsResult<Swept> {
    let mut swept = Swept::default();
    // Automatic renewals first: a term that rolled on isn't "due".
    let renewed: Vec<Renewed> = sqlx::query_as(
        "UPDATE contract_agreements \
            SET starts_on = ends_on + 1, \
                ends_on = ends_on + (ends_on - starts_on + 1), \
                renewal_announced_for = NULL, version = version + 1, updated_at = now() \
          WHERE status = 'active' AND auto_renew AND ends_on IS NOT NULL AND ends_on < $1 \
          RETURNING id, org_id, name, starts_on, ends_on",
    )
    .bind(today)
    .fetch_all(&mut *conn)
    .await?;
    let ctx = RequestContext::system(Via::System);
    for r in renewed {
        audit::record(
            &mut *conn,
            &ctx,
            AuditRecord::new("agreement.renewed")
                .org(r.org_id)
                .target("agreement", r.id)
                .details(
                    json!({ "startsOn": r.starts_on.to_string(), "endsOn": r.ends_on.to_string() }),
                ),
        )
        .await?;
        outbox::emit(
            &mut *conn,
            DomainEvent {
                event_type: "contract.renewed",
                org_id: Some(r.org_id),
                subject_type: "agreement",
                subject_id: r.id,
                payload: json!({
                    "agreement_id": r.id,
                    "name": r.name,
                    "starts_on": r.starts_on.to_string(),
                    "ends_on": r.ends_on.to_string(),
                }),
            },
        )
        .await?;
        swept.renewed += 1;
    }
    let due: Vec<Due> = sqlx::query_as(
        "UPDATE contract_agreements SET renewal_announced_for = ends_on \
          WHERE status = 'active' AND ends_on IS NOT NULL AND ends_on >= $1 \
            AND ends_on - renewal_notice_days <= $1 \
            AND renewal_announced_for IS DISTINCT FROM ends_on \
          RETURNING id, org_id, name, kind, ends_on, auto_renew, (ends_on - $1)::int AS days_left",
    )
    .bind(today)
    .fetch_all(&mut *conn)
    .await?;
    for d in due {
        outbox::emit(
            &mut *conn,
            DomainEvent {
                event_type: "contract.renewal_due",
                org_id: Some(d.org_id),
                subject_type: "agreement",
                subject_id: d.id,
                payload: json!({
                    "agreement_id": d.id,
                    "name": d.name,
                    "kind": d.kind,
                    "ends_on": d.ends_on.to_string(),
                    "days_left": d.days_left,
                    "auto_renew": d.auto_renew,
                }),
            },
        )
        .await?;
        swept.announced += 1;
    }
    Ok(swept)
}

/// How often agreements are looked at.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Sweeps renewals until shutdown, while the module is on.
pub async fn run_sweeper(db: sqlx::PgPool, shutdown: tokio_util::sync::CancellationToken) {
    loop {
        if let Err(e) = sweep_once(&db).await {
            tracing::error!(error = %e, "agreement renewal sweep failed");
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(SWEEP_EVERY) => {}
        }
    }
}

async fn sweep_once(db: &sqlx::PgPool) -> ContractsResult<()> {
    let mut tx = huxley_db::begin_scoped(db, &huxley_db::OrgScope::System).await?;
    if huxley_modules::is_active(&mut tx, crate::MODULE)
        .await
        .map_err(|e| ContractsError::Sqlx(sqlx::Error::Protocol(e.to_string())))?
    {
        let today = time::OffsetDateTime::now_utc().date();
        let swept = sweep(&mut tx, today).await?;
        if swept != Swept::default() {
            tracing::info!(
                announced = swept.announced,
                renewed = swept.renewed,
                "agreement renewals"
            );
        }
    }
    tx.commit().await?;
    Ok(())
}
