//! The sign-in flow: start (redirect to the provider) and callback.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use huxley_auth::{AuthService, LoginSuccess, User, UserStatus};
use huxley_core::{Actor, RequestContext};
use huxley_crypto::Crypto;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{OrgScope, begin_scoped};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse as _, reqwest,
};
use serde_json::{Value, json};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::providers::{self, MfaTrust, Provider, secret_binding};
use crate::{SsoError, SsoResult};

/// How long the user has at the provider before the sign-in expires.
const STATE_TTL: Duration = Duration::minutes(10);
/// Discovery documents (endpoints and signing keys) are refetched this often.
const METADATA_TTL: StdDuration = StdDuration::from_secs(600);
const STATE_PREFIX: &str = "hxo_";

/// Redirect the browser to `authorization_url`, and bind the sign-in to the
/// browser with `state` (e.g. in a cookie) to check at the callback.
#[derive(Debug)]
pub struct StartedSignIn {
    pub authorization_url: String,
    pub state: String,
}

/// A completed SSO sign-in.
#[derive(Debug)]
pub struct SignIn {
    pub session: LoginSuccess,
    /// Where the user was headed when they started.
    pub next: Option<String>,
    /// The session counts as multi-factor for the MFA policy.
    pub mfa: bool,
}

/// Runs SSO sign-ins. Cheap to clone.
#[derive(Debug, Clone)]
pub struct SsoService {
    auth: AuthService,
    crypto: Crypto,
    http: reqwest::Client,
    metadata: Arc<Mutex<HashMap<String, (Instant, CoreProviderMetadata)>>>,
}

/// A sign-in redirect and what to keep until its callback.
#[derive(Debug, Clone)]
pub struct Authorization {
    pub url: String,
    /// The state token (for a cookie binding the sign-in to the browser).
    pub state: String,
    pub state_hash: Vec<u8>,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// Who a provider says signed in, after the ID token was verified.
#[derive(Debug, Clone)]
pub struct VerifiedIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub amr: Vec<String>,
}

/// What the ID token told us, after verification.
struct Claims {
    subject: String,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    amr: Vec<String>,
    /// The whole verified payload, for provider-specific claims.
    raw: Value,
}

fn provider_error(e: impl std::fmt::Display) -> SsoError {
    SsoError::Provider(e.to_string())
}

impl SsoService {
    pub fn new(auth: AuthService, crypto: Crypto) -> SsoResult<Self> {
        // Never follow redirects from the provider (SSRF guidance in the
        // openidconnect docs); bound every call.
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(StdDuration::from_secs(10))
            .build()
            .map_err(provider_error)?;
        Ok(Self {
            auth,
            crypto,
            http,
            metadata: Arc::default(),
        })
    }

    fn redirect_url(&self) -> SsoResult<RedirectUrl> {
        let base = self.auth.public_url().ok_or(SsoError::NotConfigured)?;
        RedirectUrl::new(format!(
            "{}/login/oidc/callback",
            base.trim_end_matches('/')
        ))
        .map_err(|_| SsoError::NotConfigured)
    }

    async fn metadata(&self, issuer: &str) -> SsoResult<CoreProviderMetadata> {
        if let Some((at, m)) = self.metadata.lock().expect("cache lock").get(issuer)
            && at.elapsed() < METADATA_TTL
        {
            return Ok(m.clone());
        }
        let url = IssuerUrl::new(issuer.to_owned()).map_err(provider_error)?;
        let metadata = CoreProviderMetadata::discover_async(url, &self.http)
            .await
            .map_err(provider_error)?;
        self.metadata
            .lock()
            .expect("cache lock")
            .insert(issuer.to_owned(), (Instant::now(), metadata.clone()));
        Ok(metadata)
    }

