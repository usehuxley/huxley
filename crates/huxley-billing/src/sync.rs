//! Accounting sync (ADR-077): approved invoices and credit memos go to
//! the accounting system through a pack's system workflow.
//!
//! - **Settings** per provider name the target (a pack's key, e.g.
//!   `quickbooks` or `xero`) and whether invoices sync when approved.
//! - **Mappings** match clients to customers, products (or line kinds) to
//!   items, tax codes and classes. The accounting system's lists come from
//!   the pack's datasets (`<target>-customers`, `-items`, `-tax-codes`,
//!   `-classes`, each with `id` and `name`).
//! - **Requesting** a sync prepares the invoice with its mappings and raises
//!   `invoice.sync_requested` for the pack's workflow; an unmapped client or
//!   item fails it straight away, with the reason. The workflow reports back
//!   ([`record`], the `core.invoice.sync_result` step): synced (the invoice
//!   is then sent) or failed. Failed syncs are the error queue; retrying
//!   asks again.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::invoices::{self, Invoice, Status};
use crate::{BillingError, BillingResult, text};

// ---- settings ----

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AccountingSyncSettings)]
pub struct Settings {
    /// The accounting system: the sync pack's target, e.g. `quickbooks` or `xero`.
    pub target: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Sync each invoice as soon as it's approved (otherwise by hand).
    #[serde(default = "yes")]
    pub on_approval: bool,
    /// A client goes on credit hold when an invoice is this many days overdue…
    #[serde(default)]
    pub credit_hold_days: Option<i32>,
    /// …or this much is overdue in all (ADR-078).
    #[serde(
        default,
        serialize_with = "crate::plain_opt",
        deserialize_with = "crate::number_opt"
    )]
    #[schema(value_type = Option<String>)]
    pub credit_hold_amount: Option<Decimal>,
}

const fn yes() -> bool {
    true
}

pub async fn settings(conn: &mut PgConnection, provider: Uuid) -> BillingResult<Option<Settings>> {
    Ok(sqlx::query_as(
        "SELECT target, enabled, on_approval, credit_hold_days, credit_hold_amount FROM billing_sync_settings \
          WHERE org_id = $1",
    )
    .bind(provider)
    .fetch_optional(conn)
    .await?)
}

/// The nearest provider above a client with sync settings: `(provider, settings)`.
pub async fn for_client(
    conn: &mut PgConnection,
    client: Uuid,
) -> BillingResult<Option<(Uuid, Settings)>> {
    let provider: Option<Uuid> = sqlx::query_scalar(
        "SELECT s.org_id FROM billing_sync_settings s \
           JOIN organizations so ON so.id = s.org_id JOIN organizations c ON c.id = $1 \
          WHERE so.path @> c.path ORDER BY nlevel(so.path) DESC LIMIT 1",
    )
    .bind(client)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(provider) = provider else {
        return Ok(None);
    };
    Ok(settings(conn, provider).await?.map(|s| (provider, s)))
}

fn check_target(target: &str) -> BillingResult<String> {
    let t = target.trim().to_ascii_lowercase();
    let ok = (2..=40).contains(&t.len())
        && t.starts_with(|c: char| c.is_ascii_lowercase())
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(t)
    } else {
        Err(BillingError::invalid(
            "target",
            "a sync pack's target: lowercase letters, digits and dashes, e.g. quickbooks",
        ))
    }
}

pub async fn set_settings(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: Uuid,
    s: &Settings,
) -> BillingResult<Settings> {
    let target = check_target(&s.target)?;
    if s.credit_hold_days.is_some_and(|d| !(1..=365).contains(&d)) {
        return Err(BillingError::invalid("creditHoldDays", "1 to 365"));
    }
    if s.credit_hold_amount.is_some_and(|a| a < Decimal::ZERO) {
        return Err(BillingError::invalid("creditHoldAmount", "zero or more"));
    }
    sqlx::query(
        "INSERT INTO billing_sync_settings (org_id, target, enabled, on_approval, credit_hold_days, credit_hold_amount) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (org_id) DO UPDATE SET target = $2, enabled = $3, on_approval = $4, credit_hold_days = $5, \
            credit_hold_amount = $6, updated_at = now()",
    )
    .bind(provider)
    .bind(&target)
    .bind(s.enabled)
    .bind(s.on_approval)
    .bind(s.credit_hold_days)
    .bind(s.credit_hold_amount)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("billing.sync_settings_changed")
            .org(provider)
            .target("organization", provider)
            .details(
                json!({ "target": target, "enabled": s.enabled, "onApproval": s.on_approval }),
            ),
    )
    .await?;
    Ok(settings(conn, provider).await?.unwrap_or_else(|| s.clone()))
}

