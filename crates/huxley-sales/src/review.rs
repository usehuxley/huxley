//! Internal quote review (ADR-066).
//!
//! - **Rules** (instance-wide) decide when a quote needs review and who
//!   reviews it: always; a first-year margin below a percent; a first year
//!   above an amount; any line discounted more than a percent; a client's
//!   first quote; a product category; or any expression over the quote.
//!   The quotes they match keep the reasons, refreshed on every save.
//! - **Submitting** a draft for review keeps what was reviewed and raises
//!   `quote.submitted_for_review` (with the reviewers, the reasons and what
//!   changed since the last review) for the review workflow, a built-in
//!   pack on the approval step. The workflow records the decision
//!   ([`decide`]): approved, changes requested or rejected, raising
//!   `quote.approved`, `quote.changes_requested` or `quote.rejected`.
//! - **Sending** a quote that needs review takes an approved review of its
//!   current revision, or the override permission (recorded).
//! - **Comments** on the quote or a line, with `@mentions` that notify;
//!   threads can be resolved.

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

use crate::quotes::{self, LineKind, Quote, Status};
use crate::{SalesError, SalesResult, required};

// ---- rules ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = ReviewRuleKind)]
pub enum RuleKind {
    /// Every quote.
    Always,
    /// First-year margin under `threshold` percent.
    MarginBelow,
    /// First year over `threshold` (in the quote's currency).
    FirstYearAbove,
    /// Any line discounted more than `threshold` percent.
    DiscountAbove,
    /// The client's first quote (none accepted before).
    NewClient,
    /// A line from a product in one of `categories`.
    Category,
    /// `expression` is true of the quote (as `quote`, with its lines and totals).
    Expression,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ReviewRule)]
pub struct Rule {
    pub id: Uuid,
    pub name: String,
    pub kind: RuleKind,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub threshold: Option<Decimal>,
    pub categories: Vec<String>,
    pub expression: Option<String>,
    /// Who reviews: `role:<key>`, `team:<id>`, `user:<id>`, several separated by commas.
    pub reviewers: String,
    pub active: bool,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ReviewRuleInput)]
pub struct RuleInput {
    pub name: String,
    pub kind: RuleKind,
    #[serde(default, deserialize_with = "crate::money_opt")]
    #[schema(value_type = Option<String>)]
    pub threshold: Option<Decimal>,
    #[serde(default)]
    pub categories: Vec<String>,
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

pub async fn rules(conn: &mut PgConnection) -> SalesResult<Vec<Rule>> {
    Ok(sqlx::query_as(
        "SELECT id, name, kind, threshold, categories, expression, reviewers, active \
           FROM sales_review_rules ORDER BY position, created_at",
    )
    .fetch_all(conn)
    .await?)
}

fn check_rule(input: &RuleInput) -> SalesResult<(String, String, Option<String>)> {
    let name = required("name", &input.name, 100)?;
    let reviewers = required("reviewers", &input.reviewers, 500)?;
    for r in reviewers.split(',').map(str::trim) {
        let ok = r.strip_prefix("role:").is_some_and(|k| !k.is_empty())
            || r.strip_prefix("team:")
                .is_some_and(|i| Uuid::parse_str(i).is_ok())
            || r.strip_prefix("user:")
                .is_some_and(|i| Uuid::parse_str(i).is_ok());
        if !ok {
            return Err(SalesError::invalid(
                "reviewers",
                format!("{r:?}: use role:<key>, team:<id> or user:<id>"),
            ));
        }
    }
    let needs_threshold = matches!(
        input.kind,
        RuleKind::MarginBelow | RuleKind::FirstYearAbove | RuleKind::DiscountAbove
    );
    if needs_threshold && input.threshold.is_none() {
        return Err(SalesError::invalid(
            "threshold",
            "required for this kind of rule",
        ));
    }
    if input.kind == RuleKind::Category && input.categories.iter().all(|c| c.trim().is_empty()) {
        return Err(SalesError::invalid(
            "categories",
            "name at least one category",
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
            return Err(SalesError::invalid(
                "expression",
                "required for this kind of rule",
            ));
        };
        // A quick try against an empty quote catches syntax errors.
        let scope = huxley_expr::Scope::new().with("quote", &json!({}));
        if let Err(err) = huxley_expr::Engine::new().eval_condition(&condition(e), &scope)
            && err.to_string().contains("syntax")
        {
            return Err(SalesError::invalid("expression", err.to_string()));
        }
    }
    Ok((name, reviewers, expression))
}

pub async fn create_rule(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &RuleInput,
) -> SalesResult<Rule> {
    let (name, reviewers, expression) = check_rule(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_review_rules (id, name, kind, threshold, categories, expression, reviewers, \
            active, position) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, (SELECT coalesce(max(position), 0) + 1 FROM sales_review_rules))",
    )
    .bind(id)
    .bind(&name)
    .bind(input.kind)
    .bind(input.threshold)
    .bind(&input.categories)
    .bind(&expression)
    .bind(&reviewers)
    .bind(input.active)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_review_rule.created")
            .target("sales_review_rule", id)
            .details(json!({ "name": name, "kind": input.kind })),
    )
    .await?;
    rule(conn, id).await
}

