//! Internal invoice approval (ADR-076), on the quote review model.
//!
//! - **Rules** (instance-wide) decide which invoices need a review and who
//!   reviews them: always; any line changed by hand (on by default); a
//!   total above an amount; a total that moved more than a percent from the
//!   client's last invoice; a client's first invoice; credit memos; or any
//!   expression over the invoice. Reasons are refreshed on every change.
//! - **Submitting** keeps what was reviewed and raises
//!   `invoice.submitted_for_review` for the review workflow (a built-in
//!   pack on the approval step), which records the decision ([`decide`]).
//!   An approved review approves the invoice.
//! - **Approving** an invoice that needs review takes an approved review of
//!   its current revision, or the override permission (recorded).
//! - **Runs** are approved in one pass ([`approve_run`]): what needs no
//!   review is approved; the rest are flagged (and can be submitted).
//! - **Comments** on the invoice or a line, with `@mentions` that notify.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_notify::{Notice, kinds};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::invoices::{self, Invoice, Status};
use crate::{BillingError, BillingResult, required};

// ---- rules ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InvoiceReviewRuleKind)]
pub enum RuleKind {
    /// Every invoice.
    Always,
    /// Any line changed (or added) by hand.
    Edited,
    /// A total over `threshold` (in the invoice's currency).
    TotalAbove,
    /// A total more than `threshold` percent up or down on the client's
    /// last invoice in the same currency.
    ChangeAbove,
    /// The client's first invoice.
    NewClient,
    /// Credit memos.
    CreditMemo,
    /// `expression` is true of the invoice (as `invoice`, with its lines).
    Expression,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceReviewRule)]
pub struct Rule {
    pub id: Uuid,
    pub name: String,
    pub kind: RuleKind,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub threshold: Option<Decimal>,
    pub expression: Option<String>,
    /// Who reviews: `role:<key>`, `team:<id>`, `user:<id>`, several separated by commas.
    pub reviewers: String,
    pub active: bool,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InvoiceReviewRuleInput)]
pub struct RuleInput {
    pub name: String,
    pub kind: RuleKind,
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub threshold: Option<Decimal>,
    #[serde(default)]
    pub expression: Option<String>,
    pub reviewers: String,
    #[serde(default = "yes")]
    pub active: bool,
}

const fn yes() -> bool {
    true
}

/// A rule's expression as a condition: `{{ … }}` is optional.
fn condition(expr: &str) -> String {
    if expr.contains("{{") {
        expr.to_owned()
    } else {
        format!("{{{{ {expr} }}}}")
    }
}

const RULE: &str =
    "SELECT id, name, kind, threshold, expression, reviewers, active FROM billing_review_rules";

pub async fn rules(conn: &mut PgConnection) -> BillingResult<Vec<Rule>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{RULE} ORDER BY position, created_at"
    )))
    .fetch_all(conn)
    .await?)
}

fn check_rule(input: &RuleInput) -> BillingResult<(String, String, Option<String>)> {
    let name = required("name", &input.name, 100)?;
    let reviewers = required("reviewers", &input.reviewers, 500)?;
    for r in reviewers.split(',').map(str::trim) {
        let ok = r.strip_prefix("role:").is_some_and(|k| !k.is_empty())
            || r.strip_prefix("team:")
                .is_some_and(|i| Uuid::parse_str(i).is_ok())
            || r.strip_prefix("user:")
                .is_some_and(|i| Uuid::parse_str(i).is_ok());
        if !ok {
            return Err(BillingError::invalid(
                "reviewers",
                format!("{r:?}: use role:<key>, team:<id> or user:<id>"),
            ));
        }
    }
    if matches!(input.kind, RuleKind::TotalAbove | RuleKind::ChangeAbove)
        && input.threshold.is_none_or(|t| t < Decimal::ZERO)
    {
        return Err(BillingError::invalid(
            "threshold",
            "required for this kind of rule (zero or more)",
        ));
    }
    let expression = input
        .expression
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_owned);
    if input.kind == RuleKind::Expression {
        let Some(e) = &expression else {
            return Err(BillingError::invalid(
                "expression",
                "required for this kind of rule",
            ));
        };
        let scope = huxley_expr::Scope::new().with("invoice", &json!({}));
        if let Err(err) = huxley_expr::Engine::new().eval_condition(&condition(e), &scope)
            && err.to_string().contains("syntax")
        {
            return Err(BillingError::invalid("expression", err.to_string()));
        }
    }
    Ok((name, reviewers, expression))
}