// ---- mappings ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum MappingKind {
    /// A client (its id) ↔ a customer.
    Customer,
    /// `product:<id>`, `kind:<line kind>` or `default` ↔ an item.
    Item,
    /// A tax code (or `default`) ↔ a tax code or rate.
    TaxCode,
    /// A client (its id) or `default` ↔ a class or tracking category.
    Class,
}

impl MappingKind {
    /// The pack's dataset with the accounting system's list.
    fn dataset(self, target: &str) -> String {
        let list = match self {
            Self::Customer => "customers",
            Self::Item => "items",
            Self::TaxCode => "tax-codes",
            Self::Class => "classes",
        };
        format!("{target}-{list}")
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = AccountingMapping)]
pub struct Mapping {
    pub id: Uuid,
    pub kind: MappingKind,
    pub local_key: String,
    /// What the local key is: the client's or product's name, or the line kind.
    pub local_name: Option<String>,
    pub remote_id: String,
    pub remote_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AccountingMappingInput)]
pub struct MappingInput {
    pub kind: MappingKind,
    pub local_key: String,
    pub remote_id: String,
    #[serde(default)]
    pub remote_name: Option<String>,
}

const MAPPING: &str = "SELECT m.id, m.kind, m.local_key, \
        coalesce(o.name, p.name, CASE WHEN m.local_key LIKE 'kind:%' THEN initcap(substr(m.local_key, 6)) || ' lines' \
                                      WHEN m.local_key = 'default' THEN 'Anything else' ELSE m.local_key END) AS local_name, \
        m.remote_id, m.remote_name \
   FROM billing_mappings m \
   LEFT JOIN organizations o ON m.kind IN ('customer', 'class') AND o.id::text = m.local_key \
   LEFT JOIN catalog_products p ON m.kind = 'item' AND m.local_key LIKE 'product:%' AND p.id::text = substr(m.local_key, 9)";

pub async fn mappings(
    conn: &mut PgConnection,
    provider: Uuid,
    target: &str,
) -> BillingResult<Vec<Mapping>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{MAPPING} WHERE m.org_id = $1 AND m.target = $2 ORDER BY m.kind, local_name"
    )))
    .bind(provider)
    .bind(target)
    .fetch_all(conn)
    .await?)
}

const LINE_KINDS: [&str; 5] = ["time", "product", "recurring", "project", "manual"];

fn check_local(kind: MappingKind, key: &str) -> BillingResult<String> {
    let key = key.trim();
    let uuid = |s: &str| Uuid::parse_str(s).is_ok();
    let ok = match kind {
        MappingKind::Customer => uuid(key),
        MappingKind::Class => uuid(key) || key == "default",
        MappingKind::Item => {
            key == "default"
                || key.strip_prefix("product:").is_some_and(uuid)
                || key
                    .strip_prefix("kind:")
                    .is_some_and(|k| LINE_KINDS.contains(&k))
        }
        MappingKind::TaxCode => !key.is_empty() && key.chars().count() <= 50,
    };
    if ok {
        Ok(key.to_owned())
    } else {
        Err(BillingError::invalid(
            "localKey",
            match kind {
                MappingKind::Customer => "a client's id",
                MappingKind::Class => "a client's id, or default",
                MappingKind::Item => {
                    "product:<id>, kind:<time|product|recurring|project|manual> or default"
                }
                MappingKind::TaxCode => "a tax code (or default), up to 50 characters",
            },
        ))
    }
}

