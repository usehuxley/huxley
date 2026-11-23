//! API tokens (personal access tokens and service account tokens) and
//! service accounts (design doc §11.1, §11.2).
//!
//! A token is `hxp_` + 256 random bits; only its SHA-256 is stored. Its
//! `scopes` narrow what the holder may do (`*` = everything the holder's roles
//! allow); they're applied by the authorization layer on top of the holder's
//! roles, so a token never grants more than its owner has. People's tokens
//! must expire within a year; service account tokens may be non-expiring.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde_json::json;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    AuthError, AuthResult, AuthService, Authenticated, User, UserKind, find_user,
    normalize_display_name, token,
};

/// Prefix for API tokens (helps secret scanners recognise leaked ones).
pub const PREFIX: &str = "hxp_";
/// Longest lifetime of a person's token.
pub const MAX_HUMAN_LIFETIME: Duration = Duration::days(366);
/// `last_used_at` is updated at most this often per token.
const TOUCH_INTERVAL: Duration = Duration::seconds(60);

/// What an authenticated token allows, beyond its owner's identity.
#[derive(Debug, Clone)]
pub struct TokenGrant {
    pub id: Uuid,
    /// Permission keys, or `["*"]` for everything the owner may do.
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub hint: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
    pub last_used_ip: Option<String>,
}

/// A new token. `secret` is shown to the user once and never stored.
#[derive(Debug)]
pub struct NewApiToken {
    pub token: ApiToken,
    pub secret: String,
}

const SELECT_TOKENS: &str = "SELECT id, user_id, name, hint, scopes, expires_at, created_at, \
    last_used_at, host(last_used_ip) AS last_used_ip FROM api_tokens";

/// Creates a token for `user`. `scopes` must already be validated permission
/// keys (or `*`). Runs in the caller's transaction.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: &User,
    name: &str,
    scopes: &[String],
    expires_at: Option<OffsetDateTime>,
) -> AuthResult<NewApiToken> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(AuthError::InvalidTokenName);
    }
    if scopes.is_empty() {
        return Err(AuthError::InvalidTokenName);
    }
    let now = OffsetDateTime::now_utc();
    match (user.kind, expires_at) {
        (_, Some(at)) if at <= now => return Err(AuthError::InvalidTokenExpiry("in the past")),
        (UserKind::Human, None) => {
            return Err(AuthError::InvalidTokenExpiry("personal tokens must expire"));
        }
        (UserKind::Human, Some(at)) if at > now + MAX_HUMAN_LIFETIME => {
            return Err(AuthError::InvalidTokenExpiry(
                "personal tokens last at most a year",
            ));
        }
        _ => {}
    }
    let generated = token::generate(PREFIX);
    let hint: String = generated.secret.chars().take(PREFIX.len() + 4).collect();
    let id = huxley_core::new_id();
    let created: ApiToken = sqlx::query_as(
        "INSERT INTO api_tokens (id, user_id, name, token_hash, hint, scopes, expires_at, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, user_id, name, hint, scopes, expires_at, created_at, last_used_at, \
                   host(last_used_ip) AS last_used_ip",
    )
    .bind(id)
    .bind(user.id)
    .bind(name)
    .bind(&generated.hash)
    .bind(&hint)
    .bind(scopes)
    .bind(expires_at)
    .bind(ctx.actor.id())
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("auth.api_token_created")
            .target("user", user.id)
            .details(json!({
                "token": id,
                "name": name,
                "scopes": scopes,
                "expiresAt": expires_at.map(OffsetDateTime::unix_timestamp),
            })),
    )
    .await?;
    Ok(NewApiToken {
        token: created,
        secret: generated.secret,
    })
}

/// The user's tokens that haven't been revoked, newest first (expired ones
/// included, so people can see and clean them up).
pub async fn list(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<Vec<ApiToken>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_TOKENS} WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC"
    )))
    .bind(user_id)
    .fetch_all(conn)
    .await?)
}

