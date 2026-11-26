//! Signing in to the portal with a client's own identity provider (ADR-040):
//! the client's Entra tenant, say, configured on the client (or the MSP) and
//! matched by the email domains it signs in. A contact must already have
//! portal access, or the provider may give contacts with a matching address
//! an account on their first sign-in.

use huxley_core::RequestContext;
use huxley_crypto::{Binding, Crypto};
use huxley_db::audit::{self, AuditRecord};
use huxley_sso::{RedirectUrl, SsoService};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::signin::{NewSession, start_session};
use crate::{PortalError, PortalResult, Role};

/// Sign-in state tokens.
pub const STATE_PREFIX: &str = "hxpo_";
const STATE_TTL: Duration = Duration::minutes(10);
const SECRET_PURPOSE: &str = "portal_oidc_client_secret";

fn binding(id: Uuid) -> Binding<'static> {
    Binding {
        purpose: SECRET_PURPOSE,
        record: id,
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalIdentityProvider)]
#[allow(clippy::struct_excessive_bools)]
pub struct Provider {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    /// `entra` or `generic`.
    pub preset: String,
    pub issuer_url: String,
    pub client_id: String,
    pub has_secret: bool,
    pub email_domains: Vec<String>,
    pub trust_email: bool,
    pub provision: bool,
    pub default_role: Role,
    pub enabled: bool,
    pub version: i32,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = PortalIdentityProviderInput)]
#[allow(clippy::struct_excessive_bools)]
pub struct ProviderInput {
    pub name: String,
    /// `entra` (default) or `generic`.
    #[serde(default)]
    pub preset: Option<String>,
    /// e.g. `https://login.microsoftonline.com/<tenant id>/v2.0`.
    pub issuer_url: String,
    pub client_id: String,
    /// Left out: unchanged. Empty: removed.
    #[serde(default)]
    pub client_secret: Option<String>,
    pub email_domains: Vec<String>,
    #[serde(default = "yes")]
    pub trust_email: bool,
    #[serde(default)]
    pub provision: bool,
    #[serde(default)]
    pub default_role: Option<Role>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT id, org_id, name, preset, issuer_url, client_id, \
    client_secret_enc IS NOT NULL AS has_secret, email_domains, trust_email, provision, default_role, \
    enabled, version FROM portal_identity_providers";

pub async fn list(conn: &mut PgConnection, org_id: Uuid) -> PortalResult<Vec<Provider>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE org_id = $1 ORDER BY name"
    )))
    .bind(org_id)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> PortalResult<Provider> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(PortalError::NotFound)
}

/// The enabled provider for an email address's domain, if any.
pub async fn for_email(conn: &mut PgConnection, email: &str) -> PortalResult<Option<Provider>> {
    let Some((_, domain)) = email.trim().rsplit_once('@') else {
        return Ok(None);
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE enabled AND $1 = ANY(email_domains) LIMIT 1"
    )))
    .bind(domain.to_ascii_lowercase())
    .fetch_optional(conn)
    .await?)
}

struct Checked {
    name: String,
    preset: String,
    domains: Vec<String>,
}

fn check(input: &ProviderInput) -> PortalResult<Checked> {
    let bad = PortalError::invalid;
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(bad("name", "1 to 100 characters"));
    }
    let preset = input.preset.clone().unwrap_or_else(|| "entra".into());
    if !matches!(preset.as_str(), "entra" | "generic") {
        return Err(bad("preset", "entra or generic"));
    }
    let issuer = input.issuer_url.trim();
    if !(issuer.starts_with("https://")
        || issuer.starts_with("http://localhost")
        || issuer.starts_with("http://127.0.0.1"))
    {
        return Err(bad("issuerUrl", "an https:// issuer URL"));
    }
    if input.client_id.trim().is_empty() || input.client_id.len() > 500 {
        return Err(bad("clientId", "the app's client ID"));
    }
    let mut domains: Vec<String> = Vec::new();
    for d in &input.email_domains {
        let d = d.trim().trim_start_matches('@').to_ascii_lowercase();
        if d.is_empty()
            || !d.contains('.')
            || d.len() > 253
            || !d
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
        {
            return Err(PortalError::invalid(
                "emailDomains",
                format!("{d:?} isn't a domain"),
            ));
        }
        if !domains.contains(&d) {
            domains.push(d);
        }
    }
    if domains.is_empty() {
        return Err(bad("emailDomains", "at least one email domain it signs in"));
    }
    Ok(Checked {
        name,
        preset,
        domains,
    })
}

