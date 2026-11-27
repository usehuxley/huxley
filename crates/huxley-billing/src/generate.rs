//! What's billable for a client in a period, and billing runs that turn it
//! into draft invoices.
//!
//! - **Time**: entries worked in the period, not billed, not on fixed-fee
//!   or milestone projects (and, if the settings say so, only on approved
//!   timesheets), evaluated for agreement coverage; what's billable is
//!   billed at the agreement's rate, or the provider's default. Lines group
//!   a ticket's time by rate. Not-to-exceed projects stop at their cap.
//! - **Products**: one-time lines of sales orders, delivered in full.
//! - **Recurring**: active agreements' lines, per calendar month, quarter
//!   or year that starts in the period (or, for the first, the one the
//!   agreement starts in), prorated by days for a part period.
//! - **Projects**: fixed fees once the project is completed; milestones
//!   once reached.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Date, Month, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::invoices::LineKind;
use crate::{BillingError, BillingResult};

/// One thing to bill, and the sources it bills.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BillableLine)]
pub struct Candidate {
    pub kind: LineKind,
    pub description: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub unit: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub unit_price: Decimal,
    pub product_id: Option<Uuid>,
    pub currency: String,
    #[serde(skip)]
    pub sources: Vec<Source>,
}

impl Candidate {
    pub fn amount(&self) -> Decimal {
        (self.quantity * self.unit_price).round_dp(2)
    }
}

#[derive(Debug, Clone)]
pub struct Source {
    pub kind: &'static str,
    pub id: Uuid,
    pub period: Option<Date>,
    pub amount: Decimal,
}

/// What's billable for a client in a period (nothing is written).
/// Run in system scope.
pub async fn candidates(
    conn: &mut PgConnection,
    client: Uuid,
    from: Date,
    to: Date,
) -> BillingResult<Vec<Candidate>> {
    if to < from {
        return Err(BillingError::invalid("to", "ends before it starts"));
    }
    let settings = crate::settings::for_client(&mut *conn, client).await?;
    let mut out = time(&mut *conn, client, from, to, &settings).await?;
    out.extend(products(&mut *conn, client).await?);
    out.extend(recurring(&mut *conn, client, from, to).await?);
    out.extend(projects(&mut *conn, client, to, &settings.currency).await?);
    out.extend(expenses(&mut *conn, client, to).await?);
    Ok(out)
}

/// Approved billable expenses up to `to`, marked up, one line each.
/// Expenses on fixed-fee or milestone projects are in the fee.
async fn expenses(
    conn: &mut PgConnection,
    client: Uuid,
    to: Date,
) -> BillingResult<Vec<Candidate>> {
    let rows: Vec<(Uuid, String, String, Date, Decimal, String)> = sqlx::query_as(
        "SELECT e.id, c.name, e.description, e.spent_on, round(e.amount * (1 + e.markup_pct / 100), 2), e.currency \
           FROM exp_entries e JOIN exp_categories c ON c.id = e.category_id \
           LEFT JOIN prj_projects p ON p.id = e.project_id \
          WHERE e.org_id = $1 AND e.billable AND e.status IN ('approved', 'reimbursed') AND e.spent_on <= $2 \
            AND (p.id IS NULL OR p.billing_method NOT IN ('fixed_fee', 'milestone')) \
            AND NOT EXISTS (SELECT 1 FROM billing_billed b WHERE b.source_type = 'expense' AND b.source_id = e.id) \
          ORDER BY e.spent_on, e.created_at",
    )
    .bind(client)
    .bind(to)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, category, description, day, price, currency)| Candidate {
                kind: LineKind::Expense,
                description: format!("{category}: {description} ({day})"),
                quantity: Decimal::ONE,
                unit: "each".into(),
                unit_price: price,
                product_id: None,
                currency,
                sources: vec![Source {
                    kind: "expense",
                    id,
                    period: None,
                    amount: price,
                }],
            },
        )
        .collect())
}