    async fn client_secret(&self, provider: &Provider) -> SsoResult<Option<ClientSecret>> {
        let mut tx = begin_scoped(self.auth.db(), &OrgScope::System).await?;
        let (enc,): (Option<Vec<u8>>,) =
            sqlx::query_as("SELECT client_secret_enc FROM oidc_providers WHERE id = $1")
                .bind(provider.id)
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;
        let Some(enc) = enc else { return Ok(None) };
        let secret = self
            .crypto
            .decrypt(secret_binding(provider.id), &enc)
            .await?;
        Ok(Some(ClientSecret::new(
            String::from_utf8(secret.to_vec()).map_err(provider_error)?,
        )))
    }

    /// The enabled providers, for sign-in buttons.
    pub async fn buttons(&self) -> SsoResult<Vec<providers::ProviderButton>> {
        let mut tx = begin_scoped(self.auth.db(), &OrgScope::System).await?;
        let buttons = providers::buttons(&mut tx).await?;
        tx.commit().await?;
        Ok(buttons)
    }

    /// Starts a sign-in with the provider `key`. `next` is where to send the
    /// user afterwards (the caller validates it).
    pub async fn start(&self, key: &str, next: Option<&str>) -> SsoResult<StartedSignIn> {
        let redirect = self.redirect_url()?;
        let mut tx = begin_scoped(self.auth.db(), &OrgScope::System).await?;
        let provider = providers::get_enabled_by_key(&mut tx, key).await?;
        tx.commit().await?;
        let auth = self
            .authorize(
                &provider.issuer_url,
                &provider.client_id,
                &provider.scopes,
                redirect,
                STATE_PREFIX,
            )
            .await?;

        let mut tx = begin_scoped(self.auth.db(), &OrgScope::System).await?;
        sqlx::query("DELETE FROM oidc_login_states WHERE expires_at < now() - interval '1 hour'")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO oidc_login_states (id, token_hash, provider_id, nonce, pkce_verifier, next, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(huxley_core::new_id())
        .bind(&auth.state_hash)
        .bind(provider.id)
        .bind(&auth.nonce)
        .bind(&auth.pkce_verifier)
        .bind(next)
        .bind(OffsetDateTime::now_utc() + STATE_TTL)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(StartedSignIn {
            authorization_url: auth.url,
            state: auth.state,
        })
    }

    /// Builds a sign-in redirect to the provider at `issuer` (discovered and
    /// cached): a state token with `state_prefix`, a nonce and a PKCE
    /// verifier for the caller to keep until the callback.
    pub async fn authorize(
        &self,
        issuer: &str,
        client_id: &str,
        scopes: &[String],
        redirect: RedirectUrl,
        state_prefix: &str,
    ) -> SsoResult<Authorization> {
        let metadata = self.metadata(issuer).await?;
        let client =
            CoreClient::from_provider_metadata(metadata, ClientId::new(client_id.to_owned()), None)
                .set_redirect_uri(redirect);
        let state = huxley_auth::token::generate(state_prefix);
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let state_secret = state.secret.clone();
        let mut request = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                move || CsrfToken::new(state_secret),
                Nonce::new_random,
            )
            .set_pkce_challenge(pkce_challenge);
        for scope in scopes.iter().filter(|s| *s != "openid") {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (url, _, nonce) = request.url();
        Ok(Authorization {
            url: url.to_string(),
            state: state.secret,
            state_hash: state.hash,
            nonce: nonce.secret().clone(),
            pkce_verifier: pkce_verifier.secret().clone(),
        })
    }

    /// Completes a sign-in from the provider's callback. Failures are audited.
    pub async fn callback(
        &self,
        ctx: &RequestContext,
        state: &str,
        code: &str,
    ) -> SsoResult<SignIn> {
        let result = self.complete(ctx, state, code).await;
        if let Err((provider, error)) = &result {
            self.audit_failure(ctx, *provider, error).await;
        }
        result.map_err(|(_, e)| e)
    }

    async fn audit_failure(&self, ctx: &RequestContext, provider: Option<Uuid>, error: &SsoError) {
        tracing::warn!(reason = error.reason(), %error, "sso sign-in failed");
        let record = async {
            let mut tx = begin_scoped(self.auth.db(), &OrgScope::System).await?;
            let mut rec = AuditRecord::new("auth.login_failed")
                .details(json!({ "method": "sso", "reason": error.reason() }));
            if let Some(p) = provider {
                rec = rec.target("oidc_provider", p);
            }
            audit::record(&mut tx, &ctx.clone().with_actor(Actor::Anonymous), rec).await?;
            tx.commit().await?;
            Ok::<_, SsoError>(())
        };
        if let Err(e) = record.await {
            tracing::error!(%e, "failed to audit an sso sign-in failure");
        }
    }

    async fn complete(
        &self,
        ctx: &RequestContext,
        state: &str,
        code: &str,
    ) -> Result<SignIn, (Option<Uuid>, SsoError)> {
        let fail = |p: Option<Uuid>| move |e: SsoError| (p, e);
        if !huxley_auth::token::looks_valid(state, STATE_PREFIX) {
            return Err((None, SsoError::InvalidState));
        }
        let redirect = self.redirect_url().map_err(fail(None))?;

        let mut tx = begin_scoped(self.auth.db(), &OrgScope::System)
            .await
            .map_err(|e| (None, e.into()))?;
        let row: Option<(Uuid, String, String, Option<String>)> = sqlx::query_as(
            "DELETE FROM oidc_login_states WHERE token_hash = $1 AND expires_at > now() \
             RETURNING provider_id, nonce, pkce_verifier, next",
        )
        .bind(huxley_auth::token::hash(state))
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| (None, e.into()))?;
        tx.commit().await.map_err(|e| (None, e.into()))?;
        let (provider_id, nonce, verifier, next) = row.ok_or((None, SsoError::InvalidState))?;
        let p = Some(provider_id);

        let mut tx = begin_scoped(self.auth.db(), &OrgScope::System)
            .await
            .map_err(|e| (p, e.into()))?;
        let provider = providers::get(&mut tx, provider_id)
            .await
            .map_err(fail(p))?;
        tx.commit().await.map_err(|e| (p, e.into()))?;
        if !provider.enabled {
            return Err((p, SsoError::ProviderNotFound));
        }

        let claims = self
            .verify(&provider, redirect, code, &nonce, verifier)
            .await
            .map_err(fail(p))?;
        let email = claims
            .email
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .ok_or((p, SsoError::MissingEmail))?
            .to_owned();
        let domain = email
            .rsplit_once('@')
            .map(|(_, d)| d.to_ascii_lowercase())
            .unwrap_or_default();
        if !provider.allowed_domains.contains(&domain) {
            return Err((p, SsoError::DomainNotAllowed));
        }
        let email_trusted = provider.trust_email || claims.email_verified == Some(true);

        let mfa = match provider.mfa_trust {
            MfaTrust::Always => true,
            MfaTrust::Never => false,
            MfaTrust::Amr => claims.amr.iter().any(|m| m == "mfa"),
        };
        let role_values = provider
            .role_claim
            .as_deref()
            .map(|claim| claim_values(&claims.raw, claim));

        let mut tx = begin_scoped(self.auth.db(), &OrgScope::System)
            .await
            .map_err(|e| (p, e.into()))?;
        let user = self
            .resolve_user(&mut tx, ctx, &provider, &claims, &email, email_trusted)
            .await
            .map_err(fail(p))?;
        if user.status == UserStatus::Disabled {
            return Err((p, SsoError::Disabled));
        }
        if let Some(values) = role_values {
            sync_roles(&mut tx, ctx, &provider, user.id, &values)
                .await
                .map_err(fail(p))?;
        }
        sqlx::query(
            "UPDATE user_identities SET last_login_at = now(), email = $3 \
              WHERE provider_id = $1 AND subject = $2",
        )
        .bind(provider.id)
        .bind(&claims.subject)
        .bind(&email)
        .execute(&mut *tx)
        .await
        .map_err(|e| (p, e.into()))?;
        let method = format!("sso:{}", provider.key);
        let session = self
            .auth
            .start_external_session(&mut tx, ctx, user.id, &method, mfa)
            .await
            .map_err(|e| (p, e.into()))?;
        tx.commit().await.map_err(|e| (p, e.into()))?;
        Ok(SignIn { session, next, mfa })
    }