async fn rule(conn: &mut PgConnection, id: Uuid) -> SalesResult<Rule> {
    sqlx::query_as(
        "SELECT id, name, kind, threshold, categories, expression, reviewers, active \
           FROM sales_review_rules WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(SalesError::NotFound)
}

pub async fn update_rule(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &RuleInput,
) -> SalesResult<Rule> {
    let (name, reviewers, expression) = check_rule(input)?;
    let done = sqlx::query(
        "UPDATE sales_review_rules SET name = $2, kind = $3, threshold = $4, categories = $5, \
            expression = $6, reviewers = $7, active = $8 WHERE id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(input.kind)
    .bind(input.threshold)
    .bind(&input.categories)
    .bind(&expression)
    .bind(&reviewers)
    .bind(input.active)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::NotFound);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_review_rule.updated")
            .target("sales_review_rule", id)
            .details(json!({ "name": name, "kind": input.kind, "active": input.active })),
    )
    .await?;
    rule(conn, id).await
}

pub async fn delete_rule(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> SalesResult<()> {
    let done = sqlx::query("DELETE FROM sales_review_rules WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sales_review_rule.deleted").target("sales_review_rule", id),
    )
    .await?;
    Ok(())
}

// ---- matching ----

/// A rule a quote matches, and why.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Reason {
    pub rule_id: Uuid,
    pub name: String,
    pub detail: String,
    pub reviewers: String,
}

