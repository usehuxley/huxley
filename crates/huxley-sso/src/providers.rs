//! Identity provider configuration and claim → role mappings.

use huxley_core::RequestContext;
use huxley_crypto::{Binding, Crypto};
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{SsoError, SsoResult};

const SECRET_PURPOSE: &str = "oidc_client_secret";

pub(crate) fn secret_binding(provider_id: Uuid) -> Binding<'static> {
    Binding {
        purpose: SECRET_PURPOSE,
        record: provider_id,
    }
}

/// Provider templates with sensible defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "oidc_preset", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Generic,
    /// Microsoft Entra ID, one tenant. Needs `tenantId`.
    Entra,
    Google,
    Okta,
    Authentik,
    Keycloak,
}

/// When an SSO sign-in counts as multi-factor for the MFA policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "oidc_mfa_trust", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum MfaTrust {
    /// Never: users covered by the policy also need a Huxley second factor.
    Never,
    /// When the ID token's `amr` claim includes `mfa` (Entra, Okta, and
    /// others send it).
    Amr,
    /// Always: the provider enforces MFA itself.
    Always,
}

/// A configured identity provider. The client secret is never returned.
#[allow(clippy::struct_excessive_bools)] // independent settings, one per column
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub preset: Preset,
    pub issuer_url: String,
    pub client_id: String,
    pub has_client_secret: bool,
    pub scopes: Vec<String>,
    pub enabled: bool,
    pub jit_provisioning: bool,
    pub allowed_domains: Vec<String>,
    pub trust_email: bool,
    pub role_claim: Option<String>,
    pub mfa_trust: MfaTrust,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

macro_rules! select_providers {
    ($rest:literal) => {
        concat!(
            "SELECT id, key, name, preset, issuer_url, client_id, ",
            "client_secret_enc IS NOT NULL AS has_client_secret, scopes, enabled, ",
            "jit_provisioning, allowed_domains, trust_email, role_claim, mfa_trust, version, ",
            "created_at, updated_at FROM oidc_providers ",
            $rest
        )
    };
}

/// Settings for creating or replacing a provider. Optional fields fall back
/// to the preset's defaults.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    /// Lowercase letters, digits, and dashes; appears in sign-in URLs.
    pub key: String,
    /// Shown on the sign-in button, e.g. "Microsoft".
    pub name: String,
    pub preset: Preset,
    /// Required except for `entra` (derived from `tenantId`) and `google`.
    pub issuer_url: Option<String>,
    /// Entra directory (tenant) id.
    pub tenant_id: Option<String>,
    pub client_id: String,
    /// Write-only. Omit on update to keep the stored secret.
    #[schema(format = Password)]
    pub client_secret: Option<String>,
    pub scopes: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub jit_provisioning: bool,
    /// At least one. Only addresses in these domains may sign in.
    pub allowed_domains: Vec<String>,
    pub trust_email: Option<bool>,
    pub role_claim: Option<String>,
    pub mfa_trust: Option<MfaTrust>,
}

fn default_true() -> bool {
    true
}

/// A validated input with preset defaults applied.
struct Resolved {
    key: String,
    name: String,
    issuer_url: String,
    client_id: String,
    scopes: Vec<String>,
    allowed_domains: Vec<String>,
    trust_email: bool,
    role_claim: Option<String>,
    mfa_trust: MfaTrust,
}

fn invalid(field: &'static str, message: &'static str) -> SsoError {
    SsoError::Invalid { field, message }
}

/// Plain http only for local development and tests.
fn valid_issuer(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) if u.scheme() == "https" => u.host_str().is_some(),
        Ok(u) if u.scheme() == "http" => {
            matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        }
        _ => false,
    }
}

fn resolve_issuer(input: &ProviderInput) -> SsoResult<String> {
    let issuer_url = match input.preset {
        Preset::Entra => {
            let tenant = input
                .tenant_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            let tenant_ok = !tenant.is_empty()
                && tenant.len() <= 64
                && tenant
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
            if !tenant_ok || matches!(tenant, "common" | "organizations" | "consumers") {
                return Err(invalid(
                    "tenantId",
                    "Enter your directory (tenant) id. Multi-tenant sign-in isn't supported yet.",
                ));
            }
            format!("https://login.microsoftonline.com/{tenant}/v2.0")
        }
        Preset::Google => "https://accounts.google.com".to_owned(),
        _ => input
            .issuer_url
            .as_deref()
            .map(|u| u.trim().trim_end_matches('/').to_owned())
            .unwrap_or_default(),
    };
    if !valid_issuer(&issuer_url) {
        return Err(invalid(
            "issuerUrl",
            "Enter the provider's https issuer URL.",
        ));
    }
    Ok(issuer_url)
}

