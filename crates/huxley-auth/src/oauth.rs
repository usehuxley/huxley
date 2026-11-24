//! OAuth 2.1 authorization server for MCP clients (ADR-032).
//!
//! Huxley issues tokens for its own MCP endpoint only. Clients register
//! themselves (RFC 7591) and are all public clients: no client secrets, with
//! PKCE (S256 only) protecting the authorization code. A person approves a
//! client on the consent page; the approval is a *grant*, carrying scopes that
//! narrow the person's roles exactly as an API token's do.
//!
//! - Access tokens (`hxo_`) last an hour and are accepted only by `/mcp`.
//! - Refresh tokens (`hxr_`) last 30 days and rotate on every use. Presenting
//!   a replaced refresh token again means it was copied: the grant is revoked.
//! - Codes (`hxa_`) last five minutes and are single use. Exchanging one twice
//!   revokes the grant the first exchange made.
//!
//! Every secret is stored as its SHA-256, like sessions and API tokens.
//!
//! **Commit on [`OAuthError::InvalidGrant`].** Those failures write nothing
//! except the revocations that a reused code or refresh token triggers, and
//! those must stick even though the request fails.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::api_tokens::TokenGrant;
use crate::{AuthError, AuthService, Authenticated, find_user, token};

/// Access tokens.
pub const ACCESS_PREFIX: &str = "hxo_";
/// Refresh tokens.
pub const REFRESH_PREFIX: &str = "hxr_";
/// Authorization codes.
pub const CODE_PREFIX: &str = "hxa_";
/// Client ids (not secret: they identify a registration).
pub const CLIENT_PREFIX: &str = "hxc_";

pub const ACCESS_LIFETIME: Duration = Duration::hours(1);
pub const REFRESH_LIFETIME: Duration = Duration::days(30);
pub const CODE_LIFETIME: Duration = Duration::minutes(5);
/// Registrations that never led to a grant are deleted after this.
const UNUSED_CLIENT_LIFETIME: Duration = Duration::days(1);
/// `last_used_at` is updated at most this often per grant.
const TOUCH_INTERVAL: Duration = Duration::seconds(60);

/// OAuth errors, named by their RFC 6749 / 7591 codes.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    InvalidClientMetadata(String),
    #[error("{0}")]
    InvalidRedirectUri(String),
    #[error("unknown client")]
    InvalidClient,
    #[error("{0}")]
    InvalidGrant(&'static str),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl OAuthError {
    /// The `error` code of the response body.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidClientMetadata(_) => "invalid_client_metadata",
            Self::InvalidRedirectUri(_) => "invalid_redirect_uri",
            Self::InvalidClient => "invalid_client",
            Self::InvalidGrant(_) => "invalid_grant",
            Self::Auth(_) | Self::Db(_) | Self::Sqlx(_) => "server_error",
        }
    }

    /// Whether this is our fault (logged, and shown without detail).
    pub fn is_internal(&self) -> bool {
        matches!(self, Self::Auth(_) | Self::Db(_) | Self::Sqlx(_))
    }
}

pub type OAuthResult<T> = Result<T, OAuthError>;

/// A registered client.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Client {
    pub id: Uuid,
    pub client_id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: OffsetDateTime,
}

/// Schemes a redirect URI must never use.
const FORBIDDEN_SCHEMES: &[&str] = &["javascript", "data", "file", "vbscript", "blob", "about"];

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// Whether `uri` may be registered: `https`, `http` on a loopback address
/// (native apps, RFC 8252), or an app's own scheme (`cursor://…`), with no
/// fragment.
pub fn check_redirect_uri(uri: &str) -> Result<(), String> {
    let parsed = url::Url::parse(uri).map_err(|_| format!("{uri:?} isn't an absolute URL"))?;
    if parsed.fragment().is_some() {
        return Err(format!("{uri:?} has a fragment"));
    }
    match parsed.scheme() {
        "https" if parsed.host_str().is_some() => Ok(()),
        "http" if parsed.host_str().is_some_and(is_loopback_host) => Ok(()),
        "http" | "https" => Err(format!(
            "{uri:?}: use https, or http only on localhost, 127.0.0.1 or [::1]"
        )),
        scheme if FORBIDDEN_SCHEMES.contains(&scheme) => {
            Err(format!("{uri:?}: the {scheme} scheme isn't allowed"))
        }
        _ => Ok(()),
    }
}

