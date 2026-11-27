//! Expense reports (ADR-081): a person bundles their entries and submits
//! them for approval.
//!
//! - **Submitting** takes the person's drafts (or entries sent back), all
//!   under one MSP, and raises `expense_report.submitted` for the approval
//!   workflow (the built-in Expense approval pack), naming the person's
//!   team leads as reviewers (administrators if they have none) and the
//!   policy flags.
//! - **Deciding** ([`decide`], the `core.expense.report_decide` step):
//!   approved locks the entries; changes requested or rejected sends them
//!   back, to change and submit again (or delete). The person is told.
//! - **Comments** on the report or one of its entries, with `@mentions`.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_notify::{Notice, kinds};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::entries::{self, Expense};
use crate::{ExpenseError, ExpenseResult, required};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = ExpenseReportStatus)]
pub enum ReportStatus {
    Submitted,
    Approved,
    ChangesRequested,
    Rejected,
    Reimbursed,
}

/// A total in one currency.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Total {
    pub currency: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub amount: Decimal,
    /// What's to be paid back.
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub reimbursable: Decimal,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ExpenseReport)]
pub struct Report {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub user_name: String,
    pub number: i64,
    pub title: String,
    pub status: ReportStatus,
    pub reviewers: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub submitted_at: OffsetDateTime,
    pub decided_by: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub comment: Option<String>,
    pub version: i32,
    #[sqlx(skip)]
    pub entries: Vec<Expense>,
    #[sqlx(skip)]
    pub totals: Vec<Total>,
    /// Policy flags across its entries, counted: `{ over_limit: 1, … }`.
    #[sqlx(skip)]
    #[schema(value_type = std::collections::HashMap<String, i64>)]
    pub flags: BTreeMap<String, i64>,
}

const SELECT: &str = "SELECT r.id, r.org_id, r.user_id, u.display_name AS user_name, r.number, r.title, r.status, \
        r.reviewers, r.submitted_at, r.decided_by, r.decided_at, r.comment, r.version \
   FROM exp_reports r JOIN users u ON u.id = r.user_id";

fn totals(entries: &[Expense]) -> Vec<Total> {
    let mut by: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
    for e in entries {
        let t = by.entry(e.currency.clone()).or_default();
        t.0 += e.amount;
        if e.reimbursable {
            t.1 += e.amount;
        }
    }
    by.into_iter()
        .map(|(currency, (amount, reimbursable))| Total {
            currency,
            amount,
            reimbursable,
        })
        .collect()
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> ExpenseResult<Report> {
    let mut r: Report = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE r.id = $1")))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(ExpenseError::NotFound)?;
    r.entries = entries::in_report(&mut *conn, id).await?;
    r.totals = totals(&r.entries);
    for f in r.entries.iter().flat_map(|e| e.flags.iter()) {
        *r.flags.entry(f.clone()).or_default() += 1;
    }
    Ok(r)
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ReportFilter {
    pub user_id: Option<Uuid>,
    pub status: Option<ReportStatus>,
}

/// Reports, newest first (without their entries).
pub async fn list(conn: &mut PgConnection, f: &ReportFilter) -> ExpenseResult<Vec<Report>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR r.user_id = $1) AND ($2::text IS NULL OR r.status = $2) \
          ORDER BY r.submitted_at DESC LIMIT 200"
    )))
    .bind(f.user_id)
    .bind(f.status)
    .fetch_all(conn)
    .await?)
}

