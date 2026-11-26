//! Organization settings with inheritance and lock (design doc §5.2-5.3).
//!
//! A value set at an org applies to it and every descendant; the nearest
//! wins. A value can be **locked**, and then descendants can't override it:
//! the shallowest lock wins, and writing below one is refused. Keys and
//! their types are registered here, like permissions. Run in system scope
//! after the caller's permission check (resolution reads ancestors).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{OrgError, OrgResult};

/// How a setting's value is validated (and edited in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum SettingKind {
    /// Free text, up to 100 characters.
    Text,
    /// An IANA time zone name, e.g. `Europe/Berlin`.
    Timezone,
    /// `#rrggbb`.
    Color,
    Email,
    /// A short uppercase prefix for numbers, e.g. `T` in `T-1042`.
    Prefix,
    /// A small image as a `data:` URL (PNG, JPEG, WebP or SVG, at most
    /// 200 KB).
    Image,
    /// A host name, e.g. `support.example.com` (a port allowed for local
    /// use).
    Hostname,
    /// On or off: `true` or `false`.
    Toggle,
    /// A whole percentage, 0 to 100.
    Percent,
    /// A number of days, 1 to 3650.
    Days,
}

/// The largest image setting, as a `data:` URL.
pub const MAX_IMAGE_CHARS: usize = 280_000;

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingDef {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: SettingKind,
    /// Used when no org in the chain sets a value.
    pub default: Option<&'static str>,
}

macro_rules! settings {
    ($( $ident:ident = $key:literal, $group:literal, $kind:ident, $default:expr, $label:literal, $desc:literal; )*) => {
        $( pub const $ident: &str = $key; )*
        /// Every setting, in display order.
        pub const ALL: &[SettingDef] = &[
            $( SettingDef { key: $key, group: $group, label: $label, description: $desc, kind: SettingKind::$kind, default: $default }, )*
        ];
    };
}

settings! {
    TIMEZONE = "general.timezone", "General", Timezone, Some("UTC"), "Time zone",
        "Used for business hours, schedules, and dates in emails.";
    BRAND_NAME = "branding.name", "Branding", Text, None, "Brand name",
        "Shown to clients in the portal and emails. Defaults to the organization's name.";
    BRAND_ACCENT = "branding.accent_color", "Branding", Color, Some("#4f46e5"), "Accent color",
        "Buttons and links in the portal and emails.";
    BRAND_SUPPORT_EMAIL = "branding.support_email", "Branding", Email, None, "Support email",
        "Where clients are told to write for help.";
    BRAND_LOGO = "branding.logo", "Branding", Image, None, "Logo",
        "Shown in the client portal. PNG, JPEG, WebP or SVG, up to 200 KB.";
    PORTAL_DOMAIN = "portal.domain", "Branding", Hostname, None, "Portal domain",
        "The client portal's address for this organization and those below, e.g. support.example.com. Point it at the portal, and sign-in links use it.";
    CSAT_ENABLED = "csat.enabled", "Customer experience", Toggle, Some("true"), "Satisfaction surveys",
        "Ask requesters how it went when their ticket is resolved (with the Customer experience module and its pack). Turn off for a company to opt it out.";
    CSAT_SAMPLE = "csat.sample_percent", "Customer experience", Percent, Some("100"), "Survey this share of tickets",
        "Ask about this percentage of resolved tickets, at random, so regular requesters aren't asked every time.";
    CSAT_THUMBS = "csat.thumbs", "Customer experience", Toggle, Some("false"), "Thumbs up or down",
        "Ask for a thumbs up or down instead of a rating from 1 to 5.";
    NPS_ENABLED = "nps.enabled", "Customer experience", Toggle, Some("true"), "Net promoter surveys",
        "Ask contacts how likely they are to recommend you (with the Net promoter surveys pack). Turn off for a company to opt it out.";
    NPS_INTERVAL = "nps.interval_days", "Customer experience", Days, Some("90"), "Ask the same person at most every",
        "Days between net promoter questions to one contact.";
    NPS_ACTIVE = "nps.active_days", "Customer experience", Days, Some("180"), "Ask people who raised a ticket within",
        "Only contacts who raised a ticket in this many days are asked.";
    TRIAGE_TARGET = "triage.accuracy_target", "Service desk", Percent, Some("80"), "AI triage accuracy target",
        "The share of labeled samples the AI triage pack must route to the right queue for an evaluation to pass.";
    PORTAL_REQUIRE_MFA = "portal.require_mfa", "Portal", Toggle, Some("false"), "Require two-factor sign-in",
        "Contacts who sign in to the portal with a link or password must set up an authenticator app. Signing in with their organization's account is left to their organization.";
    TICKET_PREFIX = "numbering.ticket_prefix", "Numbering", Prefix, Some("T"), "Ticket prefix",
        "Ticket numbers look like T-1042.";
    QUOTE_PREFIX = "numbering.quote_prefix", "Numbering", Prefix, Some("Q"), "Quote prefix",
        "Quote numbers look like Q-1042.";
    INVOICE_PREFIX = "numbering.invoice_prefix", "Numbering", Prefix, Some("INV"), "Invoice prefix",
        "Invoice numbers look like INV-1042.";
}

