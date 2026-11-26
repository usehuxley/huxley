//! Two-factor sign-in for portal accounts (ADR-040): an authenticator app,
//! with single-use recovery codes. Asked for after a link or password
//! sign-in; signing in with the company's own identity provider leaves it to
//! that provider. A company can require it (setting `portal.require_mfa`).

use huxley_auth::{token, totp};
use huxley_core::RequestContext;
use huxley_crypto::{Binding, Crypto};
use huxley_db::audit::{self, AuditRecord};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::signin::{NewSession, count_failure, start_session};
use crate::{PortalError, PortalPrincipal, PortalResult};

/// Two-factor challenge tokens.
pub const CHALLENGE_PREFIX: &str = "hxpm_";
const CHALLENGE_TTL: Duration = Duration::minutes(5);
const CHALLENGE_ATTEMPTS: i32 = 5;
const PURPOSE: &str = "portal_totp_secret";

fn binding(account: Uuid) -> Binding<'static> {
    Binding {
        purpose: PURPOSE,
        record: account,
    }
}

fn crypto_err(e: impl std::fmt::Display) -> PortalError {
    PortalError::Sqlx(sqlx::Error::Protocol(e.to_string()))
}

/// Whether a company requires two-factor sign-in in the portal.
pub async fn required(conn: &mut PgConnection, org_id: Uuid) -> PortalResult<bool> {
    let value =
        huxley_orgs::settings::value(conn, org_id, huxley_orgs::settings::PORTAL_REQUIRE_MFA)
            .await
            .map_err(|e| PortalError::Sqlx(sqlx::Error::Protocol(e.to_string())))?;
    Ok(value.as_deref() == Some("true"))
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalMfaStatus)]
pub struct Status {
    /// An authenticator app is set up.
    pub enabled: bool,
    /// Unused recovery codes.
    pub recovery_codes_left: i64,
    /// Their company requires it.
    pub required: bool,
}

pub async fn status(conn: &mut PgConnection, principal: &PortalPrincipal) -> PortalResult<Status> {
    let (enabled, left): (bool, i64) = sqlx::query_as(
        "SELECT totp_confirmed_at IS NOT NULL, \
                (SELECT count(*) FROM portal_recovery_codes r WHERE r.account_id = a.id AND r.used_at IS NULL) \
           FROM portal_accounts a WHERE a.id = $1",
    )
    .bind(principal.account_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(Status {
        enabled,
        recovery_codes_left: left,
        required: required(conn, principal.org_id).await?,
    })
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalTotpEnrollment)]
pub struct Enrollment {
    /// For typing into the app (base32).
    pub secret: String,
    /// For a QR code.
    pub otpauth_uri: String,
}

/// Starts setting up an authenticator (replacing an unfinished setup).
pub async fn begin(
    conn: &mut PgConnection,
    crypto: &Crypto,
    ctx: &RequestContext,
    principal: &PortalPrincipal,
    issuer: &str,
) -> PortalResult<Enrollment> {
    let enabled: bool = sqlx::query_scalar(
        "SELECT totp_confirmed_at IS NOT NULL FROM portal_accounts WHERE id = $1",
    )
    .bind(principal.account_id)
    .fetch_one(&mut *conn)
    .await?;
    if enabled {
        return Err(PortalError::Conflict(
            "two-factor sign-in is already on".into(),
        ));
    }
    let secret = totp::generate_secret();
    let enc = crypto
        .encrypt(principal.org_id, binding(principal.account_id), &secret)
        .await
        .map_err(crypto_err)?;
    sqlx::query(
        "UPDATE portal_accounts SET totp_secret_enc = $2, totp_last_step = NULL WHERE id = $1",
    )
    .bind(principal.account_id)
    .bind(&enc)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.mfa_setup_started")
            .org(principal.org_id)
            .target("portal_account", principal.account_id),
    )
    .await?;
    Ok(Enrollment {
        secret: totp::base32_encode(&secret),
        otpauth_uri: totp::otpauth_uri(issuer, &principal.email, &secret),
    })
}

async fn secret_of(
    conn: &mut PgConnection,
    crypto: &Crypto,
    account: Uuid,
) -> PortalResult<Option<(Vec<u8>, Option<i64>)>> {
    let row: Option<(Option<Vec<u8>>, Option<i64>)> = sqlx::query_as(
        "SELECT totp_secret_enc, totp_last_step FROM portal_accounts WHERE id = $1 FOR UPDATE",
    )
    .bind(account)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some((Some(enc), last)) => {
            let secret = crypto
                .decrypt(binding(account), &enc)
                .await
                .map_err(crypto_err)?;
            Ok(Some((secret.to_vec(), last)))
        }
        _ => Ok(None),
    }
}