async fn time(
    conn: &mut PgConnection,
    client: Uuid,
    from: Date,
    to: Date,
    settings: &crate::settings::Settings,
) -> BillingResult<Vec<Candidate>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT e.id FROM sd_time_entries e \
           LEFT JOIN prj_tasks pt ON pt.ticket_id = e.ticket_id LEFT JOIN prj_projects p ON p.id = pt.project_id \
          WHERE e.org_id = $1 AND e.worked_on BETWEEN $2 AND $3 \
            AND NOT EXISTS (SELECT 1 FROM billing_billed b WHERE b.source_type = 'time_entry' AND b.source_id = e.id) \
            AND (p.id IS NULL OR p.billing_method IN ('time_materials', 'not_to_exceed', 'agreement')) \
            AND (NOT $4 OR EXISTS (SELECT 1 FROM wf_timesheets ts WHERE ts.user_id = e.user_id AND ts.status = 'approved' \
                                     AND e.worked_on BETWEEN ts.period_start AND ts.period_end)) \
          ORDER BY e.worked_on, e.created_at",
    )
    .bind(client)
    .bind(from)
    .bind(to)
    .bind(settings.approved_time_only)
    .fetch_all(&mut *conn)
    .await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let evaluated = huxley_contracts::coverage::evaluate(&mut *conn, &ids).await?;
    // Not-to-exceed caps: what's left on each such project.
    let caps: Vec<(Uuid, Uuid, Decimal)> = sqlx::query_as(
        "SELECT pt.ticket_id, p.id, greatest(p.fee_amount - coalesce((SELECT sum(b.amount) FROM billing_billed b \
              JOIN sd_time_entries x ON x.id = b.source_id JOIN prj_tasks y ON y.ticket_id = x.ticket_id \
             WHERE b.source_type = 'time_entry' AND y.project_id = p.id), 0), 0) \
           FROM prj_tasks pt JOIN prj_projects p ON p.id = pt.project_id \
          WHERE p.billing_method = 'not_to_exceed' AND p.fee_amount IS NOT NULL AND p.org_id = $1",
    )
    .bind(client)
    .fetch_all(&mut *conn)
    .await?;
    let mut left: BTreeMap<Uuid, Decimal> = caps.iter().map(|(_, p, l)| (*p, *l)).collect();
    let mut lines: Vec<(Option<Uuid>, Decimal, Candidate)> = Vec::new();
    for ev in evaluated {
        if ev.billable_minutes <= 0 {
            continue;
        }
        let Some(rate) = ev.hourly_rate.or(settings.hourly_rate) else {
            continue;
        };
        let mut hours = (Decimal::from(ev.billable_minutes) / Decimal::from(60)).round_dp(4);
        let mut amount = (hours * rate).round_dp(2);
        if let Some((_, project, _)) = caps.iter().find(|(t, _, _)| Some(*t) == ev.ticket_id) {
            let remaining = left.get(project).copied().unwrap_or_default();
            if remaining <= Decimal::ZERO {
                continue;
            }
            if amount > remaining {
                amount = remaining;
                hours = if rate.is_zero() {
                    hours
                } else {
                    (remaining / rate).round_dp(4)
                };
            }
            left.insert(*project, remaining - amount);
        }
        let currency = ev
            .currency
            .clone()
            .unwrap_or_else(|| settings.currency.clone());
        let source = Source {
            kind: "time_entry",
            id: ev.time_entry_id,
            period: None,
            amount,
        };
        if let Some((_, _, c)) = lines
            .iter_mut()
            .find(|(t, r, c)| *t == ev.ticket_id && *r == rate && c.currency == currency)
        {
            c.quantity += hours;
            c.sources.push(source);
        } else {
            let c = time_line(&mut *conn, &ev, rate, hours, currency, source).await?;
            lines.push((ev.ticket_id, rate, c));
        }
    }
    Ok(lines
        .into_iter()
        .map(|(_, _, mut c)| {
            c.quantity = c.quantity.round_dp(2);
            c
        })
        .collect())
}