/// Sets (or replaces) a mapping.
pub async fn set_mapping(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: Uuid,
    target: &str,
    input: &MappingInput,
) -> BillingResult<Mapping> {
    let target = check_target(target)?;
    let local = check_local(input.kind, &input.local_key)?;
    let remote = crate::required("remoteId", &input.remote_id, 200)?;
    let name = text("remoteName", input.remote_name.as_deref(), 300)?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO billing_mappings (id, org_id, target, kind, local_key, remote_id, remote_name) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (org_id, target, kind, local_key) DO UPDATE SET remote_id = $6, remote_name = $7, updated_at = now() \
         RETURNING id",
    )
    .bind(huxley_core::new_id())
    .bind(provider)
    .bind(&target)
    .bind(input.kind)
    .bind(&local)
    .bind(&remote)
    .bind(&name)
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("billing.mapping_set")
            .org(provider)
            .target("billing_mapping", id)
            .details(
                json!({ "target": target, "kind": input.kind, "local": local, "remote": remote }),
            ),
    )
    .await?;
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{MAPPING} WHERE m.id = $1")))
        .bind(id)
        .fetch_one(conn)
        .await
        .map_err(Into::into)
}

pub async fn delete_mapping(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: Uuid,
    id: Uuid,
) -> BillingResult<()> {
    let done = sqlx::query("DELETE FROM billing_mappings WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(provider)
        .execute(&mut *conn)
        .await?;
    if done.rows_affected() == 0 {
        return Err(BillingError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("billing.mapping_removed")
            .org(provider)
            .target("billing_mapping", id),
    )
    .await?;
    Ok(())
}

/// A customer, item, tax code or class in the accounting system.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteOption {
    pub id: String,
    pub name: String,
}

/// The accounting system's list of `kind`, from the pack's dataset (none if
/// the pack isn't installed or hasn't fetched it yet).
pub async fn remote_options(
    conn: &mut PgConnection,
    provider: Uuid,
    target: &str,
    kind: MappingKind,
) -> BillingResult<Vec<RemoteOption>> {
    let Ok(dataset) = huxley_datasets::find(&mut *conn, provider, &kind.dataset(target)).await
    else {
        return Ok(Vec::new());
    };
    let query = huxley_datasets::Query {
        order_by: Some("name".into()),
        limit: Some(1000),
        ..huxley_datasets::Query::default()
    };
    let Ok(rows) = huxley_datasets::query(&mut *conn, &dataset, &query).await else {
        return Ok(Vec::new());
    };
    Ok(rows
        .rows
        .iter()
        .filter_map(|r| {
            let id = match &r["id"] {
                Json::String(s) => s.clone(),
                Json::Number(n) => n.to_string(),
                _ => return None,
            };
            let name = r["name"].as_str().map_or_else(|| id.clone(), str::to_owned);
            Some(RemoteOption { id, name })
        })
        .collect())
}

// ---- preparing ----

type Found = Option<(String, Option<String>)>;

async fn mapped(
    conn: &mut PgConnection,
    provider: Uuid,
    target: &str,
    kind: MappingKind,
    keys: &[String],
) -> BillingResult<Found> {
    Ok(sqlx::query_as(
        "SELECT remote_id, remote_name FROM billing_mappings \
          WHERE org_id = $1 AND target = $2 AND kind = $3 AND local_key = ANY($4) \
          ORDER BY array_position($4, local_key) LIMIT 1",
    )
    .bind(provider)
    .bind(target)
    .bind(kind)
    .bind(keys)
    .fetch_optional(conn)
    .await?)
}

/// The client and the clients above it (nearest first), as mapping keys.
async fn client_keys(conn: &mut PgConnection, client: Uuid) -> BillingResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT o.id::text FROM organizations o JOIN organizations c ON c.id = $1 \
          WHERE o.path @> c.path ORDER BY nlevel(o.path) DESC",
    )
    .bind(client)
    .fetch_all(conn)
    .await?)
}

fn reference((id, name): (String, Option<String>)) -> Json {
    json!({ "id": id, "name": name })
}

fn money(d: Decimal) -> String {
    d.round_dp(4).normalize().to_string()
}