fn resolve_domains(domains: &[String]) -> SsoResult<Vec<String>> {
    let mut allowed_domains: Vec<String> = domains
        .iter()
        .map(|d| d.trim().trim_start_matches('@').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    allowed_domains.sort();
    allowed_domains.dedup();
    let domains_ok = !allowed_domains.is_empty()
        && allowed_domains.iter().all(|d| {
            d.contains('.') && d.len() <= 253 && !d.chars().any(|c| c.is_whitespace() || c == '@')
        });
    if !domains_ok {
        return Err(invalid(
            "allowedDomains",
            "Enter at least one email domain, e.g. example.com.",
        ));
    }
    Ok(allowed_domains)
}

fn resolve(input: &ProviderInput) -> SsoResult<Resolved> {
    let key = input.key.trim().to_ascii_lowercase();
    let key_ok = !key.is_empty()
        && key.len() <= 40
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !key.starts_with('-');
    if !key_ok {
        return Err(invalid(
            "key",
            "Use 1 to 40 lowercase letters, digits, and dashes.",
        ));
    }
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(invalid("name", "Enter a name of 1 to 100 characters."));
    }
    let client_id = input.client_id.trim().to_owned();
    if client_id.is_empty() || client_id.len() > 500 {
        return Err(invalid("clientId", "Enter the client (application) id."));
    }
    let issuer_url = resolve_issuer(input)?;
    let allowed_domains = resolve_domains(&input.allowed_domains)?;
    let mut scopes = input
        .scopes
        .clone()
        .unwrap_or_else(|| vec!["openid".into(), "email".into(), "profile".into()]);
    if !scopes.iter().any(|s| s == "openid") {
        scopes.insert(0, "openid".into());
    }
    if scopes
        .iter()
        .any(|s| s.is_empty() || s.chars().any(char::is_whitespace))
    {
        return Err(invalid("scopes", "Scopes can't contain spaces."));
    }
    let role_claim = match &input.role_claim {
        Some(c) if c.trim().is_empty() => None,
        Some(c) => Some(c.trim().to_owned()),
        None => match input.preset {
            Preset::Entra => Some("roles".into()),
            Preset::Okta | Preset::Authentik | Preset::Keycloak => Some("groups".into()),
            Preset::Google | Preset::Generic => None,
        },
    };
    Ok(Resolved {
        key,
        name,
        issuer_url,
        client_id,
        scopes,
        allowed_domains,
        // A single Entra tenant's admins control its addresses, and Entra
        // doesn't send email_verified. Everyone else must send it.
        trust_email: input.trust_email.unwrap_or(input.preset == Preset::Entra),
        role_claim,
        mfa_trust: input.mfa_trust.unwrap_or(MfaTrust::Amr),
    })
}

fn map_unique(e: sqlx::Error) -> SsoError {
    match huxley_db::violated_constraint(&e) {
        Some("oidc_providers_key_key") => invalid("key", "Another provider uses this key."),
        _ => SsoError::from(e),
    }
}

