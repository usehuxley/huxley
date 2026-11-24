//! Sites: a company's locations, with their timezone and opening hours
//! (later the basis of SLA business hours).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CrmError, CrmResult, text};

pub const DAYS: &[&str] = &["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub postal_code: Option<String>,
    /// ISO 3166-1 alpha-2, e.g. `GB`.
    pub country: Option<String>,
    /// IANA timezone, e.g. `Europe/London`.
    pub timezone: String,
    /// Opening hours per weekday: `{"mon": [["09:00", "17:00"]], …}`. A
    /// missing day is closed.
    #[schema(value_type = std::collections::HashMap<String, Vec<Vec<String>>>)]
    pub hours: Json,
    pub phone: Option<String>,
    pub is_primary: bool,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub custom_fields: Json,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SiteInput {
    pub name: String,
    #[serde(default)]
    pub address_line1: Option<String>,
    #[serde(default)]
    pub address_line2: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub postal_code: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    /// IANA timezone; defaults to UTC.
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, Vec<Vec<String>>>>)]
    pub hours: Option<Json>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub is_primary: bool,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT id, org_id, name, address_line1, address_line2, city, region, postal_code, \
    country, timezone, hours, phone, is_primary, custom_fields, version, created_at, updated_at, archived_at \
    FROM crm_sites";

/// `HH:MM`, 00:00 to 24:00.
fn minutes(value: &Json) -> Option<u32> {
    let text = value.as_str()?;
    let (h, m) = text.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (m < 60 && (h < 24 || (h == 24 && m == 0))).then_some(h * 60 + m)
}

/// Checks opening hours: known weekdays, each a list of `[open, close]`
/// ranges in order that don't overlap.
pub fn check_hours(hours: &Json) -> CrmResult<Json> {
    let bad = |message: String| CrmError::invalid("hours", message);
    let Json::Object(days) = hours else {
        return Err(bad("an object of weekdays".into()));
    };
    let mut out = Map::new();
    for (day, ranges) in days {
        if !DAYS.contains(&day.as_str()) {
            return Err(bad(format!(
                "{day:?} isn't a weekday ({})",
                DAYS.join(", ")
            )));
        }
        let Json::Array(ranges) = ranges else {
            return Err(bad(format!("{day}: a list of [open, close] times")));
        };
        let mut last = 0;
        for range in ranges {
            let (Some(open), Some(close)) = (
                range.get(0).and_then(minutes),
                range.get(1).and_then(minutes),
            ) else {
                return Err(bad(format!(
                    "{day}: times are HH:MM, as [\"09:00\", \"17:00\"]"
                )));
            };
            if open >= close || open < last || range.as_array().map_or(0, Vec::len) != 2 {
                return Err(bad(format!(
                    "{day}: each range opens before it closes, after the one before"
                )));
            }
            last = close;
        }
        if !ranges.is_empty() {
            out.insert(day.clone(), Json::Array(ranges.clone()));
        }
    }
    Ok(Json::Object(out))
}

struct Checked {
    name: String,
    line1: Option<String>,
    line2: Option<String>,
    city: Option<String>,
    region: Option<String>,
    postal_code: Option<String>,
    country: Option<String>,
    timezone: String,
    hours: Json,
    phone: Option<String>,
}

fn check(input: &SiteInput) -> CrmResult<Checked> {
    let name = text("name", Some(&input.name), 200)?
        .ok_or_else(|| CrmError::invalid("name", "a site needs a name"))?;
    let country = text("country", input.country.as_deref(), 2)?.map(|c| c.to_uppercase());
    if let Some(code) = &country
        && !(code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase()))
    {
        return Err(CrmError::invalid(
            "country",
            "a two-letter ISO country code, e.g. GB",
        ));
    }
    let timezone = text("timezone", input.timezone.as_deref(), 64)?.unwrap_or_else(|| "UTC".into());
    if jiff::tz::TimeZone::get(&timezone).is_err() {
        return Err(CrmError::invalid(
            "timezone",
            format!("{timezone:?} isn't a timezone; use a name like Europe/London"),
        ));
    }
    Ok(Checked {
        name,
        line1: text("addressLine1", input.address_line1.as_deref(), 200)?,
        line2: text("addressLine2", input.address_line2.as_deref(), 200)?,
        city: text("city", input.city.as_deref(), 100)?,
        region: text("region", input.region.as_deref(), 100)?,
        postal_code: text("postalCode", input.postal_code.as_deref(), 20)?,
        country,
        timezone,
        hours: check_hours(input.hours.as_ref().unwrap_or(&json!({})))?,
        phone: text("phone", input.phone.as_deref(), 50)?,
    })
}

/// The organization's sites, primary first, then by name.
pub async fn list(
    conn: &mut PgConnection,
    org_id: Uuid,
    include_archived: bool,
) -> CrmResult<Vec<Site>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE org_id = $1 AND ($2 OR archived_at IS NULL) \
          ORDER BY is_primary DESC, lower(name)"
    )))
    .bind(org_id)
    .bind(include_archived)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> CrmResult<Site> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CrmError::NotFound)
}