/// A new time line for a ticket at a rate.
async fn time_line(
    conn: &mut PgConnection,
    ev: &huxley_contracts::coverage::Evaluation,
    rate: Decimal,
    hours: Decimal,
    currency: String,
    source: Source,
) -> BillingResult<Candidate> {
    let what = ev
        .ticket_reference
        .clone()
        .unwrap_or_else(|| "Time".to_owned());
    let title: Option<String> = match ev.ticket_id {
        Some(t) => {
            sqlx::query_scalar("SELECT title FROM sd_tickets WHERE id = $1")
                .bind(t)
                .fetch_optional(conn)
                .await?
        }
        None => None,
    };
    Ok(Candidate {
        kind: LineKind::Time,
        description: title.map_or(what.clone(), |t| format!("{what} {t}")),
        quantity: hours,
        unit: "hour".into(),
        unit_price: rate,
        product_id: None,
        currency,
        sources: vec![source],
    })
}

#[derive(sqlx::FromRow)]
struct OrderLine {
    id: Uuid,
    name: String,
    quantity: Decimal,
    unit: String,
    unit_price: Decimal,
    discount_pct: Decimal,
    product_id: Option<Uuid>,
    currency: String,
    number: i64,
}

async fn products(conn: &mut PgConnection, client: Uuid) -> BillingResult<Vec<Candidate>> {
    let rows: Vec<OrderLine> = sqlx::query_as(
        "SELECT l.id, l.name, l.quantity, l.unit, l.unit_price, l.discount_pct, l.product_id, so.currency, so.number \
           FROM sales_order_lines l JOIN sales_orders so ON so.id = l.order_id \
          WHERE so.org_id = $1 AND so.status IN ('open', 'fulfilled', 'closed') AND l.billing = 'one_time' \
            AND l.fulfilled >= l.quantity \
            AND NOT EXISTS (SELECT 1 FROM billing_billed b WHERE b.source_type = 'sales_order_line' AND b.source_id = l.id) \
          ORDER BY so.number, l.position",
    )
    .bind(client)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |OrderLine {
                 id,
                 name,
                 quantity,
                 unit,
                 unit_price: price,
                 discount_pct: discount,
                 product_id: product,
                 currency,
                 number,
             }| {
                let unit_price =
                    (price * (Decimal::ONE_HUNDRED - discount) / Decimal::ONE_HUNDRED).round_dp(4);
                let c = Candidate {
                    kind: LineKind::Product,
                    description: format!("{name} (SO-{number})"),
                    quantity,
                    unit,
                    unit_price,
                    product_id: product,
                    currency,
                    sources: Vec::new(),
                };
                let amount = c.amount();
                Candidate {
                    sources: vec![Source {
                        kind: "sales_order_line",
                        id,
                        period: None,
                        amount,
                    }],
                    ..c
                }
            },
        )
        .collect())
}

/// The first day of the calendar period (of `months`) holding `d`.
fn period_start(d: Date, months: u8) -> Date {
    let m = u8::from(d.month());
    let first = (m - 1) / months * months + 1;
    Date::from_calendar_date(
        d.year(),
        Month::try_from(first).unwrap_or(Month::January),
        1,
    )
    .unwrap_or(d)
}

fn add_months(d: Date, months: u8) -> Date {
    let total = i32::from(u8::from(d.month())) - 1 + i32::from(months);
    let year = d.year() + total.div_euclid(12);
    let month = u8::try_from(total.rem_euclid(12) + 1).unwrap_or(1);
    Date::from_calendar_date(year, Month::try_from(month).unwrap_or(Month::January), 1).unwrap_or(d)
}