/// The active rules a quote matches.
pub async fn reasons(conn: &mut PgConnection, q: &Quote) -> SalesResult<Vec<Reason>> {
    let all = rules(&mut *conn).await?;
    let mut out = Vec::new();
    let items: Vec<_> = q
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Item && (!l.optional || l.selected))
        .collect();
    for r in all.into_iter().filter(|r| r.active) {
        let detail = match r.kind {
            RuleKind::Always => Some("every quote is reviewed".to_owned()),
            RuleKind::MarginBelow => {
                let floor = r.threshold.unwrap_or_default();
                match q.totals.margin_pct {
                    Some(m) if m < floor => {
                        Some(format!("margin {m}% is under {}%", floor.normalize()))
                    }
                    None if !items.is_empty() => Some("no margin (nothing priced)".into()),
                    _ => None,
                }
            }
            RuleKind::FirstYearAbove => {
                let limit = r.threshold.unwrap_or_default();
                (q.totals.first_year > limit).then(|| {
                    format!(
                        "first year {} {} is over {}",
                        q.totals.first_year.normalize(),
                        q.currency,
                        limit.normalize()
                    )
                })
            }
            RuleKind::DiscountAbove => {
                let limit = r.threshold.unwrap_or_default();
                items
                    .iter()
                    .filter(|l| l.discount_pct > limit)
                    .max_by_key(|l| l.discount_pct)
                    .map(|l| {
                        format!(
                            "{} is {}% off (over {}%)",
                            l.name,
                            l.discount_pct.normalize(),
                            limit.normalize()
                        )
                    })
            }
            RuleKind::NewClient => {
                let before: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM sales_quotes WHERE org_id = $1 AND status = 'accepted' AND id <> $2)",
                )
                .bind(q.org_id)
                .bind(q.id)
                .fetch_one(&mut *conn)
                .await?;
                (!before).then(|| "the client's first quote".to_owned())
            }
            RuleKind::Category => {
                let ids: Vec<Uuid> = items.iter().filter_map(|l| l.product_id).collect();
                let found: Vec<String> = sqlx::query_scalar(
                    "SELECT DISTINCT category FROM catalog_products \
                      WHERE id = ANY($1) AND lower(category) = ANY($2)",
                )
                .bind(&ids)
                .bind(
                    r.categories
                        .iter()
                        .map(|c| c.trim().to_lowercase())
                        .collect::<Vec<_>>(),
                )
                .fetch_all(&mut *conn)
                .await?;
                (!found.is_empty()).then(|| format!("includes {}", found.join(", ")))
            }
            RuleKind::Expression => {
                let quote = serde_json::to_value(q).unwrap_or(Json::Null);
                let scope = huxley_expr::Scope::new().with("quote", &quote);
                let expr = r.expression.as_deref().unwrap_or("false");
                huxley_expr::Engine::new()
                    .eval_condition(&condition(expr), &scope)
                    .unwrap_or(false)
                    .then(|| format!("matches {expr}"))
            }
        };
        if let Some(detail) = detail {
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

/// Works out the quote's reasons again after a change, and whether its
/// review still stands: a change after review (a new revision, or any save
/// while in review) needs a new one.
pub async fn refresh(conn: &mut PgConnection, id: Uuid, changed: bool) -> SalesResult<()> {
    let q = quotes::get(&mut *conn, id).await?;
    let found = reasons(&mut *conn, &q).await?;
    let (status, revision): (ReviewStatus, Option<i32>) =
        sqlx::query_as("SELECT review_status, review_revision FROM sales_quotes WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    let stale = changed && status != ReviewStatus::NotNeeded && status != ReviewStatus::Needed;
    let stale = stale || (status == ReviewStatus::Approved && revision != Some(q.revision));
    let next = match (found.is_empty(), status) {
        (true, ReviewStatus::Needed | ReviewStatus::NotNeeded) => ReviewStatus::NotNeeded,
        (false, ReviewStatus::NotNeeded) => ReviewStatus::Needed,
        _ if stale => {
            if found.is_empty() {
                ReviewStatus::NotNeeded
            } else {
                ReviewStatus::Needed
            }
        }
        (_, s) => s,
    };
    sqlx::query("UPDATE sales_quotes SET review_reasons = $2, review_status = $3 WHERE id = $1")
        .bind(id)
        .bind(serde_json::to_value(&found).unwrap_or(Json::Array(Vec::new())))
        .bind(next)
        .execute(conn)
        .await?;
    Ok(())
}

/// The review state of a quote, for its page.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewState {
    pub status: ReviewStatus,
    pub reasons: Vec<Reason>,
    /// It can be sent: no review needed, or one approved for this revision.
    pub can_send: bool,
    pub sent_overridden: bool,
    /// Each submission, newest first.
    pub history: Vec<ReviewRecord>,
    /// What changed since the latest decided review.
    pub changes: Option<Changes>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
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

async fn history(conn: &mut PgConnection, id: Uuid) -> SalesResult<Vec<ReviewRecord>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.revision, r.reasons, r.reviewers, u.display_name AS submitted_by_name, \
                r.submitted_at, r.decision, r.decided_by, r.decided_at, r.comment \
           FROM sales_quote_reviews r LEFT JOIN users u ON u.id = r.submitted_by \
          WHERE r.quote_id = $1 ORDER BY r.submitted_at DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

pub async fn state(conn: &mut PgConnection, id: Uuid) -> SalesResult<ReviewState> {
    let q = quotes::get(&mut *conn, id).await?;
    let (status, revision, reasons, overridden): (ReviewStatus, Option<i32>, Json, bool) =
        sqlx::query_as(
            "SELECT review_status, review_revision, review_reasons, sent_overridden FROM sales_quotes WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let reasons: Vec<Reason> = serde_json::from_value(reasons).unwrap_or_default();
    Ok(ReviewState {
        status,
        can_send: reasons.is_empty()
            || (status == ReviewStatus::Approved && revision == Some(q.revision)),
        reasons,
        sent_overridden: overridden,
        history: history(&mut *conn, id).await?,
        changes: changes(&mut *conn, &q).await?,
    })
}

/// The open approval for a quote's review (from the review workflow), and
/// whether `user` may decide it: `(approval id, user may decide)`.
pub async fn open_approval(
    conn: &mut PgConnection,
    quote: Uuid,
    user: Uuid,
) -> SalesResult<Option<(Uuid, bool)>> {
    Ok(sqlx::query_as(
        "SELECT a.id, $2 = ANY(a.approvers) FROM approvals a JOIN runs r ON r.id = a.run_id \
          WHERE a.status = 'pending' AND a.expires_at > now() \
            AND r.inputs->>'reviewId' = (SELECT id::text FROM sales_quote_reviews WHERE quote_id = $1 \
                                          ORDER BY submitted_at DESC LIMIT 1) \
          ORDER BY a.created_at DESC LIMIT 1",
    )
    .bind(quote)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// Whether a quote may be sent now, without an override.
pub async fn may_send(conn: &mut PgConnection, q: &Quote) -> SalesResult<bool> {
    let (status, revision, reasons): (ReviewStatus, Option<i32>, Json) = sqlx::query_as(
        "SELECT review_status, review_revision, review_reasons FROM sales_quotes WHERE id = $1",
    )
    .bind(q.id)
    .fetch_one(conn)
    .await?;
    let none = reasons.as_array().is_none_or(Vec::is_empty);
    Ok(none || (status == ReviewStatus::Approved && revision == Some(q.revision)))
}

// ---- what changed ----

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub field: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LineChange {
    pub name: String,
    pub fields: Vec<FieldChange>,
}

/// What changed since the last decided review.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub since_revision: i32,
    pub decision: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<LineChange>,
    pub totals: Vec<FieldChange>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.totals.is_empty()
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
        for t in &self.totals {
            parts.push(format!("{} {} → {}", t.field, t.before, t.after));
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

/// Compares the quote with the snapshot of its latest decided review.
async fn changes(conn: &mut PgConnection, q: &Quote) -> SalesResult<Option<Changes>> {
    let last: Option<(i32, String, Json)> = sqlx::query_as(
        "SELECT revision, decision, snapshot FROM sales_quote_reviews \
          WHERE quote_id = $1 AND decision IS NOT NULL ORDER BY decided_at DESC LIMIT 1",
    )
    .bind(q.id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((since, decision, before)) = last else {
        return Ok(None);
    };
    let now = serde_json::to_value(q).unwrap_or(Json::Null);
    let lines = |doc: &Json| -> BTreeMap<String, Json> {
        doc["lines"]
            .as_array()
            .map(|ls| {
                ls.iter()
                    .filter(|l| l["kind"] == "item")
                    .map(|l| (text_of(&l["id"]), l.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (old, new) = (lines(&before), lines(&now));
    let mut out = Changes {
        since_revision: since,
        decision,
        added: new
            .iter()
            .filter(|(id, _)| !old.contains_key(*id))
            .map(|(_, l)| text_of(&l["name"]))
            .collect(),
        removed: old
            .iter()
            .filter(|(id, _)| !new.contains_key(*id))
            .map(|(_, l)| text_of(&l["name"]))
            .collect(),
        changed: Vec::new(),
        totals: Vec::new(),
    };
    for (id, n) in &new {
        let Some(o) = old.get(id) else { continue };
        let fields: Vec<FieldChange> = [
            ("name", "name"),
            ("quantity", "quantity"),
            ("unit price", "unitPrice"),
            ("discount %", "discountPct"),
            ("unit cost", "unitCost"),
            ("billing", "billing"),
            ("option", "optional"),
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
                name: text_of(&n["name"]),
                fields,
            });
        }
    }
    for (field, key) in [
        ("one-time", "oneTime"),
        ("monthly", "monthly"),
        ("margin %", "marginPct"),
    ] {
        let (b, a) = (&before["totals"][key], &now["totals"][key]);
        if b != a {
            out.totals.push(FieldChange {
                field: field.into(),
                before: text_of(b),
                after: text_of(a),
            });
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

// ---- submitting and deciding ----

/// Submits a draft for review: keeps what's reviewed and raises
/// `quote.submitted_for_review` for the review workflow.
pub async fn submit(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
) -> SalesResult<ReviewState> {
    refresh(&mut *conn, id, false).await?;
    let q = quotes::get(&mut *conn, id).await?;
    if q.version != version {
        return Err(SalesError::VersionConflict);
    }
    if q.status != Status::Draft {
        return Err(SalesError::Conflict(
            "only a draft can be submitted for review".into(),
        ));
    }
    let current = state(&mut *conn, id).await?;
    if current.status == ReviewStatus::InReview {
        return Err(SalesError::Conflict("it's already in review".into()));
    }
    if !q.lines.iter().any(|l| l.kind == LineKind::Item) {
        return Err(SalesError::invalid("lines", "add something to quote first"));
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
    let review = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_quote_reviews (id, quote_id, org_id, revision, snapshot, reasons, reviewers, submitted_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(review)
    .bind(id)
    .bind(q.org_id)
    .bind(q.revision)
    .bind(serde_json::to_value(&q).unwrap_or(Json::Null))
    .bind(serde_json::to_value(&current.reasons).unwrap_or(Json::Null))
    .bind(&reviewers)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE sales_quotes SET review_status = 'in_review', review_revision = revision WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("quote.submitted_for_review")
            .org(q.org_id)
            .target("quote", id)
            .details(json!({ "revision": q.revision, "reasons": current.reasons, "reviewers": reviewers })),
    )
    .await?;
    let submitter: Option<String> = match ctx.actor.id() {
        Some(u) => {
            sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
                .bind(u)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "quote.submitted_for_review",
            org_id: Some(q.org_id),
            subject_type: "quote",
            subject_id: id,
            payload: json!({
                "quoteId": id,
                "reviewId": review,
                "reference": format!("Q-{}", q.number),
                "title": q.title,
                "client": q.org_name,
                "orgId": q.org_id,
                "revision": q.revision,
                "currency": q.currency,
                "oneTime": q.totals.one_time.normalize().to_string(),
                "monthly": q.totals.monthly.normalize().to_string(),
                "firstYear": q.totals.first_year.normalize().to_string(),
                "marginPct": q.totals.margin_pct.map(|m| m.normalize().to_string()),
                "reasons": current.reasons.iter().map(|r| format!("{}: {}", r.name, r.detail)).collect::<Vec<_>>(),
                "reviewers": reviewers,
                "submittedBy": submitter,
                "ownerId": q.owner_id,
                "changes": current.changes.as_ref().map(Changes::summary),
                "link": format!("/sales/quotes/{id}"),
            }),
        },
    )
    .await?;
    state(conn, id).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
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
            Self::Approved => "quote.approved",
            Self::ChangesRequested => "quote.changes_requested",
            Self::Rejected => "quote.rejected",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::Rejected => "rejected",
        }
    }
}

/// Records a review's decision (from the review workflow). Only the
/// quote's open review can be decided; a stale one (the quote changed
/// since) is ignored and says so.
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    review: Uuid,
    decision: Decision,
    by: &str,
    comment: Option<&str>,
) -> SalesResult<ReviewState> {
    let row: Option<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT quote_id, decision FROM sales_quote_reviews WHERE id = $1")
            .bind(review)
            .fetch_optional(&mut *conn)
            .await?;
    let (id, already) = row.ok_or(SalesError::NotFound)?;
    if already.is_some() {
        return Err(SalesError::Conflict(
            "this review was already decided".into(),
        ));
    }
    let latest: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sales_quote_reviews WHERE quote_id = $1 ORDER BY submitted_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let status: ReviewStatus =
        sqlx::query_scalar("SELECT review_status FROM sales_quotes WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    if latest != Some(review) || status != ReviewStatus::InReview {
        return Err(SalesError::Conflict(
            "the quote changed since it was submitted; it needs a new review".into(),
        ));
    }
    let by = required("by", by, 200)?;
    let comment = crate::text("comment", comment, 2000)?;
    sqlx::query(
        "UPDATE sales_quote_reviews SET decision = $2, decided_by = $3, decided_at = now(), comment = $4 WHERE id = $1",
    )
    .bind(review)
    .bind(decision.as_str())
    .bind(&by)
    .bind(&comment)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE sales_quotes SET review_status = $2 WHERE id = $1")
        .bind(id)
        .bind(decision.status())
        .execute(&mut *conn)
        .await?;
    let q = quotes::get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("quote.reviewed")
            .org(q.org_id)
            .target("quote", id)
            .details(json!({ "decision": decision, "by": by, "comment": comment, "revision": q.revision })),
    )
    .await?;
    let event = decision.event();
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: event,
            org_id: Some(q.org_id),
            subject_type: "quote",
            subject_id: id,
            payload: json!({
                "quoteId": id, "reviewId": review, "reference": format!("Q-{}", q.number),
                "title": q.title, "orgId": q.org_id, "revision": q.revision,
                "by": by, "comment": comment, "ownerId": q.owner_id,
            }),
        },
    )
    .await?;
    if let Some(owner) = q.owner_id {
        let title = match decision {
            Decision::Approved => format!("Q-{} {} was approved", q.number, q.title),
            Decision::ChangesRequested => {
                format!("Changes requested on Q-{} {}", q.number, q.title)
            }
            Decision::Rejected => format!("Q-{} {} was rejected", q.number, q.title),
        };
        let body = comment
            .clone()
            .map_or_else(|| format!("By {by}."), |c| format!("{by}: {c}"));
        tell(
            &mut *conn,
            owner,
            kinds::QUOTE_REVIEWED,
            &title,
            &body,
            &format!("/sales/quotes/{id}"),
        )
        .await?;
    }
    state(conn, id).await
}

/// Notifies someone about a quote.
async fn tell(
    conn: &mut PgConnection,
    user_id: Uuid,
    kind: &'static str,
    title: &str,
    body: &str,
    link: &str,
) -> SalesResult<()> {
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
        huxley_notify::NotifyError::Db(e) => SalesError::Sqlx(e),
        other => SalesError::Conflict(other.to_string()),
    })?;
    Ok(())
}

/// Records a send without an approved review (the caller checked the
/// permission).
pub(crate) async fn record_override(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    q: &Quote,
) -> SalesResult<()> {
    sqlx::query("UPDATE sales_quotes SET sent_overridden = true WHERE id = $1")
        .bind(q.id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("quote.review_overridden")
            .org(q.org_id)
            .target("quote", q.id)
            .details(json!({ "revision": q.revision })),
    )
    .await?;
    Ok(())
}

// ---- comments ----

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
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
pub struct CommentInput {
    pub body: String,
    /// A line of the quote, or none for the whole quote.
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
   FROM sales_quote_comments c LEFT JOIN users a ON a.id = c.author_id \
   LEFT JOIN users r ON r.id = c.resolved_by";

/// A quote's comments, oldest first.
pub async fn comments(conn: &mut PgConnection, quote: Uuid) -> SalesResult<Vec<Comment>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{COMMENT} WHERE c.quote_id = $1 ORDER BY c.created_at, c.id"
    )))
    .bind(quote)
    .fetch_all(conn)
    .await?)
}

pub async fn comment(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    quote: Uuid,
    input: &CommentInput,
) -> SalesResult<Comment> {
    let q = quotes::get(&mut *conn, quote).await?;
    let body = required("body", &input.body, 4000)?;
    let line_name = match input.line_id {
        Some(line) => Some(
            q.lines
                .iter()
                .find(|l| l.id == line)
                .map(|l| l.name.clone())
                .ok_or_else(|| SalesError::invalid("lineId", "not a line of this quote"))?,
        ),
        None => None,
    };
    if let Some(parent) = input.parent_id {
        let top: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT parent_id FROM sales_quote_comments WHERE id = $1 AND quote_id = $2",
        )
        .bind(parent)
        .bind(quote)
        .fetch_optional(&mut *conn)
        .await?;
        if !matches!(top, Some(None)) {
            return Err(SalesError::invalid(
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
        "INSERT INTO sales_quote_comments (id, quote_id, org_id, line_id, line_name, parent_id, author_id, body, mentions) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(quote)
    .bind(q.org_id)
    .bind(input.line_id)
    .bind(&line_name)
    .bind(input.parent_id)
    .bind(ctx.actor.id())
    .bind(&body)
    .bind(&known)
    .execute(&mut *conn)
    .await?;
    let author: Option<String> = match ctx.actor.id() {
        Some(u) => {
            sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
                .bind(u)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let title = format!(
        "{} mentioned you on Q-{} {}",
        author.as_deref().unwrap_or("Someone"),
        q.number,
        q.title
    );
    let link = format!("/sales/quotes/{quote}#comment-{id}");
    for user in known.iter().filter(|u| Some(**u) != ctx.actor.id()) {
        tell(
            &mut *conn,
            *user,
            kinds::QUOTE_MENTIONED,
            &title,
            &body,
            &link,
        )
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("quote.commented")
            .org(q.org_id)
            .target("quote", quote)
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
    quote: Uuid,
    comment: Uuid,
    resolved: bool,
) -> SalesResult<()> {
    let done = sqlx::query(
        "UPDATE sales_quote_comments SET resolved_at = CASE WHEN $3 THEN now() END, \
            resolved_by = CASE WHEN $3 THEN $4 END \
          WHERE id = $1 AND quote_id = $2 AND parent_id IS NULL",
    )
    .bind(comment)
    .bind(quote)
    .bind(resolved)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: RuleKind, expression: Option<&str>) -> RuleInput {
        RuleInput {
            name: "R".into(),
            kind,
            threshold: None,
            categories: Vec::new(),
            expression: expression.map(str::to_owned),
            reviewers: "role:admin".into(),
            active: true,
        }
    }

    #[test]
    fn rules_check_their_expressions_and_reviewers() {
        assert!(
            check_rule(&rule(
                RuleKind::Expression,
                Some("quote.totals.oneTime | float > 5000")
            ))
            .is_ok()
        );
        assert!(
            check_rule(&rule(
                RuleKind::Expression,
                Some("{{ quote.title == 'x' }}")
            ))
            .is_ok()
        );
        assert!(check_rule(&rule(RuleKind::Expression, Some("quote.title ==="))).is_err());
        assert!(check_rule(&rule(RuleKind::Expression, None)).is_err());
        assert!(check_rule(&rule(RuleKind::MarginBelow, None)).is_err());
        assert!(check_rule(&rule(RuleKind::Category, None)).is_err());
        let mut bad = rule(RuleKind::Always, None);
        bad.reviewers = "role:admin, admins".into();
        assert!(check_rule(&bad).is_err());
        bad.reviewers = format!("role:admin, user:{}", Uuid::nil());
        assert!(check_rule(&bad).is_ok());
    }

    #[test]
    fn expressions_become_conditions() {
        assert_eq!(condition("a == 1"), "{{ a == 1 }}");
        assert_eq!(condition("{{ a }}"), "{{ a }}");
    }
}