/// Whether the redirect URI in a request is one the client registered. Exact
/// match, except that loopback URIs may use any port (RFC 8252 §7.3: native
/// apps pick a free port each time).
pub fn redirect_uri_registered(client: &Client, presented: &str) -> bool {
    client.redirect_uris.iter().any(|registered| {
        if registered == presented {
            return true;
        }
        let (Ok(mut a), Ok(mut b)) = (url::Url::parse(registered), url::Url::parse(presented))
        else {
            return false;
        };
        if a.scheme() != "http" || !a.host_str().is_some_and(is_loopback_host) {
            return false;
        }
        let _ = a.set_port(None);
        let _ = b.set_port(None);
        a == b
    })
}

/// PKCE (RFC 7636), S256 only: `challenge` is base64url(SHA-256(verifier)).
pub fn pkce_matches(challenge: &str, verifier: &str) -> bool {
    use base64::Engine as _;
    let shaped = (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'));
    if !shaped {
        return false;
    }
    let digest = Sha256::digest(verifier.as_bytes());
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    subtle::ConstantTimeEq::ct_eq(expected.as_bytes(), challenge.as_bytes()).into()
}

/// Whether a `code_challenge` has the shape S256 produces.
pub fn challenge_shaped(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Registers a client (RFC 7591). Also clears out expired codes and tokens,
/// and registrations nobody ever approved.
pub async fn register_client(
    conn: &mut PgConnection,
    name: &str,
    redirect_uris: &[String],
    ip: Option<std::net::IpAddr>,
) -> OAuthResult<Client> {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(OAuthError::InvalidClientMetadata(
            "client_name must be 1 to 100 characters".into(),
        ));
    }
    if redirect_uris.is_empty() || redirect_uris.len() > 10 {
        return Err(OAuthError::InvalidRedirectUri(
            "register 1 to 10 redirect_uris".into(),
        ));
    }
    for uri in redirect_uris {
        if uri.len() > 2000 {
            return Err(OAuthError::InvalidRedirectUri(
                "a redirect URI is too long".into(),
            ));
        }
        check_redirect_uri(uri).map_err(OAuthError::InvalidRedirectUri)?;
    }
    prune(conn).await?;
    let client_id = token::generate(CLIENT_PREFIX).secret;
    Ok(sqlx::query_as(
        "INSERT INTO oauth_clients (id, client_id, name, redirect_uris, created_ip) \
         VALUES ($1, $2, $3, $4, $5::inet) \
         RETURNING id, client_id, name, redirect_uris, created_at",
    )
    .bind(huxley_core::new_id())
    .bind(&client_id)
    .bind(&name)
    .bind(redirect_uris)
    .bind(ip.map(|ip| ip.to_string()))
    .fetch_one(conn)
    .await?)
}

async fn prune(conn: &mut PgConnection) -> OAuthResult<()> {
    sqlx::query("DELETE FROM oauth_codes WHERE expires_at < now() - interval '1 day'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM oauth_access_tokens WHERE expires_at < now()")
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "DELETE FROM oauth_clients c WHERE c.created_at < now() - $1::interval \
            AND NOT EXISTS (SELECT 1 FROM oauth_grants g WHERE g.client_id = c.id) \
            AND NOT EXISTS (SELECT 1 FROM oauth_codes o WHERE o.client_id = c.id)",
    )
    .bind(format!(
        "{} seconds",
        UNUSED_CLIENT_LIFETIME.whole_seconds()
    ))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The client with this `client_id`.
pub async fn find_client(conn: &mut PgConnection, client_id: &str) -> OAuthResult<Option<Client>> {
    if !client_id.starts_with(CLIENT_PREFIX) || client_id.len() > 100 {
        return Ok(None);
    }
    Ok(sqlx::query_as(
        "SELECT id, client_id, name, redirect_uris, created_at FROM oauth_clients WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(conn)
    .await?)
}

/// What a person approved on the consent page. Everything in it has been
/// checked by the caller (the client's redirect URI, the challenge's shape,
/// the scopes and the resource).
#[derive(Debug)]
pub struct Approval<'a> {
    pub client: &'a Client,
    pub user_id: Uuid,
    pub redirect_uri: &'a str,
    pub code_challenge: &'a str,
    pub scopes: &'a [String],
    pub resource: &'a str,
}

/// Issues an authorization code for an approval.
pub async fn issue_code(conn: &mut PgConnection, approval: &Approval<'_>) -> OAuthResult<String> {
    let code = token::generate(CODE_PREFIX);
    sqlx::query(
        "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, code_challenge, \
                                  scopes, resource, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&code.hash)
    .bind(approval.client.id)
    .bind(approval.user_id)
    .bind(approval.redirect_uri)
    .bind(approval.code_challenge)
    .bind(approval.scopes)
    .bind(approval.resource)
    .bind(OffsetDateTime::now_utc() + CODE_LIFETIME)
    .execute(conn)
    .await?;
    Ok(code.secret)
}

/// Tokens for a client.
#[derive(Debug)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub scopes: Vec<String>,
}

/// The token request of the authorization-code grant.
#[derive(Debug)]
pub struct CodeExchange<'a> {
    pub client_id: &'a str,
    pub code: &'a str,
    pub redirect_uri: &'a str,
    pub code_verifier: &'a str,
    /// The `resource` parameter, if the client sent one.
    pub resource: Option<&'a str>,
}