async fn recurring(
    conn: &mut PgConnection,
    client: Uuid,
    from: Date,
    to: Date,
) -> BillingResult<Vec<Candidate>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        description: String,
        quantity: Decimal,
        price: Option<Decimal>,
        billing: String,
        product_id: Option<Uuid>,
        starts_on: Date,
        ends_on: Option<Date>,
        currency: String,
        agreement: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT l.id, l.description, l.quantity, coalesce(l.unit_price, p.price) AS price, l.billing, l.product_id, \
                a.starts_on, a.ends_on, a.currency, a.name AS agreement \
           FROM contract_lines l JOIN contract_agreements a ON a.id = l.agreement_id \
           LEFT JOIN catalog_products p ON p.id = l.product_id \
          WHERE a.org_id = $1 AND a.status = 'active' ORDER BY a.name, l.position",
    )
    .bind(client)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let Some(price) = r.price else { continue };
        let months: u8 = match r.billing.as_str() {
            "quarterly" => 3,
            "annual" => 12,
            _ => 1,
        };
        // Calendar periods from the one the agreement starts in.
        let mut p = period_start(r.starts_on, months);
        while p <= to {
            let next = add_months(p, months);
            let end = next.previous_day().unwrap_or(next);
            let starts_here = r.starts_on >= p && r.starts_on <= end;
            let due = (p >= from) || (starts_here && r.starts_on >= from);
            let active_from = r.starts_on.max(p);
            let active_to = r.ends_on.map_or(end, |e| e.min(end));
            if due && active_from <= active_to {
                let billed: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM billing_billed WHERE source_type = 'agreement_line' AND source_id = $1 AND period = $2)",
                )
                .bind(r.id)
                .bind(p)
                .fetch_one(&mut *conn)
                .await?;
                if !billed {
                    let days = Decimal::from((end - p).whole_days() + 1);
                    let active = Decimal::from((active_to - active_from).whole_days() + 1);
                    let fraction = if active >= days {
                        Decimal::ONE
                    } else {
                        (active / days).round_dp(4)
                    };
                    let label = match months {
                        1 => format!("{} {}", p.month(), p.year()),
                        3 => format!("Q{} {}", (u8::from(p.month()) - 1) / 3 + 1, p.year()),
                        _ => format!("{}", p.year()),
                    };
                    let prorated = if fraction < Decimal::ONE {
                        format!(", {active} of {days} days")
                    } else {
                        String::new()
                    };
                    let c = Candidate {
                        kind: LineKind::Recurring,
                        description: format!(
                            "{}: {label}{prorated} ({})",
                            r.description, r.agreement
                        ),
                        quantity: (r.quantity * fraction).round_dp(4),
                        unit: "each".into(),
                        unit_price: price,
                        product_id: r.product_id,
                        currency: r.currency.clone(),
                        sources: Vec::new(),
                    };
                    let amount = c.amount();
                    out.push(Candidate {
                        sources: vec![Source {
                            kind: "agreement_line",
                            id: r.id,
                            period: Some(p),
                            amount,
                        }],
                        ..c
                    });
                }
            }
            if r.ends_on.is_some_and(|e| e < next) {
                break;
            }
            p = next;
        }
    }
    Ok(out)
}