async fn rule(conn: &mut PgConnection, id: Uuid) -> BillingResult<Rule> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{RULE} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(BillingError::NotFound)
}

pub async fn create_rule(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &RuleInput,
) -> BillingResult<Rule> {
    let (name, reviewers, expression) = check_rule(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO billing_review_rules (id, name, kind, threshold, expression, reviewers, active, position) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, (SELECT coalesce(max(position), 0) + 1 FROM billing_review_rules))",
    )
    .bind(id)
    .bind(&name)
    .bind(input.kind)
    .bind(input.threshold)
    .bind(&expression)
    .bind(&reviewers)
    .bind(input.active)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice_review_rule.created")
            .target("invoice_review_rule", id)
            .details(json!({ "name": name, "kind": input.kind })),
    )
    .await?;
    rule(conn, id).await
}

pub async fn update_rule(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &RuleInput,
) -> BillingResult<Rule> {
    let (name, reviewers, expression) = check_rule(input)?;
    let done = sqlx::query(
        "UPDATE billing_review_rules SET name = $2, kind = $3, threshold = $4, expression = $5, reviewers = $6, \
            active = $7 WHERE id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(input.kind)
    .bind(input.threshold)
    .bind(&expression)
    .bind(&reviewers)
    .bind(input.active)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(BillingError::NotFound);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice_review_rule.updated")
            .target("invoice_review_rule", id)
            .details(json!({ "name": name, "kind": input.kind, "active": input.active })),
    )
    .await?;
    rule(conn, id).await
}

