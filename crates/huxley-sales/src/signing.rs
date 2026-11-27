//! Clients answering quotes in the portal (ADR-065).
//!
//! - **Who:** the quote's contact, and the company's approvers and admins.
//! - **What they see:** sent, accepted and declined quotes as sent: lines,
//!   options and totals, never costs or margin.
//! - **Accepting** takes a typed name, a drawn signature (a PNG) and the
//!   options taken; **declining** a name and a reason. Either keeps a
//!   record: who, the SHA-256 of the revision they answered, their address
//!   and browser, and when. Then the quote is decided as staff would
//!   ([`crate::quotes::decide`]), so its opportunity is won on acceptance.
//! - A quote past its "valid until" date can't be accepted.

use base64::Engine as _;
use huxley_core::RequestContext;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::quotes::{self, Billing, DecisionInput, LineKind, Quote, Status};
use crate::{SalesError, SalesResult, plain, required, text};

/// A line as the client sees it: no cost.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalQuoteLine {
    pub id: Uuid,
    pub kind: LineKind,
    pub sku: Option<String>,
    pub name: String,
    pub description: String,
    pub billing: Billing,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub unit: String,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub unit_price: Decimal,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub discount_pct: Decimal,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub total: Decimal,
    pub optional: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalQuoteTotals {
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub one_time: Decimal,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub monthly: Decimal,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub first_year: Decimal,
}

/// A quote as the client sees it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalQuote {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub status: Status,
    pub revision: i32,
    pub currency: String,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub valid_until: Option<Date>,
    /// Past its "valid until" date: it can't be accepted.
    pub expired: bool,
    pub contact_name: Option<String>,
    pub owner_name: Option<String>,
    pub intro: String,
    pub terms: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub sent_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub decided_by: Option<String>,
    pub lines: Vec<PortalQuoteLine>,
    pub totals: PortalQuoteTotals,
}

impl PortalQuote {
    fn from(q: Quote, today: Date) -> Self {
        Self {
            id: q.id,
            number: q.number,
            title: q.title,
            status: q.status,
            revision: q.revision,
            currency: q.currency,
            valid_until: q.valid_until,
            expired: q.status == Status::Sent && q.valid_until.is_some_and(|d| d < today),
            contact_name: q.contact_name,
            owner_name: q.owner_name,
            intro: q.intro,
            terms: q.terms,
            sent_at: q.sent_at,
            decided_at: q.decided_at,
            decided_by: q.decided_by,
            lines: q
                .lines
                .into_iter()
                .map(|l| PortalQuoteLine {
                    id: l.id,
                    kind: l.kind,
                    sku: l.sku,
                    name: l.name,
                    description: l.description,
                    billing: l.billing,
                    quantity: l.quantity,
                    unit: l.unit,
                    unit_price: l.unit_price,
                    discount_pct: l.discount_pct,
                    total: l.total,
                    optional: l.optional,
                    selected: l.selected,
                })
                .collect(),
            totals: PortalQuoteTotals {
                one_time: q.totals.one_time,
                monthly: q.totals.monthly,
                first_year: q.totals.first_year,
            },
        }
    }
}

/// Who's looking from the portal.
#[derive(Debug, Clone, Copy)]
pub struct Viewer {
    pub org_id: Uuid,
    pub contact_id: Uuid,
    pub account_id: Uuid,
    /// Approvers and admins see all the company's quotes.
    pub all: bool,
}

fn today() -> Date {
    OffsetDateTime::now_utc().date()
}

/// The quotes this viewer may see, newest first.
pub async fn portal_list(conn: &mut PgConnection, who: Viewer) -> SalesResult<Vec<PortalQuote>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sales_quotes WHERE org_id = $1 AND status <> 'draft' \
            AND ($2 OR contact_id = $3) ORDER BY coalesce(sent_at, updated_at) DESC LIMIT 200",
    )
    .bind(who.org_id)
    .bind(who.all)
    .bind(who.contact_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(PortalQuote::from(
            quotes::get(&mut *conn, id).await?,
            today(),
        ));
    }
    Ok(out)
}

async fn visible(conn: &mut PgConnection, who: Viewer, id: Uuid) -> SalesResult<Quote> {
    let q = quotes::get(&mut *conn, id).await?;
    if q.org_id != who.org_id
        || q.status == Status::Draft
        || !(who.all || q.contact_id == Some(who.contact_id))
    {
        return Err(SalesError::NotFound);
    }
    Ok(q)
}

pub async fn portal_get(
    conn: &mut PgConnection,
    who: Viewer,
    id: Uuid,
) -> SalesResult<PortalQuote> {
    Ok(PortalQuote::from(visible(conn, who, id).await?, today()))
}