async fn projects(
    conn: &mut PgConnection,
    client: Uuid,
    to: Date,
    currency: &str,
) -> BillingResult<Vec<Candidate>> {
    let mut out = Vec::new();
    let fees: Vec<(Uuid, String, Decimal, String)> = sqlx::query_as(
        "SELECT p.id, p.name, p.fee_amount, p.fee_currency FROM prj_projects p \
          WHERE p.org_id = $1 AND p.billing_method = 'fixed_fee' AND p.status = 'completed' AND p.fee_amount > 0 \
            AND NOT EXISTS (SELECT 1 FROM billing_billed b WHERE b.source_type = 'project_fee' AND b.source_id = p.id)",
    )
    .bind(client)
    .fetch_all(&mut *conn)
    .await?;
    for (id, name, fee, cur) in fees {
        out.push(Candidate {
            kind: LineKind::Project,
            description: format!("{name}: fixed fee"),
            quantity: Decimal::ONE,
            unit: "each".into(),
            unit_price: fee,
            product_id: None,
            currency: cur,
            sources: vec![Source {
                kind: "project_fee",
                id,
                period: None,
                amount: fee,
            }],
        });
    }
    let milestones: Vec<(Uuid, String, String, Decimal, String)> = sqlx::query_as(
        "SELECT m.id, p.name, m.name, m.amount, p.fee_currency FROM prj_milestones m JOIN prj_projects p ON p.id = m.project_id \
          WHERE p.org_id = $1 AND p.billing_method = 'milestone' AND m.amount > 0 AND m.reached_at IS NOT NULL \
            AND m.reached_at::date <= $2 \
            AND NOT EXISTS (SELECT 1 FROM billing_billed b WHERE b.source_type = 'project_milestone' AND b.source_id = m.id) \
          ORDER BY p.name, m.position",
    )
    .bind(client)
    .bind(to)
    .fetch_all(&mut *conn)
    .await?;
    for (id, project, milestone, amount, cur) in milestones {
        out.push(Candidate {
            kind: LineKind::Project,
            description: format!("{project}: milestone \"{milestone}\""),
            quantity: Decimal::ONE,
            unit: "each".into(),
            unit_price: amount,
            product_id: None,
            currency: if cur.is_empty() {
                currency.to_owned()
            } else {
                cur
            },
            sources: vec![Source {
                kind: "project_milestone",
                id,
                period: None,
                amount,
            }],
        });
    }
    Ok(out)
}

// ---- what's unbilled ----

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = UnbilledClient)]
pub struct Unbilled {
    pub org_id: Uuid,
    pub org_name: String,
    pub currency: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub time: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub products: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub recurring: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub projects: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub expenses: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub total: Decimal,
    pub lines: Vec<Candidate>,
}

/// What's billable, per client below `provider`, for a period. Run in
/// system scope.
pub async fn unbilled(
    conn: &mut PgConnection,
    provider: Uuid,
    from: Date,
    to: Date,
    clients: Option<&[Uuid]>,
) -> BillingResult<Vec<Unbilled>> {
    let orgs: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT c.id, c.name FROM organizations c JOIN organizations p ON p.id = $1 \
          WHERE c.kind = 'client' AND c.status = 'active' AND p.path @> c.path \
            AND ($2::uuid[] IS NULL OR c.id = ANY($2)) ORDER BY c.name",
    )
    .bind(provider)
    .bind(clients)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for (org, name) in orgs {
        let all = candidates(&mut *conn, org, from, to).await?;
        let mut by_currency: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
        for c in all {
            by_currency.entry(c.currency.clone()).or_default().push(c);
        }
        for (currency, lines) in by_currency {
            let sum = |k: LineKind| {
                lines
                    .iter()
                    .filter(|c| c.kind == k)
                    .map(Candidate::amount)
                    .sum::<Decimal>()
            };
            out.push(Unbilled {
                org_id: org,
                org_name: name.clone(),
                currency,
                time: sum(LineKind::Time),
                products: sum(LineKind::Product),
                recurring: sum(LineKind::Recurring),
                projects: sum(LineKind::Project),
                expenses: sum(LineKind::Expense),
                total: lines.iter().map(Candidate::amount).sum(),
                lines,
            });
        }
    }
    Ok(out)
}

// ---- billing runs ----

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = BillingRun)]
pub struct Run {
    pub id: Uuid,
    pub org_id: Uuid,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub period_start: Date,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub period_end: Date,
    pub invoices: i32,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BillingRunInput)]
pub struct RunInput {
    /// The provider running it.
    pub org_id: Uuid,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub from: Date,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub to: Date,
    /// Only these clients (default: every client below the provider).
    #[serde(default)]
    pub clients: Option<Vec<Uuid>>,
}

