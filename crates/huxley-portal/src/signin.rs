//! Signing in to the portal (ADR-040): by an emailed single-use link (the
//! default), or with a password once the account has one. Sessions are
//! portal sessions: their own table, token prefix and cookie.

use huxley_auth::{password, token};
use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde_json::json;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{PortalError, PortalPrincipal, PortalResult};

/// Portal session tokens.
pub const SESSION_PREFIX: &str = "hxps_";
/// Sign-in link tokens.
pub const LINK_PREFIX: &str = "hxpl_";
/// How long a sign-in link works.
pub const LINK_TTL: Duration = Duration::minutes(15);
/// At most one link a minute per account.
const LINK_THROTTLE: Duration = Duration::minutes(1);
/// A session ends after this long unused...
pub const SESSION_IDLE: Duration = Duration::hours(24);
/// ...and after this long regardless.
pub const SESSION_ABSOLUTE: Duration = Duration::days(14);
/// Lockout after this many wrong passwords in a row.
const LOCK_AFTER: i32 = 5;

/// A new session: its secret (for the cookie) and how long it can last.
#[derive(Debug)]
pub struct NewSession {
    pub secret: String,
    pub principal: PortalPrincipal,
    pub max_age: Duration,
}

/// Issues a sign-in link for an account (invited or active): its secret.
pub async fn issue_link(conn: &mut PgConnection, account_id: Uuid) -> PortalResult<String> {
    let t = token::generate(LINK_PREFIX);
    sqlx::query(
        "INSERT INTO portal_login_tokens (id, account_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(huxley_core::new_id())
    .bind(account_id)
    .bind(&t.hash)
    .bind(OffsetDateTime::now_utc() + LINK_TTL)
    .execute(conn)
    .await?;
    Ok(t.secret)
}

/// A sign-in link for whoever has portal access as `email`, if anyone
/// (invited or active, at most one a minute): the account and the link's
/// secret, for the caller to email. Callers answer the same either way.
pub async fn request_link(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    email: &str,
) -> PortalResult<Option<(Uuid, String, String)>> {
    let found: Option<(Uuid, String, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT a.id, a.email, (SELECT max(created_at) FROM portal_login_tokens t WHERE t.account_id = a.id) \
           FROM portal_accounts a JOIN crm_contacts c ON c.id = a.contact_id \
          WHERE lower(a.email) = lower($1) AND a.status IN ('invited', 'active') AND c.archived_at IS NULL",
    )
    .bind(email.trim())
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, address, last)) = found else {
        return Ok(None);
    };
    if last.is_some_and(|at| at > OffsetDateTime::now_utc() - LINK_THROTTLE) {
        return Ok(None);
    }
    let secret = issue_link(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.link_requested").target("portal_account", id),
    )
    .await?;
    Ok(Some((id, address, secret)))
}

async fn start_session(
    conn: &mut PgConnection,
    account_id: Uuid,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> PortalResult<NewSession> {
    let t = token::generate(SESSION_PREFIX);
    let now = OffsetDateTime::now_utc();
    let session_id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO portal_sessions (id, account_id, token_hash, idle_expires_at, absolute_expires_at, ip, user_agent) \
         VALUES ($1, $2, $3, $4, $5, $6::inet, $7)",
    )
    .bind(session_id)
    .bind(account_id)
    .bind(&t.hash)
    .bind(now + SESSION_IDLE)
    .bind(now + SESSION_ABSOLUTE)
    .bind(ip)
    .bind(user_agent.map(|u| u.chars().take(500).collect::<String>()))
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE portal_accounts SET last_sign_in_at = now(), failed_attempts = 0, locked_until = NULL, \
                status = CASE WHEN status = 'invited' THEN 'active' ELSE status END, \
                org_id = (SELECT org_id FROM crm_contacts WHERE id = portal_accounts.contact_id) \
          WHERE id = $1",
    )
    .bind(account_id)
    .execute(&mut *conn)
    .await?;
    let principal = principal_for(conn, session_id)
        .await?
        .ok_or(PortalError::BadCredentials)?;
    Ok(NewSession {
        secret: t.secret,
        principal,
        max_age: SESSION_ABSOLUTE,
    })
}

/// Signs in with a sign-in link (once, before it expires). An invited
/// account becomes active.
pub async fn redeem(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    secret: &str,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> PortalResult<NewSession> {
    if !token::looks_valid(secret, LINK_PREFIX) {
        return Err(PortalError::BadCredentials);
    }
    let account: Option<Uuid> = sqlx::query_scalar(
        "UPDATE portal_login_tokens t SET used_at = now() FROM portal_accounts a \
          WHERE t.token_hash = $1 AND t.used_at IS NULL AND t.expires_at > now() \
            AND a.id = t.account_id AND a.status IN ('invited', 'active') \
          RETURNING t.account_id",
    )
    .bind(token::hash(secret))
    .fetch_optional(&mut *conn)
    .await?;
    let Some(account) = account else {
        return Err(PortalError::BadCredentials);
    };
    let session = start_session(&mut *conn, account, ip, user_agent).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.signed_in")
            .org(session.principal.org_id)
            .target("portal_account", account)
            .details(json!({ "method": "link" })),
    )
    .await?;
    Ok(session)
}