/// The invoice as the sync workflow gets it, with the accounting system's
/// customer, items, tax codes and class; or why it can't be.
pub async fn prepare(
    conn: &mut PgConnection,
    i: &Invoice,
    provider: Uuid,
    target: &str,
) -> BillingResult<Result<Json, String>> {
    let clients = client_keys(&mut *conn, i.org_id).await?;
    let Some(customer) = mapped(
        &mut *conn,
        provider,
        target,
        MappingKind::Customer,
        &clients,
    )
    .await?
    else {
        return Ok(Err(format!(
            "{} isn't matched to a customer in {target}: map it under Billing → Accounting",
            i.org_name
        )));
    };
    let mut classes = clients.clone();
    classes.push("default".into());
    let class = mapped(&mut *conn, provider, target, MappingKind::Class, &classes).await?;
    let credit = i.kind == "credit_memo";
    let mut lines = Vec::new();
    let mut missing = Vec::new();
    for l in &i.lines {
        let mut keys = Vec::new();
        if let Some(p) = l.product_id {
            keys.push(format!("product:{p}"));
        }
        keys.push(format!("kind:{}", l.kind.as_str()));
        keys.push("default".into());
        let Some(item) = mapped(&mut *conn, provider, target, MappingKind::Item, &keys).await?
        else {
            missing.push(l.description.clone());
            continue;
        };
        let tax_keys = vec![l.tax_code.clone().unwrap_or_else(|| "default".into())];
        let tax = mapped(
            &mut *conn,
            provider,
            target,
            MappingKind::TaxCode,
            &tax_keys,
        )
        .await?;
        // Credit memos carry positive amounts: the document says it's a credit.
        let quantity = if credit { -l.quantity } else { l.quantity };
        let amount = if credit { -l.amount } else { l.amount };
        lines.push(json!({
            "description": l.description,
            "kind": l.kind.as_str(),
            "quantity": money(quantity),
            "unit": l.unit,
            "unitPrice": money(l.unit_price),
            "amount": money(amount),
            "item": reference(item),
            "taxCode": tax.map(reference),
        }));
    }
    if !missing.is_empty() {
        return Ok(Err(format!(
            "no item in {target} for {}: map their products or line kinds (or a default item) under Billing → Accounting",
            missing.join(", ")
        )));
    }
    let credit_of: Option<(Uuid, Option<String>)> = match i.credit_of {
        Some(of) => {
            sqlx::query_as(
                "SELECT invoice_id, remote_id FROM billing_invoice_syncs WHERE invoice_id = $1",
            )
            .bind(of)
            .fetch_optional(&mut *conn)
            .await?
        }
        None => None,
    };
    let total = if credit { -i.total } else { i.total };
    let day = |d: Option<time::Date>| d.map(|d| d.to_string());
    Ok(Ok(json!({
        "invoiceId": i.id,
        "target": target,
        "kind": i.kind,
        "reference": format!("INV-{}", i.number),
        "number": i.number.to_string(),
        "client": i.org_name,
        "orgId": i.org_id,
        "currency": i.currency,
        "issuedOn": day(i.issued_on),
        "dueOn": day(i.due_on),
        "periodStart": day(i.period_start),
        "periodEnd": day(i.period_end),
        "notes": i.notes,
        "customer": reference(customer),
        "class": class.map(reference),
        "lines": lines,
        "total": money(total),
        "creditOf": credit_of.map(|(id, remote)| json!({ "invoiceId": id, "remoteId": remote })),
    })))
}

// ---- syncing ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum SyncStatus {
    /// Waiting for the sync workflow.
    Pending,
    Synced,
    Failed,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceSync)]
pub struct Sync {
    pub invoice_id: Uuid,
    pub number: i64,
    pub kind: String,
    pub org_id: Uuid,
    pub org_name: String,
    pub target: String,
    pub status: SyncStatus,
    pub remote_id: Option<String>,
    pub remote_number: Option<String>,
    pub remote_url: Option<String>,
    pub attempts: i32,
    pub error: Option<String>,
    /// What's still owed, from the accounting system (ADR-078).
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub amount_due: Option<Decimal>,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub amount_paid: Option<Decimal>,
    pub remote_status: Option<String>,
    /// Where the client views and pays it.
    pub pay_url: Option<String>,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub paid_on: Option<time::Date>,
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub synced_at: Option<OffsetDateTime>,
}