/// Who owns a report and its organization: `(user, org)`.
pub async fn owner_of(conn: &mut PgConnection, id: Uuid) -> ExpenseResult<Option<(Uuid, Uuid)>> {
    Ok(
        sqlx::query_as("SELECT user_id, org_id FROM exp_reports WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// The MSP an organization is under: its nearest provider, or the top
/// organization when there's none above it.
async fn provider_of(conn: &mut PgConnection, org: Uuid) -> ExpenseResult<Uuid> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT p.id FROM organizations p JOIN organizations c ON c.id = $1 \
          WHERE p.path @> c.path AND (p.kind = 'provider' OR nlevel(p.path) = 1) \
          ORDER BY (p.kind = 'provider') DESC, nlevel(p.path) DESC LIMIT 1",
    )
    .bind(org)
    .fetch_optional(conn)
    .await?;
    Ok(found.unwrap_or(org))
}

/// The leads of `user`'s teams, as approvers (administrators if none).
async fn reviewers(conn: &mut PgConnection, user: Uuid) -> ExpenseResult<Vec<String>> {
    let leads: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT l.user_id FROM team_members m JOIN team_members l ON l.team_id = m.team_id AND l.lead \
          WHERE m.user_id = $1 AND l.user_id <> $1 ORDER BY l.user_id",
    )
    .bind(user)
    .fetch_all(conn)
    .await?;
    Ok(if leads.is_empty() {
        vec!["role:admin".to_owned()]
    } else {
        leads.iter().map(|u| format!("user:{u}")).collect()
    })
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitInput {
    #[serde(default)]
    pub title: Option<String>,
    /// The entries: your drafts, or ones sent back.
    pub entry_ids: Vec<Uuid>,
}

/// Checks the entries can be submitted together by `user`; their MSP.
async fn check_entries(conn: &mut PgConnection, user: Uuid, ids: &[Uuid]) -> ExpenseResult<Uuid> {
    let rows: Vec<(Uuid, Uuid, Uuid, entries::Status)> = sqlx::query_as(
        "SELECT id, user_id, org_id, status FROM exp_entries WHERE id = ANY($1) FOR UPDATE",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() != ids.len() {
        return Err(ExpenseError::invalid(
            "entryIds",
            "not all of those are expenses",
        ));
    }
    let mut provider = None;
    for (_, owner, org, status) in &rows {
        if *owner != user {
            return Err(ExpenseError::invalid(
                "entryIds",
                "you can only submit your own expenses",
            ));
        }
        if !matches!(status, entries::Status::Draft | entries::Status::Rejected) {
            return Err(ExpenseError::invalid(
                "entryIds",
                "some of those are already submitted",
            ));
        }
        let p = provider_of(&mut *conn, *org).await?;
        if provider.is_some_and(|q| q != p) {
            return Err(ExpenseError::invalid(
                "entryIds",
                "submit expenses for one MSP at a time",
            ));
        }
        provider = Some(p);
    }
    Ok(provider.unwrap_or_default())
}

/// Raises `expense_report.submitted` for the approval workflow.
async fn announce(conn: &mut PgConnection, r: &Report, reviewers: &[String]) -> ExpenseResult<()> {
    let (id, org, user) = (r.id, r.org_id, r.user_id);
    let totals: Vec<String> = r
        .totals
        .iter()
        .map(|t| format!("{} {}", t.amount.normalize(), t.currency))
        .collect();
    let flags: Vec<String> = r
        .flags
        .iter()
        .map(|(f, n)| {
            format!(
                "{n} {}",
                if f == "over_limit" {
                    "over the limit"
                } else {
                    "without a receipt"
                }
            )
        })
        .collect();
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "expense_report.submitted",
            org_id: Some(org),
            subject_type: "expense_report",
            subject_id: id,
            payload: json!({
                "reportId": id, "reference": format!("EXP-{}", r.number), "title": r.title,
                "userId": user, "person": r.user_name, "entries": r.entries.len(),
                "total": totals.join(" + "), "flags": flags, "reviewers": reviewers,
                "link": format!("/expenses/reports/{id}"),
            }),
        },
    )
    .await?;
    Ok(())
}