pub async fn list(conn: &mut PgConnection) -> SsoResult<Vec<Provider>> {
    Ok(sqlx::query_as(select_providers!("ORDER BY name"))
        .fetch_all(conn)
        .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> SsoResult<Provider> {
    sqlx::query_as(select_providers!("WHERE id = $1"))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(SsoError::ProviderNotFound)
}

pub(crate) async fn get_enabled_by_key(conn: &mut PgConnection, key: &str) -> SsoResult<Provider> {
    sqlx::query_as(select_providers!("WHERE key = $1 AND enabled"))
        .bind(key)
        .fetch_optional(conn)
        .await?
        .ok_or(SsoError::ProviderNotFound)
}

async fn audit_change(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    action: &str,
    provider: &Provider,
) -> SsoResult<()> {
    audit::record(
        conn,
        ctx,
        AuditRecord::new(action)
            .target("oidc_provider", provider.id)
            .details(json!({
                "key": provider.key,
                "issuer": provider.issuer_url,
                "enabled": provider.enabled,
                "jitProvisioning": provider.jit_provisioning,
                "allowedDomains": provider.allowed_domains,
                "trustEmail": provider.trust_email,
                "mfaTrust": provider.mfa_trust,
            })),
    )
    .await?;
    Ok(())
}

/// Creates a provider. Run in system scope after the permission check.
pub async fn create(
    conn: &mut PgConnection,
    crypto: &Crypto,
    root_org: Uuid,
    ctx: &RequestContext,
    input: &ProviderInput,
) -> SsoResult<Provider> {
    let r = resolve(input)?;
    let id = huxley_core::new_id();
    let secret = match input.client_secret.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(
            crypto
                .encrypt(root_org, secret_binding(id), s.as_bytes())
                .await?,
        ),
        None => None,
    };
    sqlx::query(
        "INSERT INTO oidc_providers (id, key, name, preset, issuer_url, client_id, client_secret_enc, \
            scopes, enabled, jit_provisioning, allowed_domains, trust_email, role_claim, mfa_trust) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(id)
    .bind(&r.key)
    .bind(&r.name)
    .bind(input.preset)
    .bind(&r.issuer_url)
    .bind(&r.client_id)
    .bind(secret)
    .bind(&r.scopes)
    .bind(input.enabled)
    .bind(input.jit_provisioning)
    .bind(&r.allowed_domains)
    .bind(r.trust_email)
    .bind(&r.role_claim)
    .bind(r.mfa_trust)
    .execute(&mut *conn)
    .await
    .map_err(map_unique)?;
    let provider = get(conn, id).await?;
    audit_change(conn, ctx, "sso.provider_created", &provider).await?;
    Ok(provider)
}

/// Replaces a provider's settings. Omitting `clientSecret` keeps the stored
/// one. Run in system scope after the permission check.
pub async fn update(
    conn: &mut PgConnection,
    crypto: &Crypto,
    root_org: Uuid,
    ctx: &RequestContext,
    id: Uuid,
    input: &ProviderInput,
) -> SsoResult<Provider> {
    get(conn, id).await?;
    let r = resolve(input)?;
    let secret = match input.client_secret.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(
            crypto
                .encrypt(root_org, secret_binding(id), s.as_bytes())
                .await?,
        ),
        None => None,
    };
    sqlx::query(
        "UPDATE oidc_providers SET key = $2, name = $3, preset = $4, issuer_url = $5, client_id = $6, \
            client_secret_enc = coalesce($7, client_secret_enc), scopes = $8, enabled = $9, \
            jit_provisioning = $10, allowed_domains = $11, trust_email = $12, role_claim = $13, \
            mfa_trust = $14, version = version + 1 \
          WHERE id = $1",
    )
    .bind(id)
    .bind(&r.key)
    .bind(&r.name)
    .bind(input.preset)
    .bind(&r.issuer_url)
    .bind(&r.client_id)
    .bind(secret)
    .bind(&r.scopes)
    .bind(input.enabled)
    .bind(input.jit_provisioning)
    .bind(&r.allowed_domains)
    .bind(r.trust_email)
    .bind(&r.role_claim)
    .bind(r.mfa_trust)
    .execute(&mut *conn)
    .await
    .map_err(map_unique)?;
    let provider = get(conn, id).await?;
    audit_change(conn, ctx, "sso.provider_updated", &provider).await?;
    Ok(provider)
}

/// Deletes a provider, its linked identities, and the role assignments its
/// mappings created. Users themselves are kept.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> SsoResult<()> {
    let provider = get(conn, id).await?;
    sqlx::query("DELETE FROM oidc_providers WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit_change(conn, ctx, "sso.provider_deleted", &provider).await
}

/// A claim value → role at an org.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RoleMapping {
    /// The group id, app role, or other value of the provider's role claim.
    pub claim_value: String,
    pub role_id: Uuid,
    pub org_id: Uuid,
    #[serde(default = "default_true")]
    pub inherit: bool,
}

pub async fn role_mappings(
    conn: &mut PgConnection,
    provider_id: Uuid,
) -> SsoResult<Vec<RoleMapping>> {
    Ok(sqlx::query_as(
        "SELECT m.claim_value, m.role_id, m.org_id, m.inherit FROM oidc_role_mappings m \
           JOIN organizations o ON o.id = m.org_id \
          WHERE m.provider_id = $1 ORDER BY m.claim_value, o.path",
    )
    .bind(provider_id)
    .fetch_all(conn)
    .await?)
}