pub async fn delete_rule(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> BillingResult<()> {
    let done = sqlx::query("DELETE FROM billing_review_rules WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    if done.rows_affected() == 0 {
        return Err(BillingError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("invoice_review_rule.deleted").target("invoice_review_rule", id),
    )
    .await?;
    Ok(())
}

// ---- matching ----

/// A rule an invoice matches, and why.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceReviewReason)]
pub struct Reason {
    pub rule_id: Uuid,
    pub name: String,
    pub detail: String,
    pub reviewers: String,
}

/// The client's last invoice before this one, in its currency: `(number, total)`.
async fn previous(conn: &mut PgConnection, i: &Invoice) -> BillingResult<Option<(i64, Decimal)>> {
    Ok(sqlx::query_as(
        "SELECT v.number, coalesce((SELECT sum(round(l.quantity * l.unit_price, 2)) FROM billing_invoice_lines l \
                                    WHERE l.invoice_id = v.id), 0) \
           FROM billing_invoices v \
          WHERE v.org_id = $1 AND v.currency = $2 AND v.kind = 'invoice' AND v.status IN ('approved', 'sent') \
            AND v.id <> $3 AND v.created_at < $4 \
          ORDER BY v.created_at DESC LIMIT 1",
    )
    .bind(i.org_id)
    .bind(&i.currency)
    .bind(i.id)
    .bind(i.created_at)
    .fetch_optional(conn)
    .await?)
}

/// Why one rule matches an invoice, if it does.
async fn matches(conn: &mut PgConnection, r: &Rule, i: &Invoice) -> BillingResult<Option<String>> {
    let limit = r.threshold.unwrap_or_default();
    Ok(match r.kind {
        RuleKind::Always => Some("every invoice is reviewed".to_owned()),
        RuleKind::Edited => {
            let edited: Vec<&str> = i
                .lines
                .iter()
                .filter(|l| l.edited)
                .map(|l| l.description.as_str())
                .collect();
            (!edited.is_empty()).then(|| {
                let shown: Vec<&str> = edited.iter().take(3).copied().collect();
                let more = edited.len().saturating_sub(3);
                format!(
                    "{} changed by hand: {}{}",
                    if edited.len() == 1 {
                        "1 line".to_owned()
                    } else {
                        format!("{} lines", edited.len())
                    },
                    shown.join(", "),
                    if more > 0 {
                        format!(" and {more} more")
                    } else {
                        String::new()
                    }
                )
            })
        }
        RuleKind::TotalAbove => (i.total > limit).then(|| {
            format!(
                "total {} {} is over {}",
                i.total.normalize(),
                i.currency,
                limit.normalize()
            )
        }),
        RuleKind::ChangeAbove => match previous(&mut *conn, i).await? {
            Some((number, before)) if before > Decimal::ZERO => {
                let pct = ((i.total - before) / before * Decimal::ONE_HUNDRED).round_dp(1);
                (pct.abs() > limit).then(|| {
                    format!(
                        "total {} is {}% {} on INV-{number} ({})",
                        i.total.normalize(),
                        pct.abs().normalize(),
                        if pct > Decimal::ZERO { "up" } else { "down" },
                        before.normalize()
                    )
                })
            }
            _ => None,
        },
        RuleKind::NewClient => previous(&mut *conn, i)
            .await?
            .is_none()
            .then(|| "the client's first invoice".to_owned()),
        RuleKind::CreditMemo => (i.kind == "credit_memo").then(|| "a credit memo".to_owned()),
        RuleKind::Expression => {
            let invoice = serde_json::to_value(i).unwrap_or(Json::Null);
            let scope = huxley_expr::Scope::new().with("invoice", &invoice);
            let expr = r.expression.as_deref().unwrap_or("false");
            huxley_expr::Engine::new()
                .eval_condition(&condition(expr), &scope)
                .unwrap_or(false)
                .then(|| format!("matches {expr}"))
        }
    })
}

/// The active rules an invoice matches.
pub async fn reasons(conn: &mut PgConnection, i: &Invoice) -> BillingResult<Vec<Reason>> {
    let mut out = Vec::new();
    for r in rules(&mut *conn).await?.into_iter().filter(|r| r.active) {
        if let Some(detail) = matches(&mut *conn, &r, i).await? {
            out.push(Reason {
                rule_id: r.id,
                name: r.name,
                detail,
                reviewers: r.reviewers,
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InvoiceReviewStatus)]
pub enum ReviewStatus {
    /// No rule matches.
    NotNeeded,
    /// A rule matches; not reviewed (or changed since).
    Needed,
    InReview,
    Approved,
    ChangesRequested,
    Rejected,
}

/// Works out a draft's reasons again after a change (or after it was
/// made), and whether its review still stands: a change after review needs
/// a new one.
pub async fn refresh(conn: &mut PgConnection, id: Uuid) -> BillingResult<()> {
    let i = invoices::get(&mut *conn, id).await?;
    if i.status != Status::Draft {
        return Ok(());
    }
    let found = reasons(&mut *conn, &i).await?;
    let (status, reviewed, revision): (ReviewStatus, Option<i32>, i32) = sqlx::query_as(
        "SELECT review_status, review_revision, revision FROM billing_invoices WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    let decided_or_open = !matches!(status, ReviewStatus::NotNeeded | ReviewStatus::Needed);
    let stale = decided_or_open && reviewed != Some(revision);
    let fresh = if found.is_empty() {
        ReviewStatus::NotNeeded
    } else {
        ReviewStatus::Needed
    };
    let next = if decided_or_open && !stale {
        status
    } else {
        fresh
    };
    sqlx::query(
        "UPDATE billing_invoices SET review_reasons = $2, review_status = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(serde_json::to_value(&found).unwrap_or(Json::Array(Vec::new())))
    .bind(next)
    .execute(conn)
    .await?;
    Ok(())
}

/// The review state of an invoice, for its page.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceReviewState)]
pub struct ReviewState {
    pub status: ReviewStatus,
    pub reasons: Vec<Reason>,
    /// It can be approved without an override: no review needed, or one
    /// approved for this revision.
    pub can_approve: bool,
    pub overridden: bool,
    /// Each submission, newest first.
    pub history: Vec<ReviewRecord>,
    /// What changed since the latest decided review.
    pub changes: Option<Changes>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceReviewRecord)]
pub struct ReviewRecord {
    pub id: Uuid,
    pub revision: i32,
    #[schema(value_type = Vec<Reason>)]
    #[sqlx(json)]
    pub reasons: Vec<Reason>,
    pub reviewers: Vec<String>,
    pub submitted_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub submitted_at: OffsetDateTime,
    /// `approved`, `changes_requested` or `rejected`; none while in review.
    pub decision: Option<String>,
    pub decided_by: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub comment: Option<String>,
}

async fn history(conn: &mut PgConnection, id: Uuid) -> BillingResult<Vec<ReviewRecord>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.revision, r.reasons, r.reviewers, u.display_name AS submitted_by_name, \
                r.submitted_at, r.decision, r.decided_by, r.decided_at, r.comment \
           FROM billing_invoice_reviews r LEFT JOIN users u ON u.id = r.submitted_by \
          WHERE r.invoice_id = $1 ORDER BY r.submitted_at DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

type Row = (ReviewStatus, Option<i32>, i32, Json, bool);

async fn row(conn: &mut PgConnection, id: Uuid) -> BillingResult<Row> {
    sqlx::query_as(
        "SELECT review_status, review_revision, revision, review_reasons, review_overridden \
           FROM billing_invoices WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(BillingError::NotFound)
}

fn approvable((status, reviewed, revision, reasons, _): &Row) -> bool {
    reasons.as_array().is_none_or(Vec::is_empty)
        || (*status == ReviewStatus::Approved && *reviewed == Some(*revision))
}

pub async fn state(conn: &mut PgConnection, id: Uuid) -> BillingResult<ReviewState> {
    let i = invoices::get(&mut *conn, id).await?;
    let r = row(&mut *conn, id).await?;
    Ok(ReviewState {
        status: r.0,
        can_approve: approvable(&r),
        reasons: serde_json::from_value(r.3.clone()).unwrap_or_default(),
        overridden: r.4,
        history: history(&mut *conn, id).await?,
        changes: changes(&mut *conn, &i).await?,
    })
}

/// Whether an invoice may be approved now, without an override.
pub async fn may_approve(conn: &mut PgConnection, id: Uuid) -> BillingResult<bool> {
    Ok(approvable(&row(conn, id).await?))
}

/// The open approval for an invoice's review (from the review workflow),
/// and whether `user` may decide it: `(approval id, user may decide)`.
pub async fn open_approval(
    conn: &mut PgConnection,
    invoice: Uuid,
    user: Uuid,
) -> BillingResult<Option<(Uuid, bool)>> {
    Ok(sqlx::query_as(
        "SELECT a.id, $2 = ANY(a.approvers) FROM approvals a JOIN runs r ON r.id = a.run_id \
          WHERE a.status = 'pending' AND a.expires_at > now() \
            AND r.inputs->>'reviewId' = (SELECT id::text FROM billing_invoice_reviews WHERE invoice_id = $1 \
                                          ORDER BY submitted_at DESC LIMIT 1) \
          ORDER BY a.created_at DESC LIMIT 1",
    )
    .bind(invoice)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// Records an approval without an approved review (the caller checked the
/// permission).
pub(crate) async fn record_override(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    i: &Invoice,
) -> BillingResult<()> {
    sqlx::query("UPDATE billing_invoices SET review_overridden = true WHERE id = $1")
        .bind(i.id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("invoice.review_overridden")
            .org(i.org_id)
            .target("invoice", i.id)
            .details(json!({ "number": i.number })),
    )
    .await?;
    Ok(())
}

// ---- what changed ----

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceFieldChange)]
pub struct FieldChange {
    pub field: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceLineChange)]
pub struct LineChange {
    pub name: String,
    pub fields: Vec<FieldChange>,
}

/// What changed since the last decided review.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceChanges)]
pub struct Changes {
    pub since_revision: i32,
    pub decision: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<LineChange>,
    pub total: Option<FieldChange>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.total.is_none()
    }

    /// One line for an event or a message.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("added {}", self.added.join(", ")));
        }
        if !self.removed.is_empty() {
            parts.push(format!("removed {}", self.removed.join(", ")));
        }
        for c in &self.changed {
            let f: Vec<String> = c
                .fields
                .iter()
                .map(|f| format!("{} {} → {}", f.field, f.before, f.after))
                .collect();
            parts.push(format!("{}: {}", c.name, f.join(", ")));
        }
        if let Some(t) = &self.total {
            parts.push(format!("total {} → {}", t.before, t.after));
        }
        parts.join("; ")
    }
}

fn text_of(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

/// Compares the invoice with the snapshot of its latest decided review.
async fn changes(conn: &mut PgConnection, i: &Invoice) -> BillingResult<Option<Changes>> {
    let last: Option<(i32, String, Json)> = sqlx::query_as(
        "SELECT revision, decision, snapshot FROM billing_invoice_reviews \
          WHERE invoice_id = $1 AND decision IS NOT NULL ORDER BY decided_at DESC LIMIT 1",
    )
    .bind(i.id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((since, decision, before)) = last else {
        return Ok(None);
    };
    let now = serde_json::to_value(i).unwrap_or(Json::Null);
    let lines = |doc: &Json| -> BTreeMap<String, Json> {
        doc["lines"]
            .as_array()
            .map(|ls| ls.iter().map(|l| (text_of(&l["id"]), l.clone())).collect())
            .unwrap_or_default()
    };
    let (old, new) = (lines(&before), lines(&now));
    let mut out = Changes {
        since_revision: since,
        decision,
        added: new
            .iter()
            .filter(|(id, _)| !old.contains_key(*id))
            .map(|(_, l)| text_of(&l["description"]))
            .collect(),
        removed: old
            .iter()
            .filter(|(id, _)| !new.contains_key(*id))
            .map(|(_, l)| text_of(&l["description"]))
            .collect(),
        changed: Vec::new(),
        total: (before["total"] != now["total"]).then(|| FieldChange {
            field: "total".into(),
            before: text_of(&before["total"]),
            after: text_of(&now["total"]),
        }),
    };
    for (id, n) in &new {
        let Some(o) = old.get(id) else { continue };
        let fields: Vec<FieldChange> = [
            ("description", "description"),
            ("quantity", "quantity"),
            ("price", "unitPrice"),
            ("tax code", "taxCode"),
        ]
        .iter()
        .filter(|(_, key)| o[*key] != n[*key])
        .map(|(field, key)| FieldChange {
            field: (*field).to_owned(),
            before: text_of(&o[*key]),
            after: text_of(&n[*key]),
        })
        .collect();
        if !fields.is_empty() {
            out.changed.push(LineChange {
                name: text_of(&n["description"]),
                fields,
            });
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

// ---- submitting and deciding ----

async fn name_of(conn: &mut PgConnection, user: Option<Uuid>) -> BillingResult<Option<String>> {
    Ok(match user {
        Some(u) => {
            sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
                .bind(u)
                .fetch_optional(conn)
                .await?
        }
        None => None,
    })
}

/// Submits a draft for review: keeps what's reviewed and raises
/// `invoice.submitted_for_review` for the review workflow. `version` is
/// checked when given.
pub async fn submit(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: Option<i32>,
) -> BillingResult<ReviewState> {
    refresh(&mut *conn, id).await?;
    let i = invoices::get(&mut *conn, id).await?;
    if version.is_some_and(|v| v != i.version) {
        return Err(BillingError::Conflict(
            "it was changed by someone else since you loaded it; reload and try again".into(),
        ));
    }
    if i.status != Status::Draft {
        return Err(BillingError::Conflict(
            "only a draft can be submitted for review".into(),
        ));
    }
    let current = state(&mut *conn, id).await?;
    if current.status == ReviewStatus::InReview {
        return Err(BillingError::Conflict("it's already in review".into()));
    }
    if i.lines.is_empty() {
        return Err(BillingError::invalid("lines", "nothing to bill"));
    }
    let mut reviewers: Vec<String> = current
        .reasons
        .iter()
        .flat_map(|r| r.reviewers.split(',').map(|s| s.trim().to_owned()))
        .filter(|s| !s.is_empty())
        .collect();
    reviewers.sort();
    reviewers.dedup();
    if reviewers.is_empty() {
        reviewers.push("role:admin".into());
    }
    let revision: i32 = sqlx::query_scalar("SELECT revision FROM billing_invoices WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let review = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO billing_invoice_reviews (id, invoice_id, org_id, revision, snapshot, reasons, reviewers, \
            submitted_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(review)
    .bind(id)
    .bind(i.org_id)
    .bind(revision)
    .bind(serde_json::to_value(&i).unwrap_or(Json::Null))
    .bind(serde_json::to_value(&current.reasons).unwrap_or(Json::Null))
    .bind(&reviewers)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE billing_invoices SET review_status = 'in_review', review_revision = revision WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.submitted_for_review")
            .org(i.org_id)
            .target("invoice", id)
            .details(
                json!({ "revision": revision, "reasons": current.reasons, "reviewers": reviewers }),
            ),
    )
    .await?;
    let submitter = name_of(&mut *conn, ctx.actor.id()).await?;
    let previous = previous(&mut *conn, &i).await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "invoice.submitted_for_review",
            org_id: Some(i.org_id),
            subject_type: "invoice",
            subject_id: id,
            payload: json!({
                "invoiceId": id,
                "reviewId": review,
                "reference": format!("INV-{}", i.number),
                "kind": i.kind,
                "client": i.org_name,
                "orgId": i.org_id,
                "revision": revision,
                "currency": i.currency,
                "total": i.total.normalize().to_string(),
                "previousTotal": previous.map(|(_, t)| t.normalize().to_string()),
                "lines": i.lines.len(),
                "edited": i.lines.iter().filter(|l| l.edited).count(),
                "reasons": current.reasons.iter().map(|r| format!("{}: {}", r.name, r.detail)).collect::<Vec<_>>(),
                "reviewers": reviewers,
                "submittedBy": submitter,
                "changes": current.changes.as_ref().map(Changes::summary),
                "link": format!("/billing/invoices/{id}"),
            }),
        },
    )
    .await?;
    state(conn, id).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = InvoiceReviewDecision)]
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

    const fn status(self) -> ReviewStatus {
        match self {
            Self::Approved => ReviewStatus::Approved,
            Self::ChangesRequested => ReviewStatus::ChangesRequested,
            Self::Rejected => ReviewStatus::Rejected,
        }
    }

    const fn event(self) -> &'static str {
        match self {
            Self::Approved => "invoice.review_approved",
            Self::ChangesRequested => "invoice.changes_requested",
            Self::Rejected => "invoice.rejected",
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::Rejected => "rejected",
        }
    }
}

/// Records a review's decision (from the review workflow). An approved
/// review approves the invoice, unless it was put on hold meanwhile. Only
/// the invoice's open review can be decided.
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    review: Uuid,
    decision: Decision,
    by: &str,
    comment: Option<&str>,
) -> BillingResult<ReviewState> {
    let row: Option<(Uuid, Option<String>, Option<Uuid>)> = sqlx::query_as(
        "SELECT invoice_id, decision, submitted_by FROM billing_invoice_reviews WHERE id = $1",
    )
    .bind(review)
    .fetch_optional(&mut *conn)
    .await?;
    let (id, already, submitter) = row.ok_or(BillingError::NotFound)?;
    if already.is_some() {
        return Err(BillingError::Conflict(
            "this review was already decided".into(),
        ));
    }
    let latest: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM billing_invoice_reviews WHERE invoice_id = $1 ORDER BY submitted_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let (status, ..) = self::row(&mut *conn, id).await?;
    if latest != Some(review) || status != ReviewStatus::InReview {
        return Err(BillingError::Conflict(
            "the invoice changed since it was submitted; it needs a new review".into(),
        ));
    }
    let by = required("by", by, 200)?;
    let comment = crate::text("comment", comment, 2000)?;
    sqlx::query(
        "UPDATE billing_invoice_reviews SET decision = $2, decided_by = $3, decided_at = now(), comment = $4 \
          WHERE id = $1",
    )
    .bind(review)
    .bind(decision.as_str())
    .bind(&by)
    .bind(&comment)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE billing_invoices SET review_status = $2 WHERE id = $1")
        .bind(id)
        .bind(decision.status())
        .execute(&mut *conn)
        .await?;
    let i = invoices::get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.reviewed")
            .org(i.org_id)
            .target("invoice", id)
            .details(json!({ "decision": decision, "by": by, "comment": comment })),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: decision.event(),
            org_id: Some(i.org_id),
            subject_type: "invoice",
            subject_id: id,
            payload: json!({
                "invoiceId": id, "reviewId": review, "reference": format!("INV-{}", i.number),
                "client": i.org_name, "orgId": i.org_id, "by": by, "comment": comment,
            }),
        },
    )
    .await?;
    if decision == Decision::Approved && i.status == Status::Draft && !i.on_hold {
        invoices::finish_approval(&mut *conn, ctx, &i).await?;
    }
    if let Some(user) = submitter {
        let title = match decision {
            Decision::Approved => format!("INV-{} for {} was approved", i.number, i.org_name),
            Decision::ChangesRequested => {
                format!("Changes requested on INV-{} for {}", i.number, i.org_name)
            }
            Decision::Rejected => format!("INV-{} for {} was rejected", i.number, i.org_name),
        };
        let body = comment
            .clone()
            .map_or_else(|| format!("By {by}."), |c| format!("{by}: {c}"));
        tell(
            &mut *conn,
            user,
            kinds::INVOICE_REVIEWED,
            &title,
            &body,
            &format!("/billing/invoices/{id}"),
        )
        .await?;
    }
    state(conn, id).await
}