pub fn def(key: &str) -> Option<&'static SettingDef> {
    ALL.iter().find(|d| d.key == key)
}

/// Checks and normalizes a value for `kind`.
pub fn validate(kind: SettingKind, raw: &str) -> Result<String, String> {
    let value = raw.trim();
    let ok = match kind {
        SettingKind::Text => {
            !value.is_empty()
                && value.chars().count() <= 100
                && !value.chars().any(char::is_control)
        }
        SettingKind::Timezone => {
            value == "UTC"
                || (value.len() <= 64
                    && value.contains('/')
                    && value.split('/').all(|part| {
                        part.starts_with(|c: char| c.is_ascii_uppercase())
                            && part
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "_+-".contains(c))
                    }))
        }
        SettingKind::Color => {
            value.len() == 7
                && value.starts_with('#')
                && value[1..].chars().all(|c| c.is_ascii_hexdigit())
        }
        SettingKind::Email => {
            value.len() <= 254
                && value.split_once('@').is_some_and(|(local, domain)| {
                    !local.is_empty()
                        && domain.contains('.')
                        && !domain.starts_with('.')
                        && !domain.ends_with('.')
                })
                && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        }
        SettingKind::Image => {
            value.len() <= MAX_IMAGE_CHARS
                && ["image/png", "image/jpeg", "image/webp", "image/svg+xml"]
                    .iter()
                    .any(|t| {
                        value
                            .strip_prefix("data:")
                            .and_then(|v| v.strip_prefix(t))
                            .and_then(|v| v.strip_prefix(";base64,"))
                            .is_some_and(|b64| {
                                !b64.is_empty()
                                    && b64
                                        .bytes()
                                        .all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c))
                            })
                    })
        }
        SettingKind::Hostname => {
            let (host, port) = value.split_once(':').unwrap_or((value, ""));
            !host.is_empty()
                && host.len() <= 253
                && (port.is_empty()
                    || (port.len() <= 5 && port.bytes().all(|c| c.is_ascii_digit())))
                && host.split('.').all(|label| {
                    (1..=63).contains(&label.len())
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                })
        }
        SettingKind::Toggle => matches!(value.to_ascii_lowercase().as_str(), "true" | "false"),
        SettingKind::Percent => value.parse::<u8>().is_ok_and(|p| p <= 100),
        SettingKind::Days => value.parse::<u16>().is_ok_and(|d| (1..=3650).contains(&d)),
        SettingKind::Prefix => {
            (1..=8).contains(&value.len())
                && value.starts_with(|c: char| c.is_ascii_uppercase())
                && value
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        }
    };
    if !ok {
        return Err(match kind {
            SettingKind::Text => "Enter 1 to 100 characters.",
            SettingKind::Timezone => "Choose a time zone such as Europe/Berlin.",
            SettingKind::Color => "Use a hex color such as #4f46e5.",
            SettingKind::Email => "Enter an email address.",
            SettingKind::Prefix => {
                "Use 1 to 8 uppercase letters or digits, starting with a letter."
            }
            SettingKind::Image => "Choose a PNG, JPEG, WebP or SVG image up to 200 KB.",
            SettingKind::Hostname => "Enter a host name such as support.example.com.",
            SettingKind::Toggle => "Use true or false.",
            SettingKind::Percent => "Enter a whole percentage from 0 to 100.",
            SettingKind::Days => "Enter a number of days from 1 to 3,650.",
        }
        .into());
    }
    Ok(match kind {
        SettingKind::Color | SettingKind::Hostname | SettingKind::Toggle => {
            value.to_ascii_lowercase()
        }
        _ => value.to_owned(),
    })
}

