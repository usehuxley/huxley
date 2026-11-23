//! Multi-factor authentication: TOTP enrollment, the sign-in challenge,
//! recovery codes, and admin reset (design doc §11.2).
//!
//! Sign-in with MFA is two requests. A correct password yields a short-lived
//! challenge token instead of a session; the token plus a valid code yields
//! the session. Wrong codes count toward the same lockout as wrong passwords,
//! and a challenge allows only a few attempts.

use huxley_core::{Actor, RequestContext};
use huxley_crypto::Binding;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{OrgScope, begin_scoped};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use webauthn_rs::prelude::PublicKeyCredential;

use crate::{
    AuthError, AuthResult, AuthService, LoginSuccess, UserStatus, passkey, token, totp,
    verify_password,
};

/// Prefix for MFA challenge tokens.
pub const CHALLENGE_PREFIX: &str = "hxm_";
/// How long the user has to enter a code after their password.
const CHALLENGE_TTL: Duration = Duration::minutes(5);
/// Wrong codes allowed per challenge before it is used up.
const CHALLENGE_MAX_ATTEMPTS: i32 = 5;
/// Shown by authenticator apps next to the account.
const TOTP_ISSUER: &str = "Huxley";
const TOTP_PURPOSE: &str = "user_totp_secret";

/// Second-factor methods a user can complete a challenge with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MfaMethod {
    Totp,
    Passkey,
    RecoveryCode,
}

/// Issued after a correct password. `token` goes to the client once.
#[derive(Debug)]
pub struct MfaChallenge {
    pub token: String,
    pub expires_at: OffsetDateTime,
    pub methods: Vec<MfaMethod>,
}

/// What the user submits to complete a challenge.
#[derive(Debug, Clone, Copy)]
pub enum SecondFactor<'a> {
    Totp(&'a str),
    RecoveryCode(&'a str),
    /// The browser's answer to [`AuthService::begin_mfa_passkey`].
    Passkey {
        ceremony: Uuid,
        credential: &'a PublicKeyCredential,
    },
}

impl SecondFactor<'_> {
    fn label(&self) -> &'static str {
        match self {
            Self::Totp(_) => "totp",
            Self::RecoveryCode(_) => "recovery_code",
            Self::Passkey { .. } => "passkey",
        }
    }
}

/// A user's MFA setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MfaStatus {
    pub totp_enabled: bool,
    pub passkeys: i64,
    pub recovery_codes_remaining: i64,
}

/// Shown once when the user starts setting up an authenticator app.
#[derive(Debug)]
pub struct TotpEnrollment {
    /// Base32, for typing into the app by hand.
    pub secret: String,
    /// For the QR code.
    pub otpauth_uri: String,
}

fn binding(user_id: Uuid) -> Binding<'static> {
    Binding {
        purpose: TOTP_PURPOSE,
        record: user_id,
    }
}