/// Notifies someone about an invoice.
async fn tell(
    conn: &mut PgConnection,
    user_id: Uuid,
    kind: &'static str,
    title: &str,
    body: &str,
    link: &str,
) -> BillingResult<()> {
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
        huxley_notify::NotifyError::Db(e) => BillingError::Sqlx(e),
        other => BillingError::Conflict(other.to_string()),
    })?;
    Ok(())
}

// ---- approving a run ----

/// An invoice a run's approval left out, and why.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunException {
    pub invoice_id: Uuid,
    pub number: i64,
    pub org_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunApproval {
    pub approved: Vec<Uuid>,
    pub exceptions: Vec<RunException>,
}

/// Approves a run's drafts in one pass. Those that need a review (and
/// haven't had one approved) and those on hold are flagged; with `submit`,
/// those that need a review and aren't in one are submitted.
pub async fn approve_run(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    run: Uuid,
    submit_reviews: bool,
) -> BillingResult<RunApproval> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM billing_invoices WHERE run_id = $1 AND status = 'draft' ORDER BY number",
    )
    .bind(run)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = RunApproval {
        approved: Vec::new(),
        exceptions: Vec::new(),
    };
    for id in ids {
        refresh(&mut *conn, id).await?;
        let i = invoices::get(&mut *conn, id).await?;
        let r = row(&mut *conn, id).await?;
        let reason = if i.on_hold {
            Some(
                i.hold_reason
                    .clone()
                    .map_or_else(|| "on hold".to_owned(), |h| format!("on hold: {h}")),
            )
        } else if i.lines.is_empty() {
            Some("nothing to bill".to_owned())
        } else if approvable(&r) {
            None
        } else {
            Some(match r.0 {
                ReviewStatus::InReview => "in review".to_owned(),
                ReviewStatus::ChangesRequested => "changes requested in review".to_owned(),
                ReviewStatus::Rejected => "rejected in review".to_owned(),
                _ if submit_reviews => {
                    submit(&mut *conn, ctx, id, None).await?;
                    "submitted for review".to_owned()
                }
                _ => "needs review".to_owned(),
            })
        };
        match reason {
            None => {
                invoices::finish_approval(&mut *conn, ctx, &i).await?;
                out.approved.push(id);
            }
            Some(reason) => out.exceptions.push(RunException {
                invoice_id: id,
                number: i.number,
                org_name: i.org_name,
                reason,
            }),
        }
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("billing_run.approved")
            .target("billing_run", run)
            .details(json!({ "approved": out.approved.len(), "exceptions": out.exceptions.len() })),
    )
    .await?;
    Ok(out)
}