#[derive(sqlx::FromRow)]
struct CodeRow {
    client_id: Uuid,
    client_name: String,
    public_client_id: String,
    user_id: Uuid,
    redirect_uri: String,
    code_challenge: String,
    scopes: Vec<String>,
    resource: String,
    expires_at: OffsetDateTime,
    grant_id: Option<Uuid>,
}

/// Exchanges an authorization code for tokens, creating the grant.
pub async fn exchange_code(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    request: &CodeExchange<'_>,
) -> OAuthResult<Tokens> {
    const BAD: OAuthError =
        OAuthError::InvalidGrant("the code is invalid, expired or already used");
    if !token::looks_valid(request.code, CODE_PREFIX) {
        return Err(BAD);
    }
    let row: Option<CodeRow> = sqlx::query_as(
        "SELECT o.client_id, c.name AS client_name, c.client_id AS public_client_id, o.user_id, \
                o.redirect_uri, o.code_challenge, o.scopes, o.resource, o.expires_at, o.grant_id \
           FROM oauth_codes o JOIN oauth_clients c ON c.id = o.client_id \
          WHERE o.code_hash = $1 FOR UPDATE OF o",
    )
    .bind(token::hash(request.code))
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else { return Err(BAD) };
    if let Some(grant_id) = row.grant_id {
        // A second exchange: whoever has the code isn't to be trusted, and
        // neither are the tokens the first exchange issued.
        revoke_grant_row(conn, ctx, grant_id, row.user_id, "auth.oauth_code_reused").await?;
        return Err(BAD);
    }
    if row.expires_at <= OffsetDateTime::now_utc() || row.public_client_id != request.client_id {
        return Err(BAD);
    }
    if row.redirect_uri != request.redirect_uri {
        return Err(OAuthError::InvalidGrant(
            "redirect_uri isn't the one the code was issued for",
        ));
    }
    if !pkce_matches(&row.code_challenge, request.code_verifier) {
        return Err(OAuthError::InvalidGrant(
            "code_verifier doesn't match the code_challenge",
        ));
    }
    if request.resource.is_some_and(|r| r != row.resource) {
        return Err(OAuthError::InvalidGrant(
            "resource isn't the one the code was issued for",
        ));
    }
    let Some(user) = find_user(conn, row.user_id).await? else {
        return Err(BAD);
    };
    if user.status != crate::UserStatus::Active {
        return Err(BAD);
    }
    let grant_id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO oauth_grants (id, client_id, user_id, scopes, resource) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(grant_id)
    .bind(row.client_id)
    .bind(row.user_id)
    .bind(&row.scopes)
    .bind(&row.resource)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE oauth_codes SET grant_id = $2 WHERE code_hash = $1")
        .bind(token::hash(request.code))
        .bind(grant_id)
        .execute(&mut *conn)
        .await?;
    let ctx = ctx
        .clone()
        .with_actor(huxley_core::Actor::User(row.user_id));
    audit::record(
        conn,
        &ctx,
        AuditRecord::new("auth.oauth_app_connected")
            .target("user", row.user_id)
            .details(json!({
                "grant": grant_id,
                "client": row.public_client_id,
                "clientName": row.client_name,
                "scopes": row.scopes,
            })),
    )
    .await?;
    crate::notify_security(
        conn,
        row.user_id,
        huxley_notify::kinds::OAUTH_APP_CONNECTED,
        &format!("{} was connected to your account", row.client_name),
    )
    .await?;
    issue_tokens(conn, grant_id, None, row.scopes).await
}