/// Signs in with an email and password. Wrong passwords lock the account
/// for a while after five in a row.
pub async fn sign_in_with_password(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    email: &str,
    given: &str,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> PortalResult<NewSession> {
    let found: Option<(Uuid, Option<String>, i32, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT a.id, a.password_hash, a.failed_attempts, a.locked_until \
           FROM portal_accounts a JOIN crm_contacts c ON c.id = a.contact_id \
          WHERE lower(a.email) = lower($1) AND a.status = 'active' AND c.archived_at IS NULL",
    )
    .bind(email.trim())
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, Some(hash), failed, locked_until)) = found else {
        password::verify_dummy(given);
        return Err(PortalError::BadCredentials);
    };
    if let Some(until) = locked_until.filter(|u| *u > OffsetDateTime::now_utc()) {
        return Err(PortalError::Locked(
            (until - OffsetDateTime::now_utc()).whole_seconds().max(1),
        ));
    }
    if !password::verify(given, &hash) {
        let failed = failed + 1;
        let lock = (failed >= LOCK_AFTER).then(|| {
            let doublings = u32::try_from(failed - LOCK_AFTER).unwrap_or(0).min(6);
            OffsetDateTime::now_utc() + Duration::seconds(60 * 2_i64.pow(doublings))
        });
        sqlx::query(
            "UPDATE portal_accounts SET failed_attempts = $2, locked_until = $3 WHERE id = $1",
        )
        .bind(id)
        .bind(failed)
        .bind(lock)
        .execute(&mut *conn)
        .await?;
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("portal.sign_in_failed")
                .target("portal_account", id)
                .details(json!({ "attempts": failed })),
        )
        .await?;
        return Err(PortalError::BadCredentials);
    }
    let session = start_session(&mut *conn, id, ip, user_agent).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.signed_in")
            .org(session.principal.org_id)
            .target("portal_account", id)
            .details(json!({ "method": "password" })),
    )
    .await?;
    Ok(session)
}

/// Sets (or changes) a signed-in account's password.
pub async fn set_password(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    principal: &PortalPrincipal,
    new_password: &str,
) -> PortalResult<()> {
    password::validate(new_password)
        .map_err(|e| PortalError::invalid("password", e.to_string()))?;
    sqlx::query("UPDATE portal_accounts SET password_hash = $2, updated_at = now() WHERE id = $1")
        .bind(principal.account_id)
        .bind(password::hash(new_password))
        .execute(&mut *conn)
        .await?;
    // Other sessions end; this one stays.
    sqlx::query(
        "UPDATE portal_sessions SET revoked_at = now() WHERE account_id = $1 AND id <> $2 AND revoked_at IS NULL",
    )
    .bind(principal.account_id)
    .bind(principal.session_id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.password_set")
            .org(principal.org_id)
            .target("portal_account", principal.account_id),
    )
    .await?;
    Ok(())
}

async fn principal_for(
    conn: &mut PgConnection,
    session_id: Uuid,
) -> PortalResult<Option<PortalPrincipal>> {
    Ok(sqlx::query_as(
        "SELECT a.id AS account_id, a.contact_id, a.org_id, o.name AS org_name, \
                btrim(c.first_name || ' ' || c.last_name) AS name, a.email, a.role, s.id AS session_id \
           FROM portal_sessions s JOIN portal_accounts a ON a.id = s.account_id \
           JOIN crm_contacts c ON c.id = a.contact_id JOIN organizations o ON o.id = a.org_id \
          WHERE s.id = $1",
    )
    .bind(session_id)
    .fetch_optional(conn)
    .await?)
}

/// Who a portal session token belongs to, if it's live (and the account
/// active). Keeps the session alive.
pub async fn authenticate(
    conn: &mut PgConnection,
    secret: &str,
) -> PortalResult<Option<PortalPrincipal>> {
    if !token::looks_valid(secret, SESSION_PREFIX) {
        return Ok(None);
    }
    let session: Option<Uuid> = sqlx::query_scalar(
        "UPDATE portal_sessions s SET last_seen_at = now(), \
                idle_expires_at = LEAST(now() + $2::interval, s.absolute_expires_at) \
           FROM portal_accounts a JOIN crm_contacts c ON c.id = a.contact_id \
          WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.idle_expires_at > now() \
            AND s.absolute_expires_at > now() AND a.id = s.account_id AND a.status = 'active' \
            AND c.archived_at IS NULL \
          RETURNING s.id",
    )
    .bind(token::hash(secret))
    .bind(format!("{} hours", SESSION_IDLE.whole_hours()))
    .fetch_optional(&mut *conn)
    .await?;
    match session {
        Some(id) => principal_for(conn, id).await,
        None => Ok(None),
    }
}

/// Ends a session.
pub async fn sign_out(conn: &mut PgConnection, session_id: Uuid) -> PortalResult<()> {
    sqlx::query(
        "UPDATE portal_sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .execute(conn)
    .await?;
    Ok(())
}