/// The client's answer from the portal.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortalAnswer {
    pub accept: bool,
    /// Their name, typed: the signature in words.
    pub name: String,
    /// The signature drawn, as a `data:image/png;base64,` URL (accepting).
    #[serde(default)]
    pub signature: Option<String>,
    /// The options taken (accepting).
    #[serde(default)]
    pub options: Vec<Uuid>,
    /// Why not (declining).
    #[serde(default)]
    pub reason: Option<String>,
    /// That they agree to the quote and its terms (accepting).
    #[serde(default)]
    pub agree: bool,
    /// The revision they're answering.
    pub revision: i32,
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
const MAX_IMAGE: usize = 200 * 1024;

fn image(signature: Option<&str>) -> SalesResult<Vec<u8>> {
    let data = signature
        .and_then(|s| s.strip_prefix("data:image/png;base64,"))
        .ok_or_else(|| SalesError::invalid("signature", "sign in the box"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| SalesError::invalid("signature", "couldn't read the signature"))?;
    if !bytes.starts_with(PNG) {
        return Err(SalesError::invalid("signature", "a PNG image"));
    }
    if bytes.len() > MAX_IMAGE {
        return Err(SalesError::invalid("signature", "too large"));
    }
    Ok(bytes)
}

/// Records the client's answer, with how they gave it, and decides the quote.
pub async fn answer(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    who: Viewer,
    id: Uuid,
    input: &PortalAnswer,
) -> SalesResult<PortalQuote> {
    let q = visible(&mut *conn, who, id).await?;
    if q.status != Status::Sent {
        return Err(SalesError::Conflict("it's already been answered".into()));
    }
    if q.revision != input.revision {
        return Err(SalesError::Conflict(
            "it has changed since you opened it; look again".into(),
        ));
    }
    let name = required("name", &input.name, 200)?;
    let png = if input.accept {
        if q.valid_until.is_some_and(|d| d < today()) {
            return Err(SalesError::Conflict(
                "it's past its valid-until date; ask for a new one".into(),
            ));
        }
        if !input.agree {
            return Err(SalesError::invalid(
                "agree",
                "agree to the quote and its terms",
            ));
        }
        Some(image(input.signature.as_deref())?)
    } else {
        None
    };
    let snapshot: serde_json::Value = sqlx::query_scalar(
        "SELECT snapshot FROM sales_quote_revisions WHERE quote_id = $1 AND revision = $2",
    )
    .bind(id)
    .bind(q.revision)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| SalesError::Conflict("its sent revision is missing".into()))?;
    let digest = Sha256::digest(snapshot.to_string().as_bytes());
    let sha = digest.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    });
    let user_agent = ctx
        .user_agent
        .as_deref()
        .map(|u| u.chars().take(512).collect::<String>());
    sqlx::query(
        "INSERT INTO sales_quote_signatures (id, quote_id, org_id, revision, accepted, name, image, \
            contact_id, portal_account_id, document_sha256, ip, user_agent) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::inet, $12)",
    )
    .bind(huxley_core::new_id())
    .bind(id)
    .bind(q.org_id)
    .bind(q.revision)
    .bind(input.accept)
    .bind(&name)
    .bind(&png)
    .bind(who.contact_id)
    .bind(who.account_id)
    .bind(&sha)
    .bind(ctx.ip.map(|ip| ip.to_string()))
    .bind(&user_agent)
    .execute(&mut *conn)
    .await?;
    quotes::decide(
        &mut *conn,
        ctx,
        id,
        &DecisionInput {
            accept: input.accept,
            name,
            reason: text("reason", input.reason.as_deref(), 2000)?,
            options: input.options.clone(),
            version: q.version,
        },
    )
    .await?;
    portal_get(conn, who, id).await
}

/// How a quote was answered in the portal, for staff.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub revision: i32,
    pub accepted: bool,
    pub name: String,
    /// A signature was drawn (fetch it from `…/signature.png`).
    pub has_image: bool,
    pub contact_name: Option<String>,
    pub document_sha256: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub signed_at: OffsetDateTime,
}

pub async fn signature(conn: &mut PgConnection, quote: Uuid) -> SalesResult<Option<Signature>> {
    Ok(sqlx::query_as(
        "SELECT s.revision, s.accepted, s.name, s.image IS NOT NULL AS has_image, \
                nullif(btrim(coalesce(c.first_name, '') || ' ' || coalesce(c.last_name, '')), '') AS contact_name, \
                s.document_sha256, host(s.ip) AS ip, s.user_agent, s.signed_at \
           FROM sales_quote_signatures s LEFT JOIN crm_contacts c ON c.id = s.contact_id \
          WHERE s.quote_id = $1",
    )
    .bind(quote)
    .fetch_optional(conn)
    .await?)
}

pub async fn signature_image(conn: &mut PgConnection, quote: Uuid) -> SalesResult<Option<Vec<u8>>> {
    Ok(sqlx::query_scalar::<_, Option<Vec<u8>>>(
        "SELECT image FROM sales_quote_signatures WHERE quote_id = $1",
    )
    .bind(quote)
    .fetch_optional(conn)
    .await?
    .flatten())
}