/// An org in a resolution.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgRef {
    pub id: Uuid,
    pub name: String,
}

/// A value set at the org itself.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalSetting {
    pub value: String,
    pub locked: bool,
}

/// A setting's effective value at an org, and why.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSetting {
    #[serde(flatten)]
    pub def: SettingDef,
    /// The effective value (`None`: unset, use the documented fallback).
    pub value: Option<String>,
    /// Where `value` comes from; `None` means the built-in default.
    pub source: Option<OrgRef>,
    /// Set when an ancestor locked this key: it can't be changed here.
    pub locked_by: Option<OrgRef>,
    /// The value set at this org, if any (ignored while an ancestor's lock applies).
    pub local: Option<LocalSetting>,
}

#[derive(sqlx::FromRow)]
struct Candidate {
    key: String,
    value: serde_json::Value,
    locked: bool,
    org_id: Uuid,
    org_name: String,
}

/// Every setting at `org_id`. One query over the org and its ancestors.
pub async fn resolve(conn: &mut PgConnection, org_id: Uuid) -> OrgResult<Vec<ResolvedSetting>> {
    crate::get(conn, org_id).await?;
    // Nearest first.
    let candidates: Vec<Candidate> = sqlx::query_as(
        "SELECT s.key, s.value, s.locked, s.org_id, o.name AS org_name \
           FROM org_settings s \
           JOIN organizations o ON o.id = s.org_id \
          WHERE o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path) DESC",
    )
    .bind(org_id)
    .fetch_all(conn)
    .await?;
    Ok(ALL
        .iter()
        .map(|def| {
            let chain: Vec<&Candidate> = candidates.iter().filter(|c| c.key == def.key).collect();
            let as_text = |v: &serde_json::Value| v.as_str().map(str::to_owned);
            let local = chain.iter().find(|c| c.org_id == org_id).and_then(|c| {
                as_text(&c.value).map(|value| LocalSetting {
                    value,
                    locked: c.locked,
                })
            });
            // The shallowest lock wins; otherwise the nearest value.
            let winner = chain
                .iter()
                .rev()
                .find(|c| c.locked)
                .or_else(|| chain.first());
            let locked_by = chain
                .iter()
                .rev()
                .find(|c| c.locked && c.org_id != org_id)
                .map(|c| OrgRef {
                    id: c.org_id,
                    name: c.org_name.clone(),
                });
            ResolvedSetting {
                def: *def,
                value: winner
                    .and_then(|c| as_text(&c.value))
                    .or_else(|| def.default.map(str::to_owned)),
                source: winner.map(|c| OrgRef {
                    id: c.org_id,
                    name: c.org_name.clone(),
                }),
                locked_by,
                local,
            }
        })
        .collect())
}

/// One setting's effective value at `org_id` (`None`: unset).
pub async fn value(conn: &mut PgConnection, org_id: Uuid, key: &str) -> OrgResult<Option<String>> {
    Ok(resolve(conn, org_id)
        .await?
        .into_iter()
        .find(|s| s.def.key == key)
        .and_then(|s| s.value))
}

async fn ancestor_lock(
    conn: &mut PgConnection,
    org_id: Uuid,
    key: &str,
) -> OrgResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT o.name FROM org_settings s \
           JOIN organizations o ON o.id = s.org_id \
          WHERE s.key = $2 AND s.locked AND s.org_id <> $1 \
            AND o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path) LIMIT 1",
    )
    .bind(org_id)
    .bind(key)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(name,)| name))
}