/// Finishes setting up with a code from the app: returns ten recovery
/// codes (shown once), ends their other sessions, and lifts this session's
/// setup-only limit.
pub async fn confirm(
    conn: &mut PgConnection,
    crypto: &Crypto,
    ctx: &RequestContext,
    principal: &PortalPrincipal,
    code: &str,
) -> PortalResult<Vec<String>> {
    let confirmed: bool = sqlx::query_scalar(
        "SELECT totp_confirmed_at IS NOT NULL FROM portal_accounts WHERE id = $1",
    )
    .bind(principal.account_id)
    .fetch_one(&mut *conn)
    .await?;
    if confirmed {
        return Err(PortalError::Conflict(
            "two-factor sign-in is already on".into(),
        ));
    }
    let Some((secret, _)) = secret_of(&mut *conn, crypto, principal.account_id).await? else {
        return Err(PortalError::invalid("code", "start setting up first"));
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let Some(step) = totp::verify(&secret, code, now, None) else {
        return Err(PortalError::invalid(
            "code",
            "that code didn't work; check the app's clock",
        ));
    };
    sqlx::query(
        "UPDATE portal_accounts SET totp_confirmed_at = now(), totp_last_step = $2 WHERE id = $1",
    )
    .bind(principal.account_id)
    .bind(step)
    .execute(&mut *conn)
    .await?;
    let codes = new_recovery_codes(&mut *conn, principal.account_id).await?;
    sqlx::query(
        "UPDATE portal_sessions SET revoked_at = now() WHERE account_id = $1 AND id <> $2 AND revoked_at IS NULL",
    )
    .bind(principal.account_id)
    .bind(principal.session_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE portal_sessions SET mfa_setup_required = false WHERE id = $1")
        .bind(principal.session_id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.mfa_enabled")
            .org(principal.org_id)
            .target("portal_account", principal.account_id),
    )
    .await?;
    Ok(codes)
}

async fn new_recovery_codes(conn: &mut PgConnection, account: Uuid) -> PortalResult<Vec<String>> {
    sqlx::query("DELETE FROM portal_recovery_codes WHERE account_id = $1")
        .bind(account)
        .execute(&mut *conn)
        .await?;
    let codes: Vec<String> = (0..totp::RECOVERY_CODE_COUNT)
        .map(|_| totp::generate_recovery_code())
        .collect();
    for code in &codes {
        sqlx::query(
            "INSERT INTO portal_recovery_codes (id, account_id, code_hash) VALUES ($1, $2, $3)",
        )
        .bind(huxley_core::new_id())
        .bind(account)
        .bind(totp::hash_recovery_code(code))
        .execute(&mut *conn)
        .await?;
    }
    Ok(codes)
}

/// Replaces their recovery codes (the old ones stop working).
pub async fn regenerate_recovery_codes(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    principal: &PortalPrincipal,
) -> PortalResult<Vec<String>> {
    let enabled: bool = sqlx::query_scalar(
        "SELECT totp_confirmed_at IS NOT NULL FROM portal_accounts WHERE id = $1",
    )
    .bind(principal.account_id)
    .fetch_one(&mut *conn)
    .await?;
    if !enabled {
        return Err(PortalError::invalid("code", "two-factor sign-in is off"));
    }
    let codes = new_recovery_codes(&mut *conn, principal.account_id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.mfa_recovery_codes_replaced")
            .org(principal.org_id)
            .target("portal_account", principal.account_id),
    )
    .await?;
    Ok(codes)
}

/// Checks an authenticator code (once per time step) or a recovery code
/// (once ever).
async fn check(
    conn: &mut PgConnection,
    crypto: &Crypto,
    account: Uuid,
    code: &str,
) -> PortalResult<bool> {
    let digits: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit()) {
        let Some((secret, last)) = secret_of(&mut *conn, crypto, account).await? else {
            return Ok(false);
        };
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let Some(step) = totp::verify(&secret, &digits, now, last) else {
            return Ok(false);
        };
        sqlx::query("UPDATE portal_accounts SET totp_last_step = $2 WHERE id = $1")
            .bind(account)
            .bind(step)
            .execute(&mut *conn)
            .await?;
        return Ok(true);
    }
    let used = sqlx::query(
        "UPDATE portal_recovery_codes SET used_at = now() WHERE account_id = $1 AND code_hash = $2 AND used_at IS NULL",
    )
    .bind(account)
    .bind(totp::hash_recovery_code(code))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    Ok(used == 1)
}

/// Turns two-factor sign-in off, with a current code. Not while their
/// company requires it.
pub async fn disable(
    conn: &mut PgConnection,
    crypto: &Crypto,
    ctx: &RequestContext,
    principal: &PortalPrincipal,
    code: &str,
) -> PortalResult<()> {
    if required(&mut *conn, principal.org_id).await? {
        return Err(PortalError::invalid(
            "code",
            "your organization requires two-factor sign-in",
        ));
    }
    if !check(&mut *conn, crypto, principal.account_id, code).await? {
        return Err(PortalError::invalid("code", "that code didn't work"));
    }
    clear(&mut *conn, principal.account_id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.mfa_disabled")
            .org(principal.org_id)
            .target("portal_account", principal.account_id),
    )
    .await?;
    Ok(())
}

async fn clear(conn: &mut PgConnection, account: Uuid) -> PortalResult<()> {
    sqlx::query(
        "UPDATE portal_accounts SET totp_secret_enc = NULL, totp_confirmed_at = NULL, totp_last_step = NULL WHERE id = $1",
    )
    .bind(account)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM portal_recovery_codes WHERE account_id = $1")
        .bind(account)
        .execute(conn)
        .await?;
    Ok(())
}

/// Staff turning off a contact's two-factor sign-in (a lost phone): their
/// sessions end, and they set it up again if their company requires it.
pub async fn reset(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    account: Uuid,
) -> PortalResult<()> {
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM portal_accounts WHERE id = $1")
        .bind(account)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(PortalError::NotFound)?;
    clear(&mut *conn, account).await?;
    sqlx::query("UPDATE portal_sessions SET revoked_at = now() WHERE account_id = $1 AND revoked_at IS NULL")
        .bind(account)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.mfa_reset")
            .org(org)
            .target("portal_account", account),
    )
    .await?;
    Ok(())
}