async fn domains_free(
    conn: &mut PgConnection,
    domains: &[String],
    except: Option<Uuid>,
) -> PortalResult<()> {
    let taken: Option<String> = sqlx::query_scalar(
        "SELECT name FROM portal_identity_providers WHERE email_domains && $1 AND id IS DISTINCT FROM $2 LIMIT 1",
    )
    .bind(domains)
    .bind(except)
    .fetch_optional(conn)
    .await?;
    match taken {
        Some(other) => Err(PortalError::Conflict(format!(
            "{other} already signs in one of those domains"
        ))),
        None => Ok(()),
    }
}

/// Adds a provider for a company (and those below it). The secret is
/// encrypted before anything is written.
pub async fn create(
    conn: &mut PgConnection,
    crypto: &Crypto,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &ProviderInput,
) -> PortalResult<Provider> {
    let c = check(input)?;
    domains_free(&mut *conn, &c.domains, None).await?;
    let id = huxley_core::new_id();
    let secret = match input
        .client_secret
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => Some(
            crypto
                .encrypt(org_id, binding(id), s.as_bytes())
                .await
                .map_err(|e| PortalError::Sqlx(sqlx::Error::Protocol(e.to_string())))?,
        ),
        None => None,
    };
    sqlx::query(
        "INSERT INTO portal_identity_providers (id, org_id, name, preset, issuer_url, client_id, client_secret_enc, \
            email_domains, trust_email, provision, default_role, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.name)
    .bind(&c.preset)
    .bind(input.issuer_url.trim())
    .bind(input.client_id.trim())
    .bind(secret)
    .bind(&c.domains)
    .bind(input.trust_email)
    .bind(input.provision)
    .bind(input.default_role.unwrap_or(Role::ClientUser))
    .bind(input.enabled)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.identity_provider_created")
            .org(org_id)
            .target("portal_identity_provider", id)
            .details(json!({ "name": c.name, "domains": c.domains })),
    )
    .await?;
    get(conn, id).await
}

/// Changes a provider (send the version you read). A left-out secret is
/// kept; an empty one is removed.
pub async fn update(
    conn: &mut PgConnection,
    crypto: &Crypto,
    ctx: &RequestContext,
    id: Uuid,
    input: &ProviderInput,
) -> PortalResult<Provider> {
    let c = check(input)?;
    let current = get(&mut *conn, id).await?;
    domains_free(&mut *conn, &c.domains, Some(id)).await?;
    let secret: Option<Option<Vec<u8>>> = match input.client_secret.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(s) => Some(Some(
            crypto
                .encrypt(current.org_id, binding(id), s.as_bytes())
                .await
                .map_err(|e| PortalError::Sqlx(sqlx::Error::Protocol(e.to_string())))?,
        )),
    };
    let changed = sqlx::query(
        "UPDATE portal_identity_providers SET name = $2, preset = $3, issuer_url = $4, client_id = $5, \
                client_secret_enc = CASE WHEN $6 THEN $7 ELSE client_secret_enc END, \
                email_domains = $8, trust_email = $9, provision = $10, default_role = $11, enabled = $12, \
                version = version + 1, updated_at = now() \
          WHERE id = $1 AND ($13::integer IS NULL OR version = $13)",
    )
    .bind(id)
    .bind(&c.name)
    .bind(&c.preset)
    .bind(input.issuer_url.trim())
    .bind(input.client_id.trim())
    .bind(secret.is_some())
    .bind(secret.flatten())
    .bind(&c.domains)
    .bind(input.trust_email)
    .bind(input.provision)
    .bind(input.default_role.unwrap_or(Role::ClientUser))
    .bind(input.enabled)
    .bind(input.version)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(PortalError::VersionConflict);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.identity_provider_updated")
            .org(current.org_id)
            .target("portal_identity_provider", id)
            .details(json!({ "name": c.name, "domains": c.domains, "enabled": input.enabled })),
    )
    .await?;
    get(conn, id).await
}

pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> PortalResult<()> {
    let current = get(&mut *conn, id).await?;
    sqlx::query("DELETE FROM portal_identity_providers WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.identity_provider_deleted")
            .org(current.org_id)
            .target("portal_identity_provider", id)
            .details(json!({ "name": current.name })),
    )
    .await?;
    Ok(())
}

fn redirect_url(redirect: &str) -> PortalResult<RedirectUrl> {
    RedirectUrl::new(redirect.to_owned()).map_err(|_| PortalError::SignIn("redirect"))
}

/// Starts a sign-in with `provider`, returning to `redirect` (the portal's
/// callback page): the provider's URL and the state token (for a cookie).
pub async fn start(
    conn: &mut PgConnection,
    sso: &SsoService,
    provider: &Provider,
    redirect: &str,
) -> PortalResult<(String, String)> {
    let scopes = vec![
        "openid".to_owned(),
        "email".to_owned(),
        "profile".to_owned(),
    ];
    let auth = sso
        .authorize(
            &provider.issuer_url,
            &provider.client_id,
            &scopes,
            redirect_url(redirect)?,
            STATE_PREFIX,
        )
        .await
        .map_err(|_| PortalError::SignIn("provider"))?;
    sqlx::query("DELETE FROM portal_oidc_states WHERE expires_at < now() - interval '1 hour'")
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO portal_oidc_states (id, token_hash, provider_id, nonce, pkce_verifier, redirect_uri, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(huxley_core::new_id())
    .bind(&auth.state_hash)
    .bind(provider.id)
    .bind(&auth.nonce)
    .bind(&auth.pkce_verifier)
    .bind(redirect)
    .bind(OffsetDateTime::now_utc() + STATE_TTL)
    .execute(conn)
    .await?;
    Ok((auth.url, auth.state))
}

/// Finishes a sign-in from the provider's callback: verifies the ID token,
/// finds (or, if the provider provisions, creates) the contact's portal
/// account, links the identity and starts a portal session.
#[allow(clippy::too_many_arguments)]
pub async fn callback(
    conn: &mut PgConnection,
    sso: &SsoService,
    crypto: &Crypto,
    ctx: &RequestContext,
    state: &str,
    code: &str,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> PortalResult<NewSession> {
    if !huxley_auth::token::looks_valid(state, STATE_PREFIX) {
        return Err(PortalError::SignIn("state"));
    }
    let row: Option<(Uuid, String, String, String)> = sqlx::query_as(
        "DELETE FROM portal_oidc_states WHERE token_hash = $1 AND expires_at > now() \
         RETURNING provider_id, nonce, pkce_verifier, redirect_uri",
    )
    .bind(huxley_auth::token::hash(state))
    .fetch_optional(&mut *conn)
    .await?;
    let (provider_id, nonce, verifier, redirect) = row.ok_or(PortalError::SignIn("state"))?;
    let provider = get(&mut *conn, provider_id).await?;
    if !provider.enabled {
        return Err(PortalError::SignIn("provider_disabled"));
    }
    let enc: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT client_secret_enc FROM portal_identity_providers WHERE id = $1")
            .bind(provider.id)
            .fetch_one(&mut *conn)
            .await?;
    let secret = match enc {
        Some(enc) => Some(
            String::from_utf8(
                crypto
                    .decrypt(binding(provider.id), &enc)
                    .await
                    .map_err(|_| PortalError::SignIn("provider"))?
                    .to_vec(),
            )
            .map_err(|_| PortalError::SignIn("provider"))?,
        ),
        None => None,
    };
    let identity = sso
        .verify_code(
            &provider.issuer_url,
            &provider.client_id,
            secret,
            redirect_url(&redirect)?,
            code,
            &nonce,
            verifier,
        )
        .await
        .map_err(|_| PortalError::SignIn("provider"))?;
    let email = identity
        .email
        .as_deref()
        .map(|e| e.trim().to_lowercase())
        .filter(|e| e.contains('@'))
        .ok_or(PortalError::SignIn("no_email"))?;
    let domain = email
        .rsplit_once('@')
        .map(|(_, d)| d.to_owned())
        .unwrap_or_default();
    if !provider.email_domains.contains(&domain) {
        return Err(PortalError::SignIn("domain"));
    }
    let trusted = provider.trust_email || identity.email_verified == Some(true);
    let account = resolve(
        &mut *conn,
        ctx,
        &provider,
        &identity.subject,
        &email,
        trusted,
    )
    .await?;
    sqlx::query(
        "INSERT INTO portal_identities (id, account_id, provider_id, subject, last_login_at) \
         VALUES ($1, $2, $3, $4, now()) \
         ON CONFLICT (provider_id, subject) DO UPDATE SET last_login_at = now()",
    )
    .bind(huxley_core::new_id())
    .bind(account)
    .bind(provider.id)
    .bind(&identity.subject)
    .execute(&mut *conn)
    .await?;
    // Their organization's sign-in stands in for two-factor sign-in here.
    let session = start_session(&mut *conn, account, ip, user_agent, false).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.signed_in")
            .org(session.principal.org_id)
            .target("portal_account", account)
            .details(json!({ "method": "sso", "provider": provider.id })),
    )
    .await?;
    Ok(session)
}

/// The portal account for an identity: already linked, matched by a
/// trusted email under the provider's company, or created for a contact
/// with that address when the provider provisions.
async fn resolve(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &Provider,
    subject: &str,
    email: &str,
    trusted: bool,
) -> PortalResult<Uuid> {
    let linked: Option<Uuid> = sqlx::query_scalar(
        "SELECT a.id FROM portal_identities i JOIN portal_accounts a ON a.id = i.account_id \
           JOIN crm_contacts c ON c.id = a.contact_id \
          WHERE i.provider_id = $1 AND i.subject = $2 AND a.status IN ('invited', 'active') \
            AND c.archived_at IS NULL",
    )
    .bind(provider.id)
    .bind(subject)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(account) = linked {
        return Ok(account);
    }
    if !trusted {
        return Err(PortalError::SignIn("unverified_email"));
    }
    let under = "(SELECT path FROM organizations WHERE id = $2)";
    let existing: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT a.id FROM portal_accounts a JOIN organizations o ON o.id = a.org_id \
           JOIN crm_contacts c ON c.id = a.contact_id \
          WHERE lower(a.email) = $1 AND a.status IN ('invited', 'active') AND c.archived_at IS NULL \
            AND o.path <@ {under}"
    )))
    .bind(email)
    .bind(provider.org_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(account) = existing {
        return Ok(account);
    }
    if !provider.provision {
        return Err(PortalError::SignIn("no_account"));
    }
    let contacts: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT c.id FROM crm_contacts c JOIN organizations o ON o.id = c.org_id \
          WHERE lower(c.email) = $1 AND c.archived_at IS NULL AND o.path <@ {under} \
            AND NOT EXISTS (SELECT 1 FROM portal_accounts a WHERE a.contact_id = c.id) LIMIT 2"
    )))
    .bind(email)
    .bind(provider.org_id)
    .fetch_all(&mut *conn)
    .await?;
    match contacts.as_slice() {
        [contact] => Ok(
            crate::accounts::invite(conn, ctx, *contact, provider.default_role)
                .await?
                .id,
        ),
        _ => Err(PortalError::SignIn("no_account")),
    }
}