/// Replaces a provider's role mappings. The Owner role can't be mapped: it
/// is granted by hand only. Takes effect at each user's next sign-in.
pub async fn set_role_mappings(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider_id: Uuid,
    mappings: &[RoleMapping],
) -> SsoResult<()> {
    get(conn, provider_id).await?;
    for m in mappings {
        if m.claim_value.trim().is_empty() || m.claim_value.len() > 500 {
            return Err(invalid("claimValue", "Enter the claim value to match."));
        }
        let role: Option<(String,)> = sqlx::query_as("SELECT key FROM roles WHERE id = $1")
            .bind(m.role_id)
            .fetch_optional(&mut *conn)
            .await?;
        match role {
            None => return Err(invalid("roleId", "Unknown role.")),
            Some((key,)) if key == "owner" => {
                return Err(invalid(
                    "roleId",
                    "The Owner role can only be granted by hand.",
                ));
            }
            Some(_) => {}
        }
        let org: Option<(bool,)> =
            sqlx::query_as("SELECT true FROM organizations WHERE id = $1 AND status = 'active'")
                .bind(m.org_id)
                .fetch_optional(&mut *conn)
                .await?;
        if org.is_none() {
            return Err(invalid("orgId", "Unknown organization."));
        }
    }
    sqlx::query("DELETE FROM oidc_role_mappings WHERE provider_id = $1")
        .bind(provider_id)
        .execute(&mut *conn)
        .await?;
    for m in mappings {
        sqlx::query(
            "INSERT INTO oidc_role_mappings (id, provider_id, claim_value, role_id, org_id, inherit) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(huxley_core::new_id())
        .bind(provider_id)
        .bind(m.claim_value.trim())
        .bind(m.role_id)
        .bind(m.org_id)
        .bind(m.inherit)
        .execute(&mut *conn)
        .await?;
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sso.role_mappings_changed")
            .target("oidc_provider", provider_id)
            .details(json!({ "mappings": mappings })),
    )
    .await?;
    Ok(())
}

/// A sign-in button: the enabled providers, without configuration details.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ProviderButton {
    pub key: String,
    pub name: String,
    pub preset: Preset,
}

pub async fn buttons(conn: &mut PgConnection) -> SsoResult<Vec<ProviderButton>> {
    Ok(
        sqlx::query_as("SELECT key, name, preset FROM oidc_providers WHERE enabled ORDER BY name")
            .fetch_all(conn)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(preset: Preset) -> ProviderInput {
        ProviderInput {
            key: "corp".into(),
            name: "Corp".into(),
            preset,
            issuer_url: Some("https://idp.example.com/".into()),
            tenant_id: Some("11111111-2222-3333-4444-555555555555".into()),
            client_id: "client".into(),
            client_secret: None,
            scopes: None,
            enabled: true,
            jit_provisioning: false,
            allowed_domains: vec![" @Example.com ".into(), "example.com".into()],
            trust_email: None,
            role_claim: None,
            mfa_trust: None,
        }
    }

    #[test]
    fn presets_fill_defaults() {
        let entra = resolve(&input(Preset::Entra)).unwrap();
        assert_eq!(
            entra.issuer_url,
            "https://login.microsoftonline.com/11111111-2222-3333-4444-555555555555/v2.0"
        );
        assert!(entra.trust_email);
        assert_eq!(entra.role_claim.as_deref(), Some("roles"));
        assert_eq!(entra.allowed_domains, vec!["example.com"]);
        let google = resolve(&input(Preset::Google)).unwrap();
        assert_eq!(google.issuer_url, "https://accounts.google.com");
        assert!(!google.trust_email);
        let okta = resolve(&input(Preset::Okta)).unwrap();
        assert_eq!(okta.issuer_url, "https://idp.example.com");
        assert_eq!(okta.role_claim.as_deref(), Some("groups"));
        assert_eq!(okta.scopes, vec!["openid", "email", "profile"]);
    }

    #[test]
    fn validation() {
        let mut i = input(Preset::Entra);
        i.tenant_id = Some("common".into());
        assert!(resolve(&i).is_err(), "multi-tenant is refused");
        let mut i = input(Preset::Generic);
        i.issuer_url = Some("http://idp.example.com".into());
        assert!(resolve(&i).is_err(), "plain http only for localhost");
        i.issuer_url = Some("http://127.0.0.1:9000".into());
        assert!(resolve(&i).is_ok());
        let mut i = input(Preset::Generic);
        i.allowed_domains = vec![];
        assert!(resolve(&i).is_err(), "a domain is required");
        let mut i = input(Preset::Generic);
        i.key = "Not OK".into();
        assert!(resolve(&i).is_err());
    }
}