// ---- comments ----

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceComment)]
pub struct Comment {
    pub id: Uuid,
    pub line_id: Option<Uuid>,
    pub line_name: Option<String>,
    pub parent_id: Option<Uuid>,
    pub author_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub body: String,
    pub mentions: Vec<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
    pub resolved_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InvoiceCommentInput)]
pub struct CommentInput {
    pub body: String,
    /// A line of the invoice, or none for the whole invoice.
    #[serde(default)]
    pub line_id: Option<Uuid>,
    /// Replying to a thread (its first comment).
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    /// People @mentioned: they're told.
    #[serde(default)]
    pub mentions: Vec<Uuid>,
}

const COMMENT: &str = "SELECT c.id, c.line_id, c.line_name, c.parent_id, c.author_id, \
        a.display_name AS author_name, c.body, c.mentions, c.resolved_at, \
        r.display_name AS resolved_by_name, c.created_at \
   FROM billing_invoice_comments c LEFT JOIN users a ON a.id = c.author_id \
   LEFT JOIN users r ON r.id = c.resolved_by";

/// An invoice's comments, oldest first.
pub async fn comments(conn: &mut PgConnection, invoice: Uuid) -> BillingResult<Vec<Comment>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{COMMENT} WHERE c.invoice_id = $1 ORDER BY c.created_at, c.id"
    )))
    .bind(invoice)
    .fetch_all(conn)
    .await?)
}