    /// Exchanges the code and verifies the ID token (signature, issuer,
    /// audience, expiry, nonce).
    async fn verify(
        &self,
        provider: &Provider,
        redirect: RedirectUrl,
        code: &str,
        nonce: &str,
        verifier: String,
    ) -> SsoResult<Claims> {
        let secret = self.client_secret(provider).await?;
        self.exchange(
            &provider.issuer_url,
            &provider.client_id,
            secret,
            redirect,
            code,
            nonce,
            verifier,
        )
        .await
    }

    /// Exchanges an authorization code with the provider at `issuer` and
    /// verifies the ID token (signature, issuer, audience, expiry, nonce).
    #[allow(clippy::too_many_arguments)]
    pub async fn verify_code(
        &self,
        issuer: &str,
        client_id: &str,
        client_secret: Option<String>,
        redirect: RedirectUrl,
        code: &str,
        nonce: &str,
        verifier: String,
    ) -> SsoResult<VerifiedIdentity> {
        let claims = self
            .exchange(
                issuer,
                client_id,
                client_secret.map(ClientSecret::new),
                redirect,
                code,
                nonce,
                verifier,
            )
            .await?;
        Ok(VerifiedIdentity {
            subject: claims.subject,
            email: claims.email,
            email_verified: claims.email_verified,
            name: claims.name,
            amr: claims.amr,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn exchange(
        &self,
        issuer: &str,
        client_id: &str,
        secret: Option<ClientSecret>,
        redirect: RedirectUrl,
        code: &str,
        nonce: &str,
        verifier: String,
    ) -> SsoResult<Claims> {
        let metadata = self.metadata(issuer).await?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(client_id.to_owned()),
            secret,
        )
        .set_redirect_uri(redirect);
        let token = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(provider_error)?
            .set_pkce_verifier(PkceCodeVerifier::new(verifier))
            .request_async(&self.http)
            .await
            .map_err(provider_error)?;
        let id_token = token
            .id_token()
            .ok_or_else(|| SsoError::Provider("no ID token in the response".into()))?;
        let id_claims = id_token
            .claims(&client.id_token_verifier(), &Nonce::new(nonce.to_owned()))
            .map_err(provider_error)?;
        // The signature covers the whole payload, so other claims can be read
        // from the same (now verified) token.
        let raw = id_token
            .to_string()
            .split('.')
            .nth(1)
            .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .unwrap_or(Value::Null);
        Ok(Claims {
            subject: id_claims.subject().to_string(),
            email: id_claims.email().map(|e| e.to_string()).or_else(|| {
                // Entra puts the sign-in address in preferred_username when
                // the user has no mail attribute.
                raw.get("preferred_username")
                    .and_then(Value::as_str)
                    .filter(|s| s.contains('@'))
                    .map(str::to_owned)
            }),
            email_verified: id_claims.email_verified(),
            name: id_claims
                .name()
                .and_then(|n| n.get(None))
                .map(|n| n.to_string()),
            amr: id_claims
                .auth_method_refs()
                .map(|a| a.iter().map(|m| m.to_string()).collect())
                .unwrap_or_default(),
            raw,
        })
    }

    /// The user for this identity: already linked, linked now by verified
    /// email, or created (JIT).
    async fn resolve_user(
        &self,
        conn: &mut PgConnection,
        ctx: &RequestContext,
        provider: &Provider,
        claims: &Claims,
        email: &str,
        verified: bool,
    ) -> SsoResult<User> {
        let linked: Option<(Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM user_identities WHERE provider_id = $1 AND subject = $2",
        )
        .bind(provider.id)
        .bind(&claims.subject)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((user_id,)) = linked {
            return huxley_auth::find_user(conn, user_id)
                .await?
                .ok_or(SsoError::NoAccount);
        }
        if !verified {
            return Err(SsoError::UnverifiedEmail);
        }
        let (user, created) = match huxley_auth::find_user_by_email(conn, email).await? {
            Some(user) => (user, false),
            None if provider.jit_provisioning => {
                let name = claims.name.as_deref().unwrap_or(email);
                let user = huxley_auth::create_external_user(conn, ctx, email, name, &provider.key)
                    .await?;
                (user, true)
            }
            None => return Err(SsoError::NoAccount),
        };
        sqlx::query(
            "INSERT INTO user_identities (id, user_id, provider_id, subject, email) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(huxley_core::new_id())
        .bind(user.id)
        .bind(provider.id)
        .bind(&claims.subject)
        .bind(email)
        .execute(&mut *conn)
        .await?;
        audit::record(
            conn,
            ctx,
            AuditRecord::new("auth.sso_identity_linked")
                .target("user", user.id)
                .details(json!({ "provider": provider.key, "created": created })),
        )
        .await?;
        Ok(user)
    }
}

/// The string values of a claim (a string or an array of strings).
fn claim_values(raw: &Value, claim: &str) -> HashSet<String> {
    match raw.get(claim) {
        Some(Value::String(s)) => std::iter::once(s.clone()).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => HashSet::new(),
    }
}

/// Makes the provider-managed assignments of `user_id` match the mappings for
/// `values`. Hand-made assignments are left alone, including ones that
/// duplicate a mapping.
async fn sync_roles(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    provider: &Provider,
    user_id: Uuid,
    values: &HashSet<String>,
) -> SsoResult<()> {
    let wanted: Vec<(Uuid, Uuid, bool)> = sqlx::query_as(
        "SELECT DISTINCT ON (m.role_id, m.org_id) m.role_id, m.org_id, m.inherit \
           FROM oidc_role_mappings m JOIN organizations o ON o.id = m.org_id AND o.status = 'active' \
          WHERE m.provider_id = $1 AND m.claim_value = ANY($2) \
          ORDER BY m.role_id, m.org_id, m.inherit DESC",
    )
    .bind(provider.id)
    .bind(values.iter().cloned().collect::<Vec<_>>())
    .fetch_all(&mut *conn)
    .await?;
    let removed = sqlx::query(
        "DELETE FROM role_assignments \
          WHERE user_id = $1 AND oidc_provider_id = $2 \
            AND NOT ((role_id, org_id) IN (SELECT * FROM unnest($3::uuid[], $4::uuid[])))",
    )
    .bind(user_id)
    .bind(provider.id)
    .bind(wanted.iter().map(|w| w.0).collect::<Vec<_>>())
    .bind(wanted.iter().map(|w| w.1).collect::<Vec<_>>())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let mut added = 0;
    for (role_id, org_id, inherit) in &wanted {
        added += sqlx::query(
            "INSERT INTO role_assignments (id, user_id, role_id, org_id, inherit, oidc_provider_id) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT ON CONSTRAINT role_assignments_unique DO NOTHING",
        )
        .bind(huxley_core::new_id())
        .bind(user_id)
        .bind(role_id)
        .bind(org_id)
        .bind(inherit)
        .bind(provider.id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    }
    if added > 0 || removed > 0 {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("auth.sso_roles_synced")
                .target("user", user_id)
                .details(json!({ "provider": provider.key, "added": added, "removed": removed })),
        )
        .await?;
    }
    Ok(())
}