/// Sets `key` at `org_id` (for it and its descendants), optionally locked.
pub async fn set(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    key: &str,
    raw: &str,
    locked: bool,
) -> OrgResult<()> {
    let def = def(key).ok_or(OrgError::UnknownSetting)?;
    let value = validate(def.kind, raw).map_err(OrgError::InvalidSetting)?;
    crate::get(conn, org_id).await?;
    if let Some(by) = ancestor_lock(conn, org_id, key).await? {
        return Err(OrgError::SettingLocked(by));
    }
    if def.kind == SettingKind::Hostname {
        let taken: Option<String> = sqlx::query_scalar(
            "SELECT o.name FROM org_settings s JOIN organizations o ON o.id = s.org_id \
              WHERE s.key = $1 AND s.value = $2 AND s.org_id <> $3",
        )
        .bind(key)
        .bind(json!(value))
        .bind(org_id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(other) = taken {
            return Err(OrgError::InvalidSetting(format!(
                "{other} already uses {value}."
            )));
        }
    }
    sqlx::query(
        "INSERT INTO org_settings (org_id, key, value, locked, updated_by) VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (org_id, key) DO UPDATE \
            SET value = EXCLUDED.value, locked = EXCLUDED.locked, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(org_id)
    .bind(key)
    .bind(json!(value))
    .bind(locked)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("org.setting_changed")
            .org(org_id)
            .target("organization", org_id)
            .details(json!({ "key": key, "value": value, "locked": locked })),
    )
    .await?;
    Ok(())
}

/// Removes the value set at `org_id`, so it inherits again.
pub async fn clear(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    key: &str,
) -> OrgResult<()> {
    def(key).ok_or(OrgError::UnknownSetting)?;
    crate::get(conn, org_id).await?;
    let removed = sqlx::query("DELETE FROM org_settings WHERE org_id = $1 AND key = $2")
        .bind(org_id)
        .bind(key)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if removed > 0 {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("org.setting_cleared")
                .org(org_id)
                .target("organization", org_id)
                .details(json!({ "key": key })),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_validated_per_kind() {
        assert_eq!(
            validate(SettingKind::Color, " #ABCDEF ").unwrap(),
            "#abcdef"
        );
        assert!(validate(SettingKind::Color, "red").is_err());
        assert!(validate(SettingKind::Timezone, "Europe/Berlin").is_ok());
        assert!(validate(SettingKind::Timezone, "America/Argentina/Buenos_Aires").is_ok());
        assert!(validate(SettingKind::Timezone, "UTC").is_ok());
        assert!(validate(SettingKind::Timezone, "../etc/passwd").is_err());
        assert!(validate(SettingKind::Prefix, "INV2").is_ok());
        assert!(validate(SettingKind::Prefix, "inv").is_err());
        assert!(validate(SettingKind::Prefix, "TOOLONGPFX").is_err());
        assert!(validate(SettingKind::Email, "help@acme.example").is_ok());
        assert!(validate(SettingKind::Email, "nope").is_err());
        assert!(validate(SettingKind::Text, "").is_err());
        assert_eq!(
            validate(SettingKind::Hostname, "Support.Example.com").unwrap(),
            "support.example.com"
        );
        assert!(validate(SettingKind::Hostname, "localhost:4174").is_ok());
        assert!(validate(SettingKind::Hostname, "https://x.example").is_err());
        assert!(validate(SettingKind::Hostname, "bad_host.example").is_err());
        assert!(validate(SettingKind::Image, "data:image/png;base64,iVBORw0KGgo=").is_ok());
        assert!(validate(SettingKind::Image, "data:text/html;base64,PHNjcmlwdD4=").is_err());
        assert!(validate(SettingKind::Image, "data:image/png;base64,<script>").is_err());
        assert!(validate(SettingKind::Image, "https://evil.example/logo.png").is_err());
        let huge = format!("data:image/png;base64,{}", "A".repeat(MAX_IMAGE_CHARS));
        assert!(validate(SettingKind::Image, &huge).is_err());
    }

    #[test]
    fn registry_keys_are_unique_and_defaults_valid() {
        let mut keys: Vec<_> = ALL.iter().map(|d| d.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), ALL.len());
        for d in ALL {
            if let Some(default) = d.default {
                assert_eq!(
                    validate(d.kind, default).as_deref(),
                    Ok(default),
                    "{}",
                    d.key
                );
            }
        }
    }
}