pub async fn comment(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    invoice: Uuid,
    input: &CommentInput,
) -> BillingResult<Comment> {
    let i = invoices::get(&mut *conn, invoice).await?;
    let body = required("body", &input.body, 4000)?;
    let line_name = match input.line_id {
        Some(line) => Some(
            i.lines
                .iter()
                .find(|l| l.id == line)
                .map(|l| l.description.clone())
                .ok_or_else(|| BillingError::invalid("lineId", "not a line of this invoice"))?,
        ),
        None => None,
    };
    if let Some(parent) = input.parent_id {
        let top: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT parent_id FROM billing_invoice_comments WHERE id = $1 AND invoice_id = $2",
        )
        .bind(parent)
        .bind(invoice)
        .fetch_optional(&mut *conn)
        .await?;
        if !matches!(top, Some(None)) {
            return Err(BillingError::invalid(
                "parentId",
                "reply to a thread's first comment",
            ));
        }
    }
    let mut mentions = input.mentions.clone();
    mentions.sort_unstable();
    mentions.dedup();
    let known: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE id = ANY($1) AND status = 'active'")
            .bind(&mentions)
            .fetch_all(&mut *conn)
            .await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO billing_invoice_comments (id, invoice_id, org_id, line_id, line_name, parent_id, author_id, \
            body, mentions) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(invoice)
    .bind(i.org_id)
    .bind(input.line_id)
    .bind(&line_name)
    .bind(input.parent_id)
    .bind(ctx.actor.id())
    .bind(&body)
    .bind(&known)
    .execute(&mut *conn)
    .await?;
    let author = name_of(&mut *conn, ctx.actor.id()).await?;
    let title = format!(
        "{} mentioned you on INV-{} for {}",
        author.as_deref().unwrap_or("Someone"),
        i.number,
        i.org_name
    );
    let link = format!("/billing/invoices/{invoice}#comment-{id}");
    for user in known.iter().filter(|u| Some(**u) != ctx.actor.id()) {
        tell(
            &mut *conn,
            *user,
            kinds::INVOICE_MENTIONED,
            &title,
            &body,
            &link,
        )
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.commented")
            .org(i.org_id)
            .target("invoice", invoice)
            .details(json!({ "comment": id, "line": line_name, "mentions": known })),
    )
    .await?;
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{COMMENT} WHERE c.id = $1")))
        .bind(id)
        .fetch_one(conn)
        .await
        .map_err(Into::into)
}

