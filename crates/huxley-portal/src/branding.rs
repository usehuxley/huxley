//! How the portal looks for an organization: its brand name, accent
//! color, logo and support email, from the organization settings (set at the
//! MSP, overridable per client), and which organization a portal domain
//! belongs to.

use huxley_orgs::settings;
use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{PortalError, PortalResult};

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalBranding)]
pub struct Branding {
    /// The brand name (the organization's name if none is set).
    pub name: String,
    /// `#rrggbb`.
    pub accent_color: String,
    /// A `data:` URL, if a logo is set.
    pub logo: Option<String>,
    pub support_email: Option<String>,
}

/// The branding that applies at `org_id` (system scope: settings inherit
/// from above).
pub async fn at(conn: &mut PgConnection, org_id: Uuid) -> PortalResult<Branding> {
    let resolved = settings::resolve(conn, org_id)
        .await
        .map_err(|e| PortalError::Sqlx(sqlx::Error::Protocol(e.to_string())))?;
    let get = |key: &str| {
        resolved
            .iter()
            .find(|s| s.def.key == key)
            .and_then(|s| s.value.clone())
    };
    let name = match get(settings::BRAND_NAME) {
        Some(name) => name,
        None => sqlx::query_scalar("SELECT name FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or_else(|| "Support portal".into()),
    };
    Ok(Branding {
        name,
        accent_color: get(settings::BRAND_ACCENT).unwrap_or_else(|| "#4f46e5".into()),
        logo: get(settings::BRAND_LOGO),
        support_email: get(settings::BRAND_SUPPORT_EMAIL),
    })
}

/// The organization whose portal domain is `host` (set on it directly),
/// else the root organization.
pub async fn org_for_host(
    conn: &mut PgConnection,
    host: Option<&str>,
) -> PortalResult<Option<Uuid>> {
    if let Some(host) = host
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
    {
        let found: Option<Uuid> = sqlx::query_scalar(
            "SELECT org_id FROM org_settings WHERE key = $1 AND value = to_jsonb($2::text)",
        )
        .bind(settings::PORTAL_DOMAIN)
        .bind(&host)
        .fetch_optional(&mut *conn)
        .await?;
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(sqlx::query_scalar(
        "SELECT id FROM organizations WHERE parent_id IS NULL ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(conn)
    .await?)
}

/// The portal's base URL for an organization: its portal domain (inherited
/// from above), as `https://…` (`http://` for `localhost`), if one is set.
pub async fn base_url(conn: &mut PgConnection, org_id: Uuid) -> PortalResult<Option<String>> {
    let domain = settings::value(conn, org_id, settings::PORTAL_DOMAIN)
        .await
        .map_err(|e| PortalError::Sqlx(sqlx::Error::Protocol(e.to_string())))?;
    Ok(domain.map(|d| {
        let scheme = if d == "localhost" || d.starts_with("localhost:") {
            "http"
        } else {
            "https"
        };
        format!("{scheme}://{d}")
    }))
}