/// Whether the user must pass a second factor to sign in.
pub(crate) async fn has_mfa(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<bool> {
    Ok(has_totp(conn, user_id).await? || passkey::has_passkeys(conn, user_id).await?)
}

async fn has_totp(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<bool> {
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT true FROM user_totp WHERE user_id = $1 AND confirmed_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_optional(conn)
    .await?;
    Ok(row.is_some())
}

/// The methods a user can answer a challenge with.
async fn available_methods(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<Vec<MfaMethod>> {
    let mut methods = Vec::new();
    if has_totp(conn, user_id).await? {
        methods.push(MfaMethod::Totp);
    }
    if passkey::has_passkeys(conn, user_id).await? {
        methods.push(MfaMethod::Passkey);
    }
    if remaining_recovery_codes(conn, user_id).await? > 0 {
        methods.push(MfaMethod::RecoveryCode);
    }
    Ok(methods)
}

/// An unexpired, unused challenge. Returns (challenge id, user id).
pub(crate) async fn find_open_challenge(
    conn: &mut PgConnection,
    token: &str,
) -> AuthResult<(Uuid, Uuid)> {
    if !token::looks_valid(token, CHALLENGE_PREFIX) {
        return Err(AuthError::InvalidMfaChallenge);
    }
    sqlx::query_as(
        "SELECT id, user_id FROM mfa_challenges \
          WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(token::hash(token))
    .fetch_optional(conn)
    .await?
    .ok_or(AuthError::InvalidMfaChallenge)
}

pub(crate) async fn delete_recovery_codes(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> AuthResult<()> {
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Replaces the user's recovery codes with a fresh set and returns them.
pub(crate) async fn replace_recovery_codes(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> AuthResult<Vec<String>> {
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    let codes: Vec<String> = (0..totp::RECOVERY_CODE_COUNT)
        .map(|_| totp::generate_recovery_code())
        .collect();
    for code in &codes {
        sqlx::query("INSERT INTO user_recovery_codes (id, user_id, code_hash) VALUES ($1, $2, $3)")
            .bind(huxley_core::new_id())
            .bind(user_id)
            .bind(totp::hash_recovery_code(code))
            .execute(&mut *conn)
            .await?;
    }
    Ok(codes)
}

async fn remaining_recovery_codes(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<i64> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM user_recovery_codes WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(conn)
    .await?;
    Ok(count)
}

/// Removes every second factor of `user_id` and signs them out everywhere.
/// For an administrator helping a user who lost their authenticator and
/// recovery codes. Runs in the caller's transaction. Returns whether the user
/// had MFA set up.
pub async fn reset_mfa(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
) -> AuthResult<bool> {
    let had = has_mfa(conn, user_id).await?;
    sqlx::query("DELETE FROM user_totp WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM user_passkeys WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE mfa_challenges SET consumed_at = now() WHERE user_id = $1 AND consumed_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *conn)
    .await?;
    let revoked = sqlx::query(
        "UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    audit::record(
        conn,
        ctx,
        AuditRecord::new("auth.mfa_reset")
            .target("user", user_id)
            .details(json!({ "hadMfa": had, "sessionsRevoked": revoked })),
    )
    .await?;
    crate::notify_security(
        conn,
        user_id,
        huxley_notify::kinds::MFA_CHANGED,
        "An administrator removed your two-factor authentication",
    )
    .await?;
    Ok(had)
}

impl AuthService {
    /// The root organization, whose data key protects user-level secrets.
    async fn root_org(&self) -> AuthResult<Uuid> {
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        let root: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM organizations WHERE parent_id IS NULL")
                .fetch_optional(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(root.ok_or(AuthError::NoRootOrg)?.0)
    }

    pub(crate) async fn issue_challenge(
        &self,
        conn: &mut PgConnection,
        ctx: &RequestContext,
        user_id: Uuid,
        now: OffsetDateTime,
    ) -> AuthResult<MfaChallenge> {
        sqlx::query("DELETE FROM mfa_challenges WHERE expires_at < now() - interval '1 hour'")
            .execute(&mut *conn)
            .await?;
        let methods = available_methods(conn, user_id).await?;
        let token = token::generate(CHALLENGE_PREFIX);
        let id = huxley_core::new_id();
        let expires_at = now + CHALLENGE_TTL;
        sqlx::query(
            "INSERT INTO mfa_challenges (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(user_id)
        .bind(&token.hash)
        .bind(expires_at)
        .execute(&mut *conn)
        .await?;
        audit::record(
            conn,
            &ctx.clone().with_actor(Actor::Anonymous),
            AuditRecord::new("auth.mfa_challenged")
                .target("user", user_id)
                .details(json!({ "challenge": id })),
        )
        .await?;
        Ok(MfaChallenge {
            token: token.secret,
            expires_at,
            methods,
        })
    }

    /// Completes a sign-in that [`AuthService::login`] answered with
    /// [`crate::LoginOutcome::MfaRequired`].
    pub async fn complete_mfa(
        &self,
        ctx: &RequestContext,
        challenge: &str,
        factor: SecondFactor<'_>,
    ) -> AuthResult<LoginSuccess> {
        if !token::looks_valid(challenge, CHALLENGE_PREFIX) {
            return Err(AuthError::InvalidMfaChallenge);
        }
        let mut tx = self.db.begin().await?;
        let row: Option<(Uuid, Uuid, i32)> = sqlx::query_as(
            "SELECT id, user_id, failed_attempts FROM mfa_challenges \
              WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now() \
              FOR UPDATE",
        )
        .bind(token::hash(challenge))
        .fetch_optional(&mut *tx)
        .await?;
        let Some((challenge_id, user_id, attempts)) = row else {
            return Err(AuthError::InvalidMfaChallenge);
        };

        let (status, failures, locked_until): (UserStatus, i32, Option<OffsetDateTime>) =
            sqlx::query_as(
                "SELECT status, failed_login_count, locked_until FROM users WHERE id = $1 FOR UPDATE",
            )
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        let now = OffsetDateTime::now_utc();
        if status == UserStatus::Disabled || locked_until.is_some_and(|until| until > now) {
            consume_challenge(&mut tx, challenge_id).await?;
            tx.commit().await?;
            return Err(AuthError::InvalidMfaChallenge);
        }

        let ok = match factor {
            SecondFactor::Totp(code) => self.check_totp(&mut tx, user_id, code, now).await?,
            SecondFactor::RecoveryCode(code) => use_recovery_code(&mut tx, user_id, code).await?,
            SecondFactor::Passkey {
                ceremony,
                credential,
            } => {
                self.check_mfa_passkey(&mut tx, user_id, challenge_id, ceremony, credential)
                    .await?
            }
        };

        if !ok {
            let attempts = attempts + 1;
            sqlx::query(
                "UPDATE mfa_challenges SET failed_attempts = $2, \
                 consumed_at = CASE WHEN $2 >= $3 THEN now() END WHERE id = $1",
            )
            .bind(challenge_id)
            .bind(attempts)
            .bind(CHALLENGE_MAX_ATTEMPTS)
            .execute(&mut *tx)
            .await?;
            self.count_failure(&mut tx, ctx, user_id, failures, now)
                .await?;
            audit::record(
                &mut tx,
                &ctx.clone().with_actor(Actor::Anonymous),
                AuditRecord::new("auth.mfa_failed")
                    .target("user", user_id)
                    .details(json!({ "method": factor.label(), "attempt": attempts })),
            )
            .await?;
            tx.commit().await?;
            return Err(AuthError::InvalidMfaCode);
        }

        consume_challenge(&mut tx, challenge_id).await?;
        if let SecondFactor::RecoveryCode(_) = factor {
            let remaining = remaining_recovery_codes(&mut tx, user_id).await?;
            audit::record(
                &mut tx,
                &ctx.clone().with_actor(Actor::User(user_id)),
                AuditRecord::new("auth.recovery_code_used")
                    .target("user", user_id)
                    .details(json!({ "remaining": remaining })),
            )
            .await?;
        }
        let method = match factor {
            SecondFactor::Totp(_) => "password+totp",
            SecondFactor::RecoveryCode(_) => "password+recovery_code",
            SecondFactor::Passkey { .. } => "password+passkey",
        };
        let success = self
            .finish_login(&mut tx, ctx, user_id, now, method, true)
            .await?;
        tx.commit().await?;
        Ok(success)
    }

    /// Verifies a TOTP code against the user's confirmed authenticator and
    /// records its time step so it can't be replayed.
    async fn check_totp(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        code: &str,
        now: OffsetDateTime,
    ) -> AuthResult<bool> {
        let row: Option<(Vec<u8>, Option<i64>)> = sqlx::query_as(
            "SELECT secret_enc, last_used_step FROM user_totp \
              WHERE user_id = $1 AND confirmed_at IS NOT NULL FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((secret_enc, last_used_step)) = row else {
            return Ok(false);
        };
        let secret = self.crypto.decrypt(binding(user_id), &secret_enc).await?;
        let Some(step) = totp::verify(&secret, code, now.unix_timestamp(), last_used_step) else {
            return Ok(false);
        };
        sqlx::query("UPDATE user_totp SET last_used_step = $2 WHERE user_id = $1")
            .bind(user_id)
            .bind(step)
            .execute(conn)
            .await?;
        Ok(true)
    }

    pub async fn mfa_status(&self, user_id: Uuid) -> AuthResult<MfaStatus> {
        let mut conn = self.db.acquire().await?;
        Ok(MfaStatus {
            totp_enabled: has_totp(&mut conn, user_id).await?,
            passkeys: passkey::passkey_count(&mut conn, user_id).await?,
            recovery_codes_remaining: remaining_recovery_codes(&mut conn, user_id).await?,
        })
    }

    /// Starts authenticator setup after re-checking the password. Replaces any
    /// unfinished setup. Fails if an authenticator is already confirmed.
    pub async fn begin_totp(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        account: &str,
        password: &str,
    ) -> AuthResult<TotpEnrollment> {
        let root = self.root_org().await?;
        let mut tx = self.db.begin().await?;
        self.reauthenticate(&mut tx, ctx, user_id, password).await?;
        if has_totp(&mut tx, user_id).await? {
            return Err(AuthError::MfaAlreadyEnabled);
        }
        let secret = totp::generate_secret();
        let secret_enc = self.crypto.encrypt(root, binding(user_id), &secret).await?;
        sqlx::query(
            "INSERT INTO user_totp (user_id, secret_enc) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET secret_enc = excluded.secret_enc, \
                 created_at = now(), last_used_step = NULL \
             WHERE user_totp.confirmed_at IS NULL",
        )
        .bind(user_id)
        .bind(&secret_enc)
        .execute(&mut *tx)
        .await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.mfa_enrollment_started")
                .target("user", user_id)
                .details(json!({ "method": "totp" })),
        )
        .await?;
        tx.commit().await?;
        Ok(TotpEnrollment {
            secret: totp::base32_encode(&secret),
            otpauth_uri: totp::otpauth_uri(TOTP_ISSUER, account, &secret),
        })
    }

    /// Finishes authenticator setup with a code from the app. Returns a fresh
    /// set of recovery codes (shown once) and signs out every other session.
    pub async fn confirm_totp(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        keep_session: Uuid,
        code: &str,
    ) -> AuthResult<Vec<String>> {
        let mut tx = self.db.begin().await?;
        let row: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT secret_enc FROM user_totp WHERE user_id = $1 AND confirmed_at IS NULL FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((secret_enc,)) = row else {
            return Err(if has_totp(&mut tx, user_id).await? {
                AuthError::MfaAlreadyEnabled
            } else {
                AuthError::NoPendingEnrollment
            });
        };
        let secret = self.crypto.decrypt(binding(user_id), &secret_enc).await?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let Some(step) = totp::verify(&secret, code, now, None) else {
            return Err(AuthError::InvalidMfaCode);
        };
        sqlx::query(
            "UPDATE user_totp SET confirmed_at = now(), last_used_step = $2 WHERE user_id = $1",
        )
        .bind(user_id)
        .bind(step)
        .execute(&mut *tx)
        .await?;
        let codes = replace_recovery_codes(&mut tx, user_id).await?;
        let revoked = revoke_other_sessions(&mut tx, user_id, keep_session).await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.mfa_enabled")
                .target("user", user_id)
                .details(json!({ "method": "totp", "otherSessionsRevoked": revoked })),
        )
        .await?;
        crate::notify_security(
            &mut tx,
            user_id,
            huxley_notify::kinds::MFA_CHANGED,
            "An authenticator app was turned on",
        )
        .await?;
        tx.commit().await?;
        Ok(codes)
    }

    /// Turns off the authenticator and deletes recovery codes after
    /// re-checking the password.
    pub async fn disable_totp(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        password: &str,
    ) -> AuthResult<()> {
        let mut tx = self.db.begin().await?;
        self.reauthenticate(&mut tx, ctx, user_id, password).await?;
        if !has_totp(&mut tx, user_id).await? {
            return Err(AuthError::MfaNotEnabled);
        }
        sqlx::query("DELETE FROM user_totp WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        // Recovery codes stay while a passkey still protects the account.
        if !passkey::has_passkeys(&mut tx, user_id).await? {
            delete_recovery_codes(&mut tx, user_id).await?;
        }
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.mfa_disabled")
                .target("user", user_id)
                .details(json!({ "method": "totp" })),
        )
        .await?;
        crate::notify_security(
            &mut tx,
            user_id,
            huxley_notify::kinds::MFA_CHANGED,
            "Your authenticator app was turned off",
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Replaces the recovery codes after re-checking the password.
    pub async fn regenerate_recovery_codes(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        password: &str,
    ) -> AuthResult<Vec<String>> {
        let mut tx = self.db.begin().await?;
        self.reauthenticate(&mut tx, ctx, user_id, password).await?;
        if !has_mfa(&mut tx, user_id).await? {
            return Err(AuthError::MfaNotEnabled);
        }
        let codes = replace_recovery_codes(&mut tx, user_id).await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.recovery_codes_regenerated").target("user", user_id),
        )
        .await?;
        tx.commit().await?;
        Ok(codes)
    }

    /// Re-checks the signed-in user's password. A failure is audited and
    /// committed on its own before the error returns.
    pub(crate) async fn reauthenticate(
        &self,
        tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        ctx: &RequestContext,
        user_id: Uuid,
        password: &str,
    ) -> AuthResult<()> {
        if verify_password(tx, user_id, password).await? {
            return Ok(());
        }
        // Record the failure outside the caller's transaction, which is
        // about to roll back.
        let mut audit_tx = self.db.begin().await?;
        audit::record(
            &mut audit_tx,
            ctx,
            AuditRecord::new("auth.reauthentication_failed").target("user", user_id),
        )
        .await?;
        audit_tx.commit().await?;
        Err(AuthError::WrongCurrentPassword)
    }
}

async fn consume_challenge(conn: &mut PgConnection, id: Uuid) -> AuthResult<()> {
    sqlx::query("UPDATE mfa_challenges SET consumed_at = now() WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

async fn use_recovery_code(conn: &mut PgConnection, user_id: Uuid, code: &str) -> AuthResult<bool> {
    let used = sqlx::query(
        "UPDATE user_recovery_codes SET used_at = now() \
          WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL",
    )
    .bind(user_id)
    .bind(totp::hash_recovery_code(code))
    .execute(conn)
    .await?
    .rows_affected();
    Ok(used == 1)
}

async fn revoke_other_sessions(
    conn: &mut PgConnection,
    user_id: Uuid,
    keep: Uuid,
) -> AuthResult<u64> {
    Ok(sqlx::query(
        "UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(keep)
    .execute(conn)
    .await?
    .rows_affected())
}