/// Generates draft invoices for what's billable in the period: one per
/// client and currency. Run in system scope (the caller checked access).
pub async fn run(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &RunInput,
) -> BillingResult<Run> {
    if input.to < input.from {
        return Err(BillingError::invalid("to", "ends before it starts"));
    }
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO billing_runs (id, org_id, period_start, period_end, created_by) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(input.org_id)
        .bind(input.from)
        .bind(input.to)
        .bind(ctx.actor.id())
        .execute(&mut *conn)
        .await?;
    let all = unbilled(
        &mut *conn,
        input.org_id,
        input.from,
        input.to,
        input.clients.as_deref(),
    )
    .await?;
    let mut made = 0;
    for u in all.into_iter().filter(|u| !u.lines.is_empty()) {
        draft(&mut *conn, ctx, &u, input.from, input.to, Some(id)).await?;
        made += 1;
    }
    sqlx::query("UPDATE billing_runs SET invoices = $2 WHERE id = $1")
        .bind(id)
        .bind(made)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("billing.run").org(input.org_id).details(json!({
            "run": id, "from": input.from.to_string(), "to": input.to.to_string(), "invoices": made,
        })),
    )
    .await?;
    get_run(conn, id).await
}

/// Writes a client's billable lines as a draft invoice, marking each
/// source billed.
async fn draft(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    u: &Unbilled,
    from: Date,
    to: Date,
    run: Option<Uuid>,
) -> BillingResult<Uuid> {
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO billing_invoices (id, org_id, currency, period_start, period_end, run_id, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(u.org_id)
    .bind(&u.currency)
    .bind(from)
    .bind(to)
    .bind(run)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    for (i, c) in u.lines.iter().enumerate() {
        let line = huxley_core::new_id();
        sqlx::query(
            "INSERT INTO billing_invoice_lines (id, invoice_id, org_id, position, kind, description, quantity, unit, \
                unit_price, product_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(line)
        .bind(id)
        .bind(u.org_id)
        .bind(i32::try_from(i).unwrap_or(0))
        .bind(c.kind)
        .bind(c.description.chars().take(1000).collect::<String>())
        .bind(c.quantity)
        .bind(&c.unit)
        .bind(c.unit_price)
        .bind(c.product_id)
        .execute(&mut *conn)
        .await?;
        for s in &c.sources {
            sqlx::query(
                "INSERT INTO billing_billed (source_type, source_id, period, line_id, org_id, amount) \
                 VALUES ($1, $2, coalesce($3, '-infinity'::date), $4, $5, $6)",
            )
            .bind(s.kind)
            .bind(s.id)
            .bind(s.period)
            .bind(line)
            .bind(u.org_id)
            .bind(s.amount)
            .execute(&mut *conn)
            .await?;
        }
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.created")
            .org(u.org_id)
            .target("invoice", id)
            .details(json!({ "lines": u.lines.len(), "total": u.total.normalize().to_string(), "run": run })),
    )
    .await?;
    crate::review::refresh(&mut *conn, id).await?;
    Ok(id)
}

pub async fn get_run(conn: &mut PgConnection, id: Uuid) -> BillingResult<Run> {
    sqlx::query_as(
        "SELECT r.id, r.org_id, r.period_start, r.period_end, r.invoices, u.display_name AS created_by_name, r.created_at \
           FROM billing_runs r LEFT JOIN users u ON u.id = r.created_by WHERE r.id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(BillingError::NotFound)
}

pub async fn runs(conn: &mut PgConnection) -> BillingResult<Vec<Run>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.org_id, r.period_start, r.period_end, r.invoices, u.display_name AS created_by_name, r.created_at \
           FROM billing_runs r LEFT JOIN users u ON u.id = r.created_by ORDER BY r.created_at DESC LIMIT 100",
    )
    .fetch_all(conn)
    .await?)
}