/// Resolves a thread (or opens it again).
pub async fn resolve(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    invoice: Uuid,
    comment: Uuid,
    resolved: bool,
) -> BillingResult<()> {
    let done = sqlx::query(
        "UPDATE billing_invoice_comments SET resolved_at = CASE WHEN $3 THEN now() END, \
            resolved_by = CASE WHEN $3 THEN $4 END \
          WHERE id = $1 AND invoice_id = $2 AND parent_id IS NULL",
    )
    .bind(comment)
    .bind(invoice)
    .bind(resolved)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(BillingError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: RuleKind, threshold: Option<i64>, expression: Option<&str>) -> RuleInput {
        RuleInput {
            name: "R".into(),
            kind,
            threshold: threshold.map(Decimal::from),
            expression: expression.map(str::to_owned),
            reviewers: "role:admin".into(),
            active: true,
        }
    }

    #[test]
    fn rules_check_their_thresholds_expressions_and_reviewers() {
        assert!(
            check_rule(&rule(
                RuleKind::Expression,
                None,
                Some("invoice.total | float > 5000")
            ))
            .is_ok()
        );
        assert!(check_rule(&rule(RuleKind::Expression, None, Some("invoice.kind ==="))).is_err());
        assert!(check_rule(&rule(RuleKind::Expression, None, None)).is_err());
        assert!(check_rule(&rule(RuleKind::TotalAbove, None, None)).is_err());
        assert!(check_rule(&rule(RuleKind::ChangeAbove, Some(-1), None)).is_err());
        assert!(check_rule(&rule(RuleKind::ChangeAbove, Some(25), None)).is_ok());
        let mut bad = rule(RuleKind::Edited, None, None);
        bad.reviewers = "role:admin, finance".into();
        assert!(check_rule(&bad).is_err());
    }
}