/// Submits `user`'s entries as a report.
pub async fn submit(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: Uuid,
    input: &SubmitInput,
) -> ExpenseResult<Report> {
    if input.entry_ids.is_empty() || input.entry_ids.len() > 200 {
        return Err(ExpenseError::invalid(
            "entryIds",
            "choose 1 to 200 expenses",
        ));
    }
    let org = check_entries(&mut *conn, user, &input.entry_ids).await?;
    let title = match input
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(t) => required("title", t, 200)?,
        None => format!("Expenses, {}", OffsetDateTime::now_utc().date()),
    };
    let reviewers = reviewers(&mut *conn, user).await?;
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO exp_reports (id, org_id, user_id, title, reviewers) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(org)
        .bind(user)
        .bind(&title)
        .bind(&reviewers)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE exp_entries SET report_id = $2, status = 'submitted', version = version + 1, updated_at = now() \
          WHERE id = ANY($1)",
    )
    .bind(&input.entry_ids)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    let r = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense_report.submitted")
            .org(org)
            .target("expense_report", id)
            .details(json!({ "entries": r.entries.len(), "reviewers": reviewers })),
    )
    .await?;
    announce(&mut *conn, &r, &reviewers).await?;
    Ok(r)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ExpenseReportDecision)]
pub enum Decision {
    Approved,
    ChangesRequested,
    Rejected,
}

impl Decision {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "approved" => Some(Self::Approved),
            "changes_requested" => Some(Self::ChangesRequested),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    const fn status(self) -> ReportStatus {
        match self {
            Self::Approved => ReportStatus::Approved,
            Self::ChangesRequested => ReportStatus::ChangesRequested,
            Self::Rejected => ReportStatus::Rejected,
        }
    }
}

/// Records the decision on a submitted report (from the approval
/// workflow): approved locks its entries; otherwise they're sent back.
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    decision: Decision,
    by: &str,
    comment: Option<&str>,
) -> ExpenseResult<Report> {
    let status: ReportStatus =
        sqlx::query_scalar("SELECT status FROM exp_reports WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(ExpenseError::NotFound)?;
    if status != ReportStatus::Submitted {
        return Err(ExpenseError::Conflict(
            "this report was already decided".into(),
        ));
    }
    let by = required("by", by, 200)?;
    let comment = crate::text("comment", comment, 2000)?;
    sqlx::query(
        "UPDATE exp_reports SET status = $2, decided_by = $3, decided_at = now(), comment = $4, version = version + 1 \
          WHERE id = $1",
    )
    .bind(id)
    .bind(decision.status())
    .bind(&by)
    .bind(&comment)
    .execute(&mut *conn)
    .await?;
    let entry_status = if decision == Decision::Approved {
        "approved"
    } else {
        "rejected"
    };
    sqlx::query(
        "UPDATE exp_entries SET status = $2, version = version + 1, updated_at = now() WHERE report_id = $1",
    )
    .bind(id)
    .bind(entry_status)
    .execute(&mut *conn)
    .await?;
    let r = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense_report.decided")
            .org(r.org_id)
            .target("expense_report", id)
            .details(json!({ "decision": decision, "by": by, "comment": comment })),
    )
    .await?;
    let event = match decision {
        Decision::Approved => "expense_report.approved",
        Decision::ChangesRequested => "expense_report.changes_requested",
        Decision::Rejected => "expense_report.rejected",
    };
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: event,
            org_id: Some(r.org_id),
            subject_type: "expense_report",
            subject_id: id,
            payload: json!({
                "reportId": id, "reference": format!("EXP-{}", r.number), "userId": r.user_id,
                "by": by, "comment": comment, "link": format!("/expenses/reports/{id}"),
            }),
        },
    )
    .await?;
    let title = match decision {
        Decision::Approved => format!("EXP-{} {} was approved", r.number, r.title),
        Decision::ChangesRequested => format!("Changes asked for on EXP-{} {}", r.number, r.title),
        Decision::Rejected => format!("EXP-{} {} was rejected", r.number, r.title),
    };
    let body = comment.map_or_else(|| format!("By {by}."), |c| format!("{by}: {c}"));
    tell(
        &mut *conn,
        r.user_id,
        kinds::EXPENSE_REVIEWED,
        &title,
        &body,
        &format!("/expenses/reports/{id}"),
    )
    .await?;
    Ok(r)
}

