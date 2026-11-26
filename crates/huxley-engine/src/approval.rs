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
            let (approvers, portal) =
                match resolve_approvers(&mut tx, org_id, &input["approvers"]).await? {
                    Ok(both) => both,
                    Err(e) => return Ok(Executed::Failed(e)),
                };
            let id = huxley_core::new_id();
            sqlx::query(
                "INSERT INTO approvals (id, run_id, org_id, step_key, attempt, title, message, \
                        approvers, expires_at, portal_approvers) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
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
            .bind(&portal)
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
            self.email_portal_approvers(&mut tx, id, org_id, &title, body)
                .await?;
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
        self.decide(ctx, id, Decider::User(user_id), approve, comment)
            .await
    }

    /// Records a client contact's decision from the portal (their portal
    /// account must be one of the approval's portal approvers).
    pub async fn decide_portal_approval(
        &self,
        ctx: &RequestContext,
        id: Uuid,
        account: Uuid,
        approve: bool,
        comment: Option<&str>,
    ) -> Result<ApprovalView, DecisionError> {
        self.decide(ctx, id, Decider::Portal(account), approve, comment)
            .await
    }

    async fn decide(
        &self,
        ctx: &RequestContext,
        id: Uuid,
        decider: Decider,
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
            "SELECT run_id, org_id, step_key, attempt, status, expires_at, \
                    CASE WHEN $3 THEN $2 = ANY(approvers) ELSE $2 = ANY(portal_approvers) END \
                   FROM approvals WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .bind(decider.id())
        .bind(matches!(decider, Decider::User(_)))
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
        let by = decider.describe(&mut tx).await?;
        let now = OffsetDateTime::now_utc();
        let output = json!({
            "decision": decision,
            "by": by,
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
            "UPDATE approvals SET status = $2::approval_status, decided_by = $3, decided_by_portal = $5, \
                    decided_at = now(), comment = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(decision)
        .bind(match decider {
            Decider::User(user) => Some(user),
            Decider::Portal(_) => None,
        })
        .bind(comment)
        .bind(match decider {
            Decider::Portal(account) => Some(account),
            Decider::User(_) => None,
        })
        .execute(&mut *tx)
        .await?;
        queue::enqueue_advance(&mut tx, org_id, run_id).await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("workflow.approval_decided")
                .org(org_id)
                .target("approval", id)
                .details(json!({ "decision": decision, "run": run_id, "by": by })),
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

/// Who decides: a staff user, or a client contact's portal account.
#[derive(Debug, Clone, Copy)]
enum Decider {
    User(Uuid),
    Portal(Uuid),
}

impl Decider {
    fn id(self) -> Uuid {
        match self {
            Self::User(id) | Self::Portal(id) => id,
        }
    }

    /// The step output's `by`: `{id, name}` for staff; a contact adds
    /// `contact` and `portal_account` (`id` is the contact).
    async fn describe(self, conn: &mut sqlx::PgConnection) -> sqlx::Result<Json> {
        Ok(match self {
            Self::User(user) => {
                let name: Option<String> =
                    sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
                        .bind(user)
                        .fetch_optional(conn)
                        .await?;
                json!({ "id": user, "name": name })
            }
            Self::Portal(account) => {
                let row: Option<(Uuid, String, String)> = sqlx::query_as(
                    "SELECT c.id, btrim(c.first_name || ' ' || c.last_name), a.email \
                       FROM portal_accounts a JOIN crm_contacts c ON c.id = a.contact_id WHERE a.id = $1",
                )
                .bind(account)
                .fetch_optional(conn)
                .await?;
                let (contact, name, email) = row.unzip3();
                json!({ "id": contact, "name": name, "email": email, "contact": contact, "portal_account": account })
            }
        })
    }
}

trait Unzip3<A, B, C> {
    fn unzip3(self) -> (Option<A>, Option<B>, Option<C>);
}

impl<A, B, C> Unzip3<A, B, C> for Option<(A, B, C)> {
    fn unzip3(self) -> (Option<A>, Option<B>, Option<C>) {
        match self {
            Some((a, b, c)) => (Some(a), Some(b), Some(c)),
            None => (None, None, None),
        }
    }
}

/// The staff users and portal accounts an approval's `approvers` name at
/// the run's company; at least one of them.
async fn resolve_approvers(
    conn: &mut sqlx::PgConnection,
    org_id: Uuid,
    to: &Json,
) -> EngineResult<Result<(Vec<Uuid>, Vec<Uuid>), Json>> {
    let (staff, portal) = match split_approvers(&mut *conn, org_id, to).await? {
        Ok(split) => split,
        Err(e) => return Ok(Err(e)),
    };
    let users = if staff.is_empty() {
        Vec::new()
    } else {
        match crate::notify::recipients(conn, org_id, &Json::Array(staff)).await? {
            Ok(users) => users,
            Err(e) => return Ok(Err(e)),
        }
    };
    if users.is_empty() && portal.is_empty() {
        return Ok(Err(error(
            "definition",
            "nobody can approve: the approvers resolve to no one",
        )));
    }
    Ok(Ok((users, portal)))
}

/// Splits an approval's `approvers` into staff targets (`user:`, `team:`,
/// `role:`) and client contacts' portal accounts at the run's company:
/// `contact:<id>` (who must be able to approve) and `client_approvers`
/// (its client approvers and client admins). The inner error says what's
/// wrong with the list.
async fn split_approvers(
    conn: &mut sqlx::PgConnection,
    org_id: Uuid,
    to: &Json,
) -> EngineResult<Result<(Vec<Json>, Vec<Uuid>), Json>> {
    let entries: Vec<Json> = match to {
        Json::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    let mut staff = Vec::new();
    let mut portal: Vec<Uuid> = Vec::new();
    for entry in entries {
        let text = entry.as_str().map(str::trim).unwrap_or_default();
        if text == "client_approvers" {
            let found: Vec<Uuid> = sqlx::query_scalar(
                "SELECT id FROM portal_accounts WHERE org_id = $1 AND status IN ('invited', 'active') \
                    AND role IN ('client_approver', 'client_admin')",
            )
            .bind(org_id)
            .fetch_all(&mut *conn)
            .await?;
            portal.extend(
                found
                    .into_iter()
                    .filter(|a| !portal.contains(a))
                    .collect::<Vec<_>>(),
            );
        } else if let Some(contact) = text.strip_prefix("contact:") {
            let Ok(contact) = contact.parse::<Uuid>() else {
                return Ok(Err(error(
                    "definition",
                    format!("{text:?}: expected contact:<id>"),
                )));
            };
            let found: Option<(Uuid, String)> = sqlx::query_as(
                "SELECT id, role FROM portal_accounts WHERE contact_id = $1 AND org_id = $2 \
                    AND status IN ('invited', 'active')",
            )
            .bind(contact)
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await?;
            match found {
                None => {
                    return Ok(Err(error(
                        "definition",
                        format!("{text}: that contact has no portal access at this company"),
                    )));
                }
                Some((_, role)) if role == "client_user" => {
                    return Ok(Err(error(
                        "definition",
                        format!(
                            "{text}: that contact can't approve (give them the client approver role)"
                        ),
                    )));
                }
                Some((account, _)) if !portal.contains(&account) => portal.push(account),
                Some(_) => {}
            }
        } else {
            staff.push(entry);
        }
    }
    Ok(Ok((staff, portal)))
}

impl Engine {
    /// Emails each portal approver a link to the approval in the portal,
    /// when email and a portal address are set up.
    async fn email_portal_approvers(
        &self,
        conn: &mut sqlx::PgConnection,
        id: Uuid,
        org_id: Uuid,
        title: &str,
        body: &str,
    ) -> EngineResult<()> {
        let Some(mailer) = self.mailer.as_ref().filter(|m| m.enabled()) else {
            return Ok(());
        };
        let emails: Vec<String> = sqlx::query_scalar(
            "SELECT a.email FROM approvals ap JOIN portal_accounts a ON a.id = ANY (ap.portal_approvers) \
              WHERE ap.id = $1",
        )
        .bind(id)
        .fetch_all(&mut *conn)
        .await?;
        if emails.is_empty() {
            return Ok(());
        }
        let domain =
            huxley_orgs::settings::value(conn, org_id, huxley_orgs::settings::PORTAL_DOMAIN)
                .await
                .map_err(|e| EngineError::Document(e.to_string()))?;
        let base = match domain {
            Some(d) if d == "localhost" || d.starts_with("localhost:") => {
                Some(format!("http://{d}"))
            }
            Some(d) => Some(format!("https://{d}")),
            None => self.portal_url.clone(),
        };
        let Some(base) = base else {
            return Ok(());
        };
        let link = format!("{}/approvals/{id}", base.trim_end_matches('/'));
        let text = if body.trim().is_empty() {
            "Your approval is needed. Open it in the support portal to approve or reject it."
                .to_owned()
        } else {
            body.to_owned()
        };
        for to in emails {
            mailer.send_later(huxley_email::action_email(
                &to,
                &format!("Approval needed: {title}"),
                "Hello,",
                &text,
                "Review it",
                &link,
                "You're getting this because you approve requests for your organization.",
            ));
        }
        Ok(())
    }
}

/// An approval as a client contact sees it in the portal.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PortalApproval {
    pub id: Uuid,
    pub title: String,
    pub message: Option<String>,
    /// `pending`, `approved`, `rejected` or `expired`.
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub decided_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub comment: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const PORTAL_VIEW: &str = "SELECT ap.id, ap.title, ap.message, \
        CASE WHEN ap.status = 'pending' AND ap.expires_at <= now() THEN 'expired' ELSE ap.status::text END AS status, \
        ap.expires_at, COALESCE(u.display_name, nullif(btrim(c.first_name || ' ' || c.last_name), '')) AS decided_by_name, \
        ap.decided_at, ap.comment, ap.created_at \
      FROM approvals ap LEFT JOIN users u ON u.id = ap.decided_by \
      LEFT JOIN portal_accounts pa ON pa.id = ap.decided_by_portal LEFT JOIN crm_contacts c ON c.id = pa.contact_id \
     WHERE $1 = ANY (ap.portal_approvers)";

/// The approvals sent to a portal account: waiting ones first, then the
/// latest decided.
pub async fn portal_approvals(
    conn: &mut sqlx::PgConnection,
    account: Uuid,
) -> sqlx::Result<Vec<PortalApproval>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PORTAL_VIEW} ORDER BY (ap.status = 'pending' AND ap.expires_at > now()) DESC, ap.created_at DESC LIMIT 100"
    )))
    .bind(account)
    .fetch_all(conn)
    .await
}

/// One approval sent to a portal account.
pub async fn portal_approval(
    conn: &mut sqlx::PgConnection,
    account: Uuid,
    id: Uuid,
) -> sqlx::Result<Option<PortalApproval>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{PORTAL_VIEW} AND ap.id = $2")))
        .bind(account)
        .bind(id)
        .fetch_optional(conn)
        .await
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