async fn issue_tokens(
    conn: &mut PgConnection,
    grant_id: Uuid,
    replaced_refresh: Option<&[u8]>,
    scopes: Vec<String>,
) -> OAuthResult<Tokens> {
    let now = OffsetDateTime::now_utc();
    let access = token::generate(ACCESS_PREFIX);
    let refresh = token::generate(REFRESH_PREFIX);
    sqlx::query(
        "INSERT INTO oauth_access_tokens (token_hash, grant_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(&access.hash)
    .bind(grant_id)
    .bind(now + ACCESS_LIFETIME)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE oauth_grants SET refresh_hash = $2, refresh_expires_at = $3, \
                previous_refresh_hash = $4 \
          WHERE id = $1",
    )
    .bind(grant_id)
    .bind(&refresh.hash)
    .bind(now + REFRESH_LIFETIME)
    .bind(replaced_refresh)
    .execute(&mut *conn)
    .await?;
    Ok(Tokens {
        access_token: access.secret,
        refresh_token: refresh.secret,
        expires_in: ACCESS_LIFETIME.whole_seconds(),
        scopes,
    })
}

/// The refresh-token grant: new tokens, and the refresh token rotates.
pub async fn refresh(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    client_id: &str,
    refresh_token: &str,
) -> OAuthResult<Tokens> {
    const BAD: OAuthError = OAuthError::InvalidGrant("the refresh token is invalid or expired");
    if !token::looks_valid(refresh_token, REFRESH_PREFIX) {
        return Err(BAD);
    }
    let hash = token::hash(refresh_token);
    let current: Option<RefreshRow> = sqlx::query_as(
        "SELECT g.id AS grant_id, c.client_id AS owner, g.scopes, \
                g.refresh_expires_at AS expires_at, g.revoked_at \
               FROM oauth_grants g JOIN oauth_clients c ON c.id = g.client_id \
               JOIN users u ON u.id = g.user_id \
              WHERE g.refresh_hash = $1 AND u.status = 'active' FOR UPDATE OF g",
    )
    .bind(&hash)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(RefreshRow {
        grant_id,
        owner,
        scopes,
        expires_at,
        revoked_at,
    }) = current
    {
        let live =
            revoked_at.is_none() && expires_at.is_some_and(|at| at > OffsetDateTime::now_utc());
        if !live || owner != client_id {
            return Err(BAD);
        }
        // The access tokens issued before stay valid until they expire.
        return issue_tokens(conn, grant_id, Some(&hash), scopes).await;
    }
    let replaced: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, user_id FROM oauth_grants \
          WHERE previous_refresh_hash = $1 AND revoked_at IS NULL FOR UPDATE",
    )
    .bind(&hash)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((grant_id, user_id)) = replaced {
        revoke_grant_row(conn, ctx, grant_id, user_id, "auth.oauth_refresh_reused").await?;
    }
    Err(BAD)
}