async fn tell(
    conn: &mut PgConnection,
    user_id: Uuid,
    kind: &'static str,
    title: &str,
    body: &str,
    link: &str,
) -> ExpenseResult<()> {
    huxley_notify::notify(
        conn,
        Notice {
            user_id,
            kind,
            title,
            body,
            link: Some(link),
        },
    )
    .await
    .map_err(|e| match e {
        huxley_notify::NotifyError::Db(e) => ExpenseError::Sqlx(e),
        other => ExpenseError::Conflict(other.to_string()),
    })?;
    Ok(())
}

/// The open approval for a report (from the approval workflow), and
/// whether `user` may decide it: `(approval id, user may decide)`.
pub async fn open_approval(
    conn: &mut PgConnection,
    report: Uuid,
    user: Uuid,
) -> ExpenseResult<Option<(Uuid, bool)>> {
    Ok(sqlx::query_as(
        "SELECT a.id, $2 = ANY(a.approvers) FROM approvals a JOIN runs r ON r.id = a.run_id \
          WHERE a.status = 'pending' AND a.expires_at > now() AND r.inputs->>'reportId' = $1::text \
          ORDER BY a.created_at DESC LIMIT 1",
    )
    .bind(report)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

// ---- comments ----

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ExpenseReportComment)]
pub struct Comment {
    pub id: Uuid,
    pub entry_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub body: String,
    pub mentions: Vec<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ExpenseReportCommentInput)]
pub struct CommentInput {
    pub body: String,
    /// One of its entries, or none for the whole report.
    #[serde(default)]
    pub entry_id: Option<Uuid>,
    /// People @mentioned: they're told.
    #[serde(default)]
    pub mentions: Vec<Uuid>,
}

const COMMENT: &str = "SELECT c.id, c.entry_id, a.display_name AS author_name, c.body, c.mentions, c.created_at \
    FROM exp_report_comments c LEFT JOIN users a ON a.id = c.author_id";

pub async fn comments(conn: &mut PgConnection, report: Uuid) -> ExpenseResult<Vec<Comment>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{COMMENT} WHERE c.report_id = $1 ORDER BY c.created_at, c.id"
    )))
    .bind(report)
    .fetch_all(conn)
    .await?)
}

pub async fn comment(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    report: Uuid,
    input: &CommentInput,
) -> ExpenseResult<Comment> {
    let r = get(&mut *conn, report).await?;
    let body = required("body", &input.body, 4000)?;
    if let Some(entry) = input.entry_id
        && !r.entries.iter().any(|e| e.id == entry)
    {
        return Err(ExpenseError::invalid(
            "entryId",
            "not an expense in this report",
        ));
    }
    let mut mentions = input.mentions.clone();
    if ctx.actor.id() != Some(r.user_id) && !mentions.contains(&r.user_id) {
        // Whoever submitted it hears about comments from others.
        mentions.push(r.user_id);
    }
    mentions.sort_unstable();
    mentions.dedup();
    let known: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE id = ANY($1) AND status = 'active'")
            .bind(&mentions)
            .fetch_all(&mut *conn)
            .await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO exp_report_comments (id, report_id, org_id, entry_id, author_id, body, mentions) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(report)
    .bind(r.org_id)
    .bind(input.entry_id)
    .bind(ctx.actor.id())
    .bind(&body)
    .bind(&known)
    .execute(&mut *conn)
    .await?;
    let title = format!("A comment on EXP-{} {}", r.number, r.title);
    let link = format!("/expenses/reports/{report}");
    for user in known.iter().filter(|u| Some(**u) != ctx.actor.id()) {
        tell(
            &mut *conn,
            *user,
            kinds::EXPENSE_MENTIONED,
            &title,
            &body,
            &link,
        )
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense_report.commented")
            .org(r.org_id)
            .target("expense_report", report)
            .details(json!({ "comment": id, "mentions": known })),
    )
    .await?;
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{COMMENT} WHERE c.id = $1")))
        .bind(id)
        .fetch_one(conn)
        .await
        .map_err(Into::into)
}
