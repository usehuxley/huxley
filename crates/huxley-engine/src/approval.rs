//! The `approval` step (design doc §6.2): parks until one of its approvers
//! approves or rejects. The approver list is resolved once, when the step
//! parks; being on it is what lets someone decide.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{OrgScope, begin_scoped};
use serde::Serialize;
use serde_json::{Value as Json, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::journal::Record;
use crate::step::Executed;
use crate::walk::error;
use crate::{Engine, EngineError, EngineResult, queue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "approval_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

/// An approval as approvers see it.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    pub id: Uuid,
    pub run_id: Uuid,
    pub org_id: Uuid,
    pub title: String,
    pub message: Option<String>,
    pub status: ApprovalStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub decided_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub comment: Option<String>,
    /// The workflow's name, if the run is of a saved workflow.
    pub workflow_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const VIEW: &str = "a.id, a.run_id, a.org_id, a.title, a.message, a.status, a.expires_at, \
                    a.decided_by, a.decided_at, a.comment, w.name AS workflow_name, a.created_at";
const FROM: &str = "approvals a JOIN runs r ON r.id = a.run_id \
                    LEFT JOIN workflows w ON w.id = r.workflow_id";

/// Why a decision wasn't recorded.
#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error("approval not found")]
    NotFound,
    #[error("only its approvers can decide")]
    NotAnApprover,
    #[error("it was already decided, or expired")]
    Closed,
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl From<sqlx::Error> for DecisionError {
    fn from(e: sqlx::Error) -> Self {
        Self::Engine(e.into())
    }
}

impl Engine {
    /// The step job of an `approval` step: records the approval and tells
    /// the approvers (once), then parks until a decision or the expiry.
    pub(crate) async fn park_approval(
        &self,
        run_id: Uuid,
        org_id: Uuid,
        record: &Record,
        input: &Json,
    ) -> EngineResult<Executed> {
        let Some(expires_at) = input["expires_at"]
            .as_str()
            .and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
        else {
            return Ok(Executed::Failed(error(
                "definition",
                "the approval has no expiry",
            )));
        };
        let title = input["title"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if title.is_empty() {
            return Ok(Executed::Failed(error(
                "definition",
                "an approval needs a title",
            )));
        }
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let existing: Option<(Uuid, ApprovalStatus, Option<OffsetDateTime>)> = sqlx::query_as(
            "SELECT id, status, notified_at FROM approvals \
              WHERE run_id = $1 AND step_key = $2 AND attempt = $3",
        )
        .bind(run_id)
        .bind(&record.step_key)
        .bind(record.attempt)
        .fetch_optional(&mut *tx)
        .await?;
        let (id, status, notified_at) = if let Some(row) = existing {
            row
        } else {
            let approvers =
                match crate::notify::recipients(&mut tx, org_id, &input["approvers"]).await? {
                    Ok(users) if users.is_empty() => {
                        return Ok(Executed::Failed(error(
                            "definition",
                            "nobody can approve: the approvers resolve to no one",
                        )));
                    }
                    Ok(users) => users,
                    Err(e) => return Ok(Executed::Failed(e)),
                };
            let id = huxley_core::new_id();
            sqlx::query(
                "INSERT INTO approvals (id, run_id, org_id, step_key, attempt, title, message, \
                        approvers, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(id)
            .bind(run_id)
            .bind(org_id)
            .bind(&record.step_key)
            .bind(record.attempt)
            .bind(&title)
            .bind(input["message"].as_str())
            .bind(&approvers)
            .bind(expires_at)
            .execute(&mut *tx)
            .await?;
            (id, ApprovalStatus::Pending, None)
        };
        if status != ApprovalStatus::Pending {
            // Decided meanwhile: the decision settled the step already.
            tx.commit().await?;
            return Ok(Executed::NotYet(OffsetDateTime::now_utc()));
        }
        if OffsetDateTime::now_utc() >= expires_at {
            sqlx::query("UPDATE approvals SET status = 'expired' WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(Executed::Failed(error(
                "timeout",
                "nobody decided before the approval expired",
            )));
        }
        if notified_at.is_none() {
            let body = input["message"].as_str().unwrap_or_default();
            notify_approvers(&mut tx, id, &title, body).await?;
            sqlx::query("UPDATE approvals SET notified_at = now() WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "UPDATE step_runs SET status = 'waiting' \
                  WHERE run_id = $1 AND step_key = $2 AND attempt = $3 AND status = 'running'",
            )
            .bind(run_id)
            .bind(&record.step_key)
            .bind(record.attempt)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Executed::NotYet(expires_at))
    }

    /// Records `user_id`'s decision on an approval, completes its step with
    /// `{decision, by, comment, decided_at}`, and moves the run on.
    pub async fn decide_approval(
        &self,
        ctx: &RequestContext,
        id: Uuid,
        user_id: Uuid,
        approve: bool,
        comment: Option<&str>,
    ) -> Result<ApprovalView, DecisionError> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(EngineError::from)?;
        let row: Option<(
            Uuid,
            Uuid,
            String,
            i32,
            ApprovalStatus,
            OffsetDateTime,
            bool,
        )> = sqlx::query_as(
            "SELECT run_id, org_id, step_key, attempt, status, expires_at, $2 = ANY(approvers) \
                   FROM approvals WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (run_id, org_id, step_key, attempt, status, expires_at, allowed) =
            row.ok_or(DecisionError::NotFound)?;
        if !allowed {
            return Err(DecisionError::NotAnApprover);
        }
        if status != ApprovalStatus::Pending || expires_at <= OffsetDateTime::now_utc() {
            return Err(DecisionError::Closed);
        }
        let comment = comment.map(str::trim).filter(|c| !c.is_empty());
        let decision = if approve { "approved" } else { "rejected" };
        let name: Option<String> =
            sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let now = OffsetDateTime::now_utc();
        let output = json!({
            "decision": decision,
            "by": { "id": user_id, "name": name },
            "comment": comment,
            "decided_at": now.format(&Rfc3339).unwrap_or_default(),
        });
        let updated = sqlx::query(
            "UPDATE step_runs SET status = 'succeeded', output = $4, finished_at = now() \
              WHERE run_id = $1 AND step_key = $2 AND attempt = $3 \
                AND status IN ('running', 'waiting')",
        )
        .bind(run_id)
        .bind(&step_key)
        .bind(attempt)
        .bind(&output)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            // The run was cancelled or the step settled another way.
            return Err(DecisionError::Closed);
        }
        sqlx::query(
            "UPDATE approvals SET status = $2::approval_status, decided_by = $3, \
                    decided_at = now(), comment = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(decision)
        .bind(user_id)
        .bind(comment)
        .execute(&mut *tx)
        .await?;
        queue::enqueue_advance(&mut tx, org_id, run_id).await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("workflow.approval_decided")
                .org(org_id)
                .target("approval", id)
                .details(json!({ "decision": decision, "run": run_id })),
        )
        .await
        .map_err(EngineError::from)?;
        let view = approval(&mut tx, id)
            .await?
            .ok_or(DecisionError::NotFound)?;
        tx.commit().await?;
        Ok(view)
    }
}

/// Tells each approver there's something to decide, with a link to it.
async fn notify_approvers(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    title: &str,
    body: &str,
) -> EngineResult<()> {
    let approvers: Vec<Uuid> =
        sqlx::query_scalar("SELECT unnest(approvers) FROM approvals WHERE id = $1")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    let link = format!("/approvals/{id}");
    for user_id in approvers {
        huxley_notify::notify(
            &mut *conn,
            huxley_notify::Notice {
                user_id,
                kind: huxley_notify::kinds::WORKFLOW,
                title: &format!("Approval needed: {title}"),
                body,
                link: Some(&link),
            },
        )
        .await
        .map_err(|e| EngineError::Document(e.to_string()))?;
    }
    Ok(())
}

/// One approval.
pub async fn approval(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
) -> sqlx::Result<Option<ApprovalView>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM {FROM} WHERE a.id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await
}

/// Whether `user_id` is one of an approval's approvers.
pub async fn is_approver(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    user_id: Uuid,
) -> sqlx::Result<bool> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT $2 = ANY(approvers) FROM approvals WHERE id = $1")
            .bind(id)
            .bind(user_id)
            .fetch_optional(conn)
            .await?
            .unwrap_or(false),
    )
}

/// Approvals `user_id` can decide now, plus the ones they decided lately.
pub async fn for_approver(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
) -> sqlx::Result<Vec<ApprovalView>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM {FROM} \
          WHERE $1 = ANY(a.approvers) \
            AND ((a.status = 'pending' AND a.expires_at > now() \
                  AND r.status IN ('queued', 'running')) \
                 OR a.decided_at > now() - interval '30 days') \
          ORDER BY (a.status = 'pending') DESC, a.created_at DESC LIMIT 200"
    )))
    .bind(user_id)
    .fetch_all(conn)
    .await
}