const SYNC: &str = "SELECT s.invoice_id, i.number, i.kind, s.org_id, o.name AS org_name, s.target, s.status, s.remote_id, \
        s.remote_number, s.remote_url, s.attempts, s.error, s.amount_due, s.amount_paid, s.remote_status, s.pay_url, \
        s.paid_on, s.requested_at, s.synced_at \
   FROM billing_invoice_syncs s JOIN billing_invoices i ON i.id = s.invoice_id JOIN organizations o ON o.id = s.org_id";

pub async fn state(conn: &mut PgConnection, invoice: Uuid) -> BillingResult<Option<Sync>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SYNC} WHERE s.invoice_id = $1"
    )))
    .bind(invoice)
    .fetch_optional(conn)
    .await?)
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct QueueFilter {
    /// `failed` (the error queue, the default), `pending` or `synced`.
    pub status: Option<SyncStatus>,
}

/// Syncs in a status (failed, by default), oldest first.
pub async fn queue(conn: &mut PgConnection, filter: &QueueFilter) -> BillingResult<Vec<Sync>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SYNC} WHERE s.status = $1 ORDER BY s.requested_at LIMIT 500"
    )))
    .bind(filter.status.unwrap_or(SyncStatus::Failed))
    .fetch_all(conn)
    .await?)
}

/// Asks for an invoice to be synced: prepares it and raises
/// `invoice.sync_requested`, or fails it with the reason (an unmapped
/// client or item, or no accounting system set up).
pub async fn request(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> BillingResult<Sync> {
    let i = invoices::get(&mut *conn, id).await?;
    if !matches!(i.status, Status::Approved | Status::Sent) {
        return Err(BillingError::Conflict(
            "only an approved invoice can be synced".into(),
        ));
    }
    if state(&mut *conn, id)
        .await?
        .is_some_and(|s| s.status == SyncStatus::Synced)
    {
        return Err(BillingError::Conflict("it's already synced".into()));
    }
    let Some((provider, s)) = for_client(&mut *conn, i.org_id).await? else {
        return Err(BillingError::Conflict(
            "no accounting system is set up: choose one in Settings → Billing".into(),
        ));
    };
    if !s.enabled {
        return Err(BillingError::Conflict(
            "the accounting sync is turned off in Settings → Billing".into(),
        ));
    }
    let prepared = prepare(&mut *conn, &i, provider, &s.target).await?;
    let (status, error) = match &prepared {
        Ok(_) => (SyncStatus::Pending, None),
        Err(why) => (SyncStatus::Failed, Some(why.clone())),
    };
    let attempts: i32 = sqlx::query_scalar(
        "INSERT INTO billing_invoice_syncs (invoice_id, org_id, target, status, attempts, error) \
         VALUES ($1, $2, $3, $4, 1, $5) \
         ON CONFLICT (invoice_id) DO UPDATE SET target = $3, status = $4, attempts = billing_invoice_syncs.attempts + 1, \
            error = $5, requested_at = now() \
         RETURNING attempts",
    )
    .bind(id)
    .bind(i.org_id)
    .bind(&s.target)
    .bind(status)
    .bind(&error)
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.sync_requested")
            .org(i.org_id)
            .target("invoice", id)
            .details(json!({ "target": s.target, "attempt": attempts, "error": error })),
    )
    .await?;
    match prepared {
        Ok(mut payload) => {
            payload["attempt"] = json!(attempts);
            payload["link"] = json!(format!("/billing/invoices/{id}"));
            outbox::emit(
                &mut *conn,
                DomainEvent {
                    event_type: "invoice.sync_requested",
                    org_id: Some(i.org_id),
                    subject_type: "invoice",
                    subject_id: id,
                    payload,
                },
            )
            .await?;
        }
        Err(why) => failed_event(&mut *conn, &i, &s.target, &why).await?,
    }
    state(conn, id).await?.ok_or(BillingError::NotFound)
}

async fn failed_event(
    conn: &mut PgConnection,
    i: &Invoice,
    target: &str,
    why: &str,
) -> BillingResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "invoice.sync_failed",
            org_id: Some(i.org_id),
            subject_type: "invoice",
            subject_id: i.id,
            payload: json!({
                "invoiceId": i.id, "reference": format!("INV-{}", i.number), "client": i.org_name,
                "target": target, "error": why, "link": format!("/billing/invoices/{}", i.id),
            }),
        },
    )
    .await?;
    Ok(())
}