/// A challenge for the code, after the first factor.
pub(crate) async fn challenge(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    account: Uuid,
    method: &'static str,
) -> PortalResult<String> {
    let t = token::generate(CHALLENGE_PREFIX);
    sqlx::query("DELETE FROM portal_mfa_challenges WHERE expires_at < now() - interval '1 hour'")
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO portal_mfa_challenges (id, token_hash, account_id, method, expires_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(huxley_core::new_id())
    .bind(&t.hash)
    .bind(account)
    .bind(method)
    .bind(OffsetDateTime::now_utc() + CHALLENGE_TTL)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.mfa_challenged")
            .target("portal_account", account)
            .details(json!({ "method": method })),
    )
    .await?;
    Ok(t.secret)
}

/// Finishes a sign-in with an authenticator or recovery code. Wrong codes
/// count toward the account's lockout; a challenge takes five.
pub async fn complete(
    conn: &mut PgConnection,
    crypto: &Crypto,
    ctx: &RequestContext,
    challenge: &str,
    code: &str,
    ip: Option<&str>,
    user_agent: Option<&str>,
) -> PortalResult<NewSession> {
    if !token::looks_valid(challenge, CHALLENGE_PREFIX) {
        return Err(PortalError::BadCode);
    }
    let row: Option<(Uuid, Uuid, String, i32)> = sqlx::query_as(
        "SELECT id, account_id, method, failed_attempts FROM portal_mfa_challenges \
          WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now() FOR UPDATE",
    )
    .bind(token::hash(challenge))
    .fetch_optional(&mut *conn)
    .await?;
    let Some((id, account, method, attempts)) = row else {
        return Err(PortalError::BadCode);
    };
    let (active, failed, locked_until): (bool, i32, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT a.status = 'active' AND c.archived_at IS NULL, a.failed_attempts, a.locked_until \
           FROM portal_accounts a JOIN crm_contacts c ON c.id = a.contact_id WHERE a.id = $1 FOR UPDATE OF a",
    )
    .bind(account)
    .fetch_one(&mut *conn)
    .await?;
    let now = OffsetDateTime::now_utc();
    if !active {
        return Err(PortalError::BadCode);
    }
    if let Some(until) = locked_until.filter(|u| *u > now) {
        return Err(PortalError::Locked((until - now).whole_seconds().max(1)));
    }
    if !check(&mut *conn, crypto, account, code).await? {
        sqlx::query(
            "UPDATE portal_mfa_challenges SET failed_attempts = $2, \
                    consumed_at = CASE WHEN $2 >= $3 THEN now() END WHERE id = $1",
        )
        .bind(id)
        .bind(attempts + 1)
        .bind(CHALLENGE_ATTEMPTS)
        .execute(&mut *conn)
        .await?;
        count_failure(&mut *conn, ctx, account, failed, "mfa").await?;
        return Err(PortalError::BadCode);
    }
    sqlx::query("UPDATE portal_mfa_challenges SET consumed_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let session = start_session(&mut *conn, account, ip, user_agent, false).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.signed_in")
            .org(session.principal.org_id)
            .target("portal_account", account)
            .details(json!({ "method": method, "mfa": true })),
    )
    .await?;
    Ok(session)
}