/// Revokes one of `user_id`'s tokens.
pub async fn revoke(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    token_id: Uuid,
) -> AuthResult<()> {
    let revoked = sqlx::query(
        "UPDATE api_tokens SET revoked_at = now() \
          WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(token_id)
    .bind(user_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if revoked == 0 {
        return Err(AuthError::TokenNotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("auth.api_token_revoked")
            .target("user", user_id)
            .details(json!({ "token": token_id })),
    )
    .await?;
    Ok(())
}

/// Creates a service account. Runs in the caller's transaction.
pub async fn create_service_account(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    name: &str,
    description: &str,
) -> AuthResult<User> {
    let name = normalize_display_name(name)?;
    let description = description.trim();
    if description.chars().count() > 500 {
        return Err(AuthError::InvalidDisplayName);
    }
    let id = huxley_core::new_id();
    // RFC 2606 reserves .invalid: this address can never receive mail or
    // collide with a real one.
    let email = format!("sa-{}@service.invalid", id.simple());
    sqlx::query(
        "INSERT INTO users (id, email, display_name, kind, description) \
         VALUES ($1, $2, $3, 'service', $4)",
    )
    .bind(id)
    .bind(&email)
    .bind(&name)
    .bind(description)
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("user.created")
            .target("user", id)
            .details(json!({ "after": { "displayName": name, "kind": "service" } })),
    )
    .await?;
    find_user(conn, id).await?.ok_or(AuthError::UserNotFound)
}

/// A service account's description.
pub async fn description(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<String> {
    let row: Option<(String,)> = sqlx::query_as("SELECT description FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(|(d,)| d).unwrap_or_default())
}

impl AuthService {
    /// Resolves an API token to its owner. `None` for unknown, revoked, or
    /// expired tokens and disabled owners. Records when (and from where) it
    /// was last used, at most once a minute.
    pub async fn authenticate_token(
        &self,
        secret: &str,
        ip: Option<std::net::IpAddr>,
    ) -> AuthResult<Option<Authenticated>> {
        if !token::looks_valid(secret, PREFIX) {
            return Ok(None);
        }
        let row: Option<(Uuid, Uuid, Vec<String>, Option<OffsetDateTime>)> = sqlx::query_as(
            "SELECT t.id, t.user_id, t.scopes, t.last_used_at \
               FROM api_tokens t JOIN users u ON u.id = t.user_id \
              WHERE t.token_hash = $1 AND t.revoked_at IS NULL \
                AND (t.expires_at IS NULL OR t.expires_at > now()) \
                AND u.status = 'active'",
        )
        .bind(token::hash(secret))
        .fetch_optional(&self.db)
        .await?;
        let Some((token_id, user_id, scopes, last_used)) = row else {
            return Ok(None);
        };
        let now = OffsetDateTime::now_utc();
        if last_used.is_none_or(|at| now - at > TOUCH_INTERVAL) {
            sqlx::query(
                "UPDATE api_tokens SET last_used_at = $2, last_used_ip = $3::inet WHERE id = $1",
            )
            .bind(token_id)
            .bind(now)
            .bind(ip.map(|ip| ip.to_string()))
            .execute(&self.db)
            .await?;
        }
        let mut conn = self.db.acquire().await?;
        Ok(find_user(&mut conn, user_id)
            .await?
            .map(|user| Authenticated {
                session_id: token_id,
                user,
                session_mfa: false,
                token: Some(TokenGrant {
                    id: token_id,
                    scopes,
                }),
            }))
    }
}

/// Service accounts, ordered by name.
pub async fn list_service_accounts(conn: &mut PgConnection) -> AuthResult<Vec<User>> {
    Ok(sqlx::query_as(crate::select_user!(
        "WHERE kind = 'service' ORDER BY lower(display_name), id"
    ))
    .fetch_all(conn)
    .await?)
}

/// Changes a service account's description.
pub async fn set_description(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    description: &str,
) -> AuthResult<()> {
    let description = description.trim();
    if description.chars().count() > 500 {
        return Err(AuthError::InvalidDisplayName);
    }
    sqlx::query("UPDATE users SET description = $2 WHERE id = $1 AND kind = 'service'")
        .bind(user_id)
        .bind(description)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("user.updated")
            .target("user", user_id)
            .details(json!({ "after": { "description": description } })),
    )
    .await?;
    Ok(())
}