/// What the sync workflow reports.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Outcome {
    pub ok: bool,
    pub remote_id: Option<String>,
    pub remote_number: Option<String>,
    pub remote_url: Option<String>,
    pub error: Option<String>,
}

/// Records how a sync went (from the sync workflow). Synced, an approved
/// invoice is marked sent and raises `invoice.synced`; failed, it joins the
/// error queue and raises `invoice.sync_failed`.
pub async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    outcome: &Outcome,
) -> BillingResult<Sync> {
    let i = invoices::get(&mut *conn, id).await?;
    let current = state(&mut *conn, id)
        .await?
        .ok_or_else(|| BillingError::Conflict("no sync was asked for this invoice".into()))?;
    if current.status == SyncStatus::Synced {
        return Err(BillingError::Conflict("it's already synced".into()));
    }
    let clip = |s: Option<&str>, n: usize| {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(n).collect::<String>())
    };
    if outcome.ok {
        let remote = clip(outcome.remote_id.as_deref(), 200)
            .ok_or_else(|| BillingError::invalid("remote_id", "required when it synced"))?;
        sqlx::query(
            "UPDATE billing_invoice_syncs SET status = 'synced', remote_id = $2, remote_number = $3, remote_url = $4, \
                error = NULL, synced_at = now() WHERE invoice_id = $1",
        )
        .bind(id)
        .bind(&remote)
        .bind(clip(outcome.remote_number.as_deref(), 100))
        .bind(clip(outcome.remote_url.as_deref(), 2000).filter(|u| u.starts_with("https://")))
        .execute(&mut *conn)
        .await?;
        if i.status == Status::Approved {
            invoices::mark_sent(&mut *conn, ctx, id).await?;
        }
        outbox::emit(
            &mut *conn,
            DomainEvent {
                event_type: "invoice.synced",
                org_id: Some(i.org_id),
                subject_type: "invoice",
                subject_id: id,
                payload: json!({
                    "invoiceId": id, "reference": format!("INV-{}", i.number), "client": i.org_name,
                    "target": current.target, "remoteId": remote, "link": format!("/billing/invoices/{id}"),
                }),
            },
        )
        .await?;
    } else {
        let why = clip(outcome.error.as_deref(), 4000)
            .unwrap_or_else(|| "the sync failed without saying why".into());
        sqlx::query(
            "UPDATE billing_invoice_syncs SET status = 'failed', error = $2 WHERE invoice_id = $1",
        )
        .bind(id)
        .bind(&why)
        .execute(&mut *conn)
        .await?;
        failed_event(&mut *conn, &i, &current.target, &why).await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(if outcome.ok {
            "invoice.synced"
        } else {
            "invoice.sync_failed"
        })
        .org(i.org_id)
        .target("invoice", id)
        .details(json!({ "target": current.target, "remoteId": outcome.remote_id, "error": outcome.error })),
    )
    .await?;
    state(conn, id).await?.ok_or(BillingError::NotFound)
}

/// After approval: syncs it if its provider syncs on approval.
pub(crate) async fn on_approved(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    i: &Invoice,
) -> BillingResult<()> {
    if let Some((_, s)) = for_client(&mut *conn, i.org_id).await?
        && s.enabled
        && s.on_approval
    {
        request(conn, ctx, i.id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_and_local_keys_are_checked() {
        assert_eq!(check_target(" QuickBooks ").unwrap(), "quickbooks");
        assert!(check_target("x").is_err());
        assert!(check_target("quick books").is_err());
        let id = Uuid::nil().to_string();
        assert!(check_local(MappingKind::Customer, &id).is_ok());
        assert!(check_local(MappingKind::Customer, "default").is_err());
        assert!(check_local(MappingKind::Class, "default").is_ok());
        assert!(check_local(MappingKind::Item, &format!("product:{id}")).is_ok());
        assert!(check_local(MappingKind::Item, "kind:time").is_ok());
        assert!(check_local(MappingKind::Item, "kind:travel").is_err());
        assert!(check_local(MappingKind::TaxCode, "TAX").is_ok());
        assert_eq!(MappingKind::TaxCode.dataset("xero"), "xero-tax-codes");
    }
}