/// Revokes a grant (its tokens stop working at once) and audits why.
async fn revoke_grant_row(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    grant_id: Uuid,
    user_id: Uuid,
    action: &str,
) -> OAuthResult<bool> {
    let revoked = sqlx::query(
        "UPDATE oauth_grants SET revoked_at = now(), refresh_hash = NULL, \
                previous_refresh_hash = NULL \
          WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(grant_id)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    sqlx::query("DELETE FROM oauth_access_tokens WHERE grant_id = $1")
        .bind(grant_id)
        .execute(&mut *conn)
        .await?;
    if revoked {
        audit::record(
            conn,
            ctx,
            AuditRecord::new(action)
                .target("user", user_id)
                .details(json!({ "grant": grant_id })),
        )
        .await?;
    }
    Ok(revoked)
}

/// Token revocation (RFC 7009): revoking either token revokes the grant.
/// Unknown tokens, and other clients' tokens, are ignored (the response is
/// the same, so nobody learns which tokens exist).
pub async fn revoke_token(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    client_id: &str,
    secret: &str,
) -> OAuthResult<()> {
    let hash = token::hash(secret);
    let found: Option<(Uuid, Uuid)> = if token::looks_valid(secret, ACCESS_PREFIX) {
        sqlx::query_as(
            "SELECT g.id, g.user_id FROM oauth_access_tokens t \
               JOIN oauth_grants g ON g.id = t.grant_id \
               JOIN oauth_clients c ON c.id = g.client_id \
              WHERE t.token_hash = $1 AND c.client_id = $2",
        )
        .bind(&hash)
        .bind(client_id)
        .fetch_optional(&mut *conn)
        .await?
    } else if token::looks_valid(secret, REFRESH_PREFIX) {
        sqlx::query_as(
            "SELECT g.id, g.user_id FROM oauth_grants g \
               JOIN oauth_clients c ON c.id = g.client_id \
              WHERE g.refresh_hash = $1 AND c.client_id = $2",
        )
        .bind(&hash)
        .bind(client_id)
        .fetch_optional(&mut *conn)
        .await?
    } else {
        None
    };
    if let Some((grant_id, user_id)) = found {
        let ctx = ctx.clone().with_actor(huxley_core::Actor::User(user_id));
        revoke_grant_row(conn, &ctx, grant_id, user_id, "auth.oauth_app_disconnected").await?;
    }
    Ok(())
}

/// An app connected to someone's account.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConnectedApp {
    pub id: Uuid,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
    pub last_used_ip: Option<String>,
}