/// Makes `id` the organization's only primary site.
async fn take_primary(conn: &mut PgConnection, org_id: Uuid, id: Uuid) -> CrmResult<()> {
    sqlx::query(
        "UPDATE crm_sites SET is_primary = false WHERE org_id = $1 AND id <> $2 AND is_primary",
    )
    .bind(org_id)
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    site: &Site,
    action: &str,
) -> CrmResult<()> {
    audit::record(
        conn,
        ctx,
        AuditRecord::new(action)
            .org(site.org_id)
            .target("site", site.id)
            .details(json!({ "name": site.name })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: action,
            org_id: Some(site.org_id),
            subject_type: "site",
            subject_id: site.id,
            payload: json!({ "name": site.name, "timezone": site.timezone }),
        },
    )
    .await?;
    Ok(())
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &SiteInput,
) -> CrmResult<Site> {
    let c = check(input)?;
    let id = huxley_core::new_id();
    // The first site of an organization is its primary one.
    let first: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT 1 FROM crm_sites WHERE org_id = $1 AND archived_at IS NULL)",
    )
    .bind(org_id)
    .fetch_one(&mut *conn)
    .await?;
    let primary = input.is_primary || first;
    if primary {
        take_primary(conn, org_id, id).await?;
    }
    sqlx::query(
        "INSERT INTO crm_sites (id, org_id, name, address_line1, address_line2, city, region, \
                                postal_code, country, timezone, hours, phone, is_primary) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.name)
    .bind(&c.line1)
    .bind(&c.line2)
    .bind(&c.city)
    .bind(&c.region)
    .bind(&c.postal_code)
    .bind(&c.country)
    .bind(&c.timezone)
    .bind(&c.hours)
    .bind(&c.phone)
    .bind(primary)
    .execute(&mut *conn)
    .await?;
    let site = get(conn, id).await?;
    record(conn, ctx, &site, "site.created").await?;
    Ok(site)
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &SiteInput,
) -> CrmResult<Site> {
    let c = check(input)?;
    let before = get(conn, id).await?;
    let version = input.version.unwrap_or(-1);
    if before.version != version {
        return Err(CrmError::VersionConflict);
    }
    if input.is_primary {
        take_primary(conn, before.org_id, id).await?;
    }
    let changed = sqlx::query(
        "UPDATE crm_sites SET name = $3, address_line1 = $4, address_line2 = $5, city = $6, region = $7, \
                postal_code = $8, country = $9, timezone = $10, hours = $11, phone = $12, \
                is_primary = $13, version = version + 1, updated_at = now() \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .bind(&c.name)
    .bind(&c.line1)
    .bind(&c.line2)
    .bind(&c.city)
    .bind(&c.region)
    .bind(&c.postal_code)
    .bind(&c.country)
    .bind(&c.timezone)
    .bind(&c.hours)
    .bind(&c.phone)
    .bind(input.is_primary || before.is_primary)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(CrmError::VersionConflict);
    }
    let site = get(conn, id).await?;
    record(conn, ctx, &site, "site.updated").await?;
    Ok(site)
}

/// Archives a site (it stays on contacts that were at it).
pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> CrmResult<Site> {
    let changed = sqlx::query(
        "UPDATE crm_sites SET archived_at = now(), is_primary = false, version = version + 1 \
          WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(CrmError::NotFound);
    }
    let site = get(conn, id).await?;
    record(conn, ctx, &site, "site.archived").await?;
    Ok(site)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_hours_are_checked() {
        let ok = json!({ "mon": [["09:00", "12:30"], ["13:30", "17:00"]], "sat": [["00:00", "24:00"]], "sun": [] });
        // An empty day is dropped (closed).
        assert_eq!(
            check_hours(&ok).unwrap(),
            json!({ "mon": [["09:00", "12:30"], ["13:30", "17:00"]], "sat": [["00:00", "24:00"]] })
        );
        for bad in [
            json!({ "monday": [["09:00", "17:00"]] }),
            json!({ "mon": [["17:00", "09:00"]] }),
            json!({ "mon": [["09:00", "13:00"], ["12:00", "17:00"]] }),
            json!({ "mon": [["9:00", "17:00"]] }),
            json!({ "mon": [["09:00", "24:30"]] }),
            json!({ "mon": [["09:00", "17:00", "18:00"]] }),
            json!({ "mon": "9-5" }),
            json!([]),
        ] {
            assert!(check_hours(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sites_need_a_real_timezone_and_country_code() {
        let base = SiteInput {
            name: "HQ".into(),
            ..SiteInput::default()
        };
        assert_eq!(check(&base).unwrap().timezone, "UTC");
        let london = SiteInput {
            timezone: Some("Europe/London".into()),
            country: Some("gb".into()),
            ..base.clone()
        };
        let checked = check(&london).unwrap();
        assert_eq!(checked.country.as_deref(), Some("GB"));
        assert!(
            check(&SiteInput {
                timezone: Some("Mars/Base".into()),
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            check(&SiteInput {
                country: Some("G1".into()),
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            check(&SiteInput {
                name: "  ".into(),
                ..base
            })
            .is_err()
        );
    }
}