/// The apps connected to `user_id`'s account, newest first.
pub async fn connected_apps(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> OAuthResult<Vec<ConnectedApp>> {
    Ok(sqlx::query_as(
        "SELECT g.id, c.name AS client_name, g.scopes, g.created_at, g.last_used_at, \
                host(g.last_used_ip) AS last_used_ip \
           FROM oauth_grants g JOIN oauth_clients c ON c.id = g.client_id \
          WHERE g.user_id = $1 AND g.revoked_at IS NULL \
            AND g.refresh_expires_at > now() \
          ORDER BY g.created_at DESC",
    )
    .bind(user_id)
    .fetch_all(conn)
    .await?)
}

/// Disconnects one of `user_id`'s apps. `false` if there's no such app.
pub async fn disconnect(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    grant_id: Uuid,
) -> OAuthResult<bool> {
    let owned: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM oauth_grants WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL FOR UPDATE",
    )
    .bind(grant_id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?;
    if owned.is_none() {
        return Ok(false);
    }
    revoke_grant_row(conn, ctx, grant_id, user_id, "auth.oauth_app_disconnected").await
}

#[derive(sqlx::FromRow)]
struct RefreshRow {
    grant_id: Uuid,
    owner: String,
    scopes: Vec<String>,
    expires_at: Option<OffsetDateTime>,
    revoked_at: Option<OffsetDateTime>,
}

#[derive(sqlx::FromRow)]
struct AccessRow {
    grant_id: Uuid,
    user_id: Uuid,
    scopes: Vec<String>,
    resource: String,
    last_used: Option<OffsetDateTime>,
}

/// An authenticated OAuth access token.
#[derive(Debug, Clone)]
pub struct OAuthAccess {
    pub auth: Authenticated,
    /// The resource the token was issued for.
    pub resource: String,
}

impl AuthService {
    /// Resolves an OAuth access token. `None` for unknown or expired tokens,
    /// revoked grants and disabled people. `auth.token` carries the grant's
    /// id and scopes, so authorization narrows to them like an API token's.
    pub async fn authenticate_oauth(
        &self,
        secret: &str,
        ip: Option<std::net::IpAddr>,
    ) -> Result<Option<OAuthAccess>, AuthError> {
        if !token::looks_valid(secret, ACCESS_PREFIX) {
            return Ok(None);
        }
        let row: Option<AccessRow> = sqlx::query_as(
            "SELECT g.id AS grant_id, g.user_id, g.scopes, g.resource, g.last_used_at AS last_used \
                   FROM oauth_access_tokens t \
                   JOIN oauth_grants g ON g.id = t.grant_id \
                   JOIN users u ON u.id = g.user_id \
                  WHERE t.token_hash = $1 AND t.expires_at > now() \
                    AND g.revoked_at IS NULL AND u.status = 'active'",
        )
        .bind(token::hash(secret))
        .fetch_optional(&self.db)
        .await?;
        let Some(AccessRow {
            grant_id,
            user_id,
            scopes,
            resource,
            last_used,
        }) = row
        else {
            return Ok(None);
        };
        let now = OffsetDateTime::now_utc();
        if last_used.is_none_or(|at| now - at > TOUCH_INTERVAL) {
            sqlx::query(
                "UPDATE oauth_grants SET last_used_at = $2, last_used_ip = $3::inet WHERE id = $1",
            )
            .bind(grant_id)
            .bind(now)
            .bind(ip.map(|ip| ip.to_string()))
            .execute(&self.db)
            .await?;
        }
        let mut conn = self.db.acquire().await?;
        Ok(find_user(&mut conn, user_id)
            .await?
            .map(|user| OAuthAccess {
                auth: Authenticated {
                    session_id: grant_id,
                    user,
                    session_mfa: false,
                    token: Some(TokenGrant {
                        id: grant_id,
                        scopes,
                    }),
                },
                resource,
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(uris: &[&str]) -> Client {
        Client {
            id: Uuid::nil(),
            client_id: "hxc_x".into(),
            name: "x".into(),
            redirect_uris: uris.iter().map(|u| (*u).to_owned()).collect(),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn redirect_uris_that_may_be_registered() {
        for ok in [
            "https://claude.ai/api/mcp/auth_callback",
            "http://localhost:33418/callback",
            "http://127.0.0.1/cb",
            "http://[::1]:8080/cb",
            "cursor://anysphere.cursor-retrieval/oauth/callback",
            "com.example.app:/oauth",
        ] {
            assert!(check_redirect_uri(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://example.com/cb",
            "http://localhost.evil.com/cb",
            "https://example.com/cb#frag",
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "/relative",
            "not a url",
        ] {
            assert!(check_redirect_uri(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn loopback_redirects_may_change_port_and_nothing_else() {
        let c = client(&["http://127.0.0.1:1234/callback", "https://app.example/cb"]);
        assert!(redirect_uri_registered(
            &c,
            "http://127.0.0.1:1234/callback"
        ));
        assert!(redirect_uri_registered(
            &c,
            "http://127.0.0.1:5555/callback"
        ));
        assert!(redirect_uri_registered(&c, "http://127.0.0.1/callback"));
        assert!(!redirect_uri_registered(&c, "http://127.0.0.1:5555/other"));
        assert!(!redirect_uri_registered(
            &c,
            "http://localhost:1234/callback"
        ));
        assert!(!redirect_uri_registered(&c, "https://app.example:8443/cb"));
        assert!(!redirect_uri_registered(&c, "https://app.example/cb?x=1"));
        assert!(redirect_uri_registered(&c, "https://app.example/cb"));
    }

    #[test]
    fn pkce_s256() {
        // RFC 7636 appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(challenge_shaped(challenge));
        assert!(pkce_matches(challenge, verifier));
        assert!(!pkce_matches(
            challenge,
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXX"
        ));
        assert!(!pkce_matches(challenge, "short"));
        assert!(!pkce_matches(verifier, verifier));
    }
}
