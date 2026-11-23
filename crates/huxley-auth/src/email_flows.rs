//! Password reset, invitations, and email verification (design doc §11.2).
//!
//! Each flow emails a link carrying a single-use token (`hxe_` + 256 random
//! bits; only its SHA-256 is stored). Requests that could reveal whether an
//! account exists answer the same way either way and send in the background.
//!
//! - **Password reset** (30 minutes): only for people with a password. SSO-only
//!   accounts and service accounts get nothing, so reset can't bypass single
//!   sign-on. Using it sets the password, signs out every session, clears any
//!   lockout, and counts as verifying the address. MFA still applies at the
//!   next sign-in.
//! - **Invitation** (72 hours): a reset that reads as "set your password", for
//!   accounts an administrator created.
//! - **Verification** (24 hours): proves the user receives mail at their
//!   address. Only valid while the address is unchanged.

use huxley_core::{Actor, RequestContext};
use huxley_db::audit::{self, AuditRecord};
use huxley_email::action_email;
use serde_json::json;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    AuthError, AuthResult, AuthService, User, UserKind, UserStatus, find_user, password, token,
};

pub const PREFIX: &str = "hxe_";
const RESET_TTL: Duration = Duration::minutes(30);
const INVITE_TTL: Duration = Duration::hours(72);
const VERIFY_TTL: Duration = Duration::hours(24);
/// At most one reset email per account this often.
const RESET_THROTTLE: Duration = Duration::seconds(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "user_token_purpose", rename_all = "snake_case")]
enum Purpose {
    PasswordReset,
    Invite,
    EmailVerification,
}

/// Creates a token for `user`, replacing their unused ones of the same kind.
async fn issue(
    conn: &mut PgConnection,
    user: &User,
    purpose: Purpose,
    ttl: Duration,
) -> AuthResult<String> {
    sqlx::query("UPDATE user_tokens SET used_at = now() WHERE user_id = $1 AND purpose = $2 AND used_at IS NULL")
        .bind(user.id)
        .bind(purpose)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM user_tokens WHERE expires_at < now() - interval '7 days'")
        .execute(&mut *conn)
        .await?;
    let generated = token::generate(PREFIX);
    sqlx::query(
        "INSERT INTO user_tokens (id, user_id, purpose, token_hash, email, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(huxley_core::new_id())
    .bind(user.id)
    .bind(purpose)
    .bind(&generated.hash)
    .bind(&user.email)
    .bind(OffsetDateTime::now_utc() + ttl)
    .execute(conn)
    .await?;
    Ok(generated.secret)
}

/// Consumes a valid token of one of `purposes`. Returns (user id, purpose,
/// address it was sent to).
async fn consume(
    conn: &mut PgConnection,
    secret: &str,
    purposes: &[Purpose],
) -> AuthResult<(Uuid, Purpose, String)> {
    if !token::looks_valid(secret, PREFIX) {
        return Err(AuthError::InvalidEmailToken);
    }
    let row: Option<(Uuid, Purpose, String)> = sqlx::query_as(
        "UPDATE user_tokens SET used_at = now() \
          WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() \
         RETURNING user_id, purpose, email",
    )
    .bind(token::hash(secret))
    .fetch_optional(conn)
    .await?;
    match row {
        Some(row) if purposes.contains(&row.1) => Ok(row),
        _ => Err(AuthError::InvalidEmailToken),
    }
}

fn has_password_sql() -> &'static str {
    "SELECT EXISTS (SELECT 1 FROM user_passwords WHERE user_id = $1)"
}

impl AuthService {
    /// Email features work only with an SMTP server and a public URL for links.
    pub fn email_enabled(&self) -> bool {
        self.mailer.enabled() && self.config.public_url.is_some()
    }

    fn link(&self, path: &str, secret: &str) -> AuthResult<String> {
        let base = self
            .config
            .public_url
            .as_deref()
            .ok_or(AuthError::EmailNotConfigured)?;
        Ok(format!(
            "{}{path}?token={secret}",
            base.trim_end_matches('/')
        ))
    }

    /// Emails a reset link if `email` belongs to an active person with a
    /// password. Always succeeds (when email is configured), so the response
    /// doesn't reveal whether the account exists.
    pub async fn request_password_reset(
        &self,
        ctx: &RequestContext,
        email: &str,
    ) -> AuthResult<()> {
        if !self.email_enabled() {
            return Err(AuthError::EmailNotConfigured);
        }
        let mut tx = self.db.begin().await?;
        let user = crate::find_user_by_email(&mut tx, email).await?;
        let eligible = match &user {
            Some(u) if u.kind == UserKind::Human && u.status == UserStatus::Active => {
                let (has_password,): (bool,) = sqlx::query_as(has_password_sql())
                    .bind(u.id)
                    .fetch_one(&mut *tx)
                    .await?;
                let (recent,): (bool,) = sqlx::query_as(
                    "SELECT EXISTS (SELECT 1 FROM user_tokens WHERE user_id = $1 \
                       AND purpose = 'password_reset' AND created_at > $2)",
                )
                .bind(u.id)
                .bind(OffsetDateTime::now_utc() - RESET_THROTTLE)
                .fetch_one(&mut *tx)
                .await?;
                if !has_password {
                    Err("no_password")
                } else if recent {
                    Err("throttled")
                } else {
                    Ok(u.clone())
                }
            }
            Some(_) => Err("ineligible"),
            None => Err("unknown_user"),
        };
        let anonymous = ctx.clone().with_actor(Actor::Anonymous);
        match eligible {
            Ok(user) => {
                let secret = issue(&mut tx, &user, Purpose::PasswordReset, RESET_TTL).await?;
                audit::record(
                    &mut tx,
                    &anonymous,
                    AuditRecord::new("auth.password_reset_requested").target("user", user.id),
                )
                .await?;
                tx.commit().await?;
                self.mailer.send_later(action_email(
                    &user.email,
                    "Reset your Huxley password",
                    &format!("Hi {},", user.display_name),
                    "Someone (hopefully you) asked to reset your Huxley password. The link works once, for 30 minutes.",
                    "Choose a new password",
                    &self.link("/reset-password", &secret)?,
                    "If you didn't ask for this, you can ignore this email: your password hasn't changed.",
                ));
            }
            Err(reason) => {
                let mut record = AuditRecord::new("auth.password_reset_refused")
                    .details(json!({ "reason": reason }));
                if let Some(u) = &user {
                    record = record.target("user", u.id);
                }
                audit::record(&mut tx, &anonymous, record).await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    /// Sets a new password with a reset or invitation link. Signs out every
    /// session and verifies the address the link was sent to.
    pub async fn reset_password(
        &self,
        ctx: &RequestContext,
        secret: &str,
        new_password: &str,
    ) -> AuthResult<User> {
        password::validate(new_password)?;
        let mut tx = self.db.begin().await?;
        let (user_id, purpose, sent_to) =
            consume(&mut tx, secret, &[Purpose::PasswordReset, Purpose::Invite]).await?;
        let user = find_user(&mut tx, user_id)
            .await?
            .ok_or(AuthError::InvalidEmailToken)?;
        if user.status != UserStatus::Active || user.kind != UserKind::Human {
            return Err(AuthError::InvalidEmailToken);
        }
        sqlx::query(
            "INSERT INTO user_passwords (user_id, hash) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET hash = excluded.hash, updated_at = now()",
        )
        .bind(user_id)
        .bind(password::hash(new_password))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE user_tokens SET used_at = now() WHERE user_id = $1 \
               AND purpose IN ('password_reset', 'invite') AND used_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        let revoked = sqlx::query(
            "UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "UPDATE users SET failed_login_count = 0, locked_until = NULL, \
                    email_verified_at = CASE WHEN lower(email) = lower($2) \
                                             THEN coalesce(email_verified_at, now()) \
                                             ELSE email_verified_at END \
              WHERE id = $1",
        )
        .bind(user_id)
        .bind(&sent_to)
        .execute(&mut *tx)
        .await?;
        let action = if purpose == Purpose::Invite {
            "auth.invitation_accepted"
        } else {
            "auth.password_reset"
        };
        audit::record(
            &mut tx,
            &ctx.clone().with_actor(Actor::User(user_id)),
            AuditRecord::new(action)
                .target("user", user_id)
                .details(json!({ "sessionsRevoked": revoked })),
        )
        .await?;
        if purpose == Purpose::PasswordReset {
            crate::notify_security(
                &mut tx,
                user_id,
                huxley_notify::kinds::PASSWORD_CHANGED,
                "Your password was reset",
            )
            .await?;
        }
        let user = find_user(&mut tx, user_id)
            .await?
            .ok_or(AuthError::InvalidEmailToken)?;
        tx.commit().await?;
        Ok(user)
    }

    /// Emails an invitation to set a password. For accounts an administrator
    /// just created. Runs in its own transaction after the user exists.
    pub async fn send_invitation(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        inviter: &str,
    ) -> AuthResult<()> {
        if !self.email_enabled() {
            return Err(AuthError::EmailNotConfigured);
        }
        let mut tx = self.db.begin().await?;
        let user = find_user(&mut tx, user_id)
            .await?
            .ok_or(AuthError::UserNotFound)?;
        if user.kind != UserKind::Human {
            return Err(AuthError::UserNotFound);
        }
        let secret = issue(&mut tx, &user, Purpose::Invite, INVITE_TTL).await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.invitation_sent").target("user", user_id),
        )
        .await?;
        tx.commit().await?;
        self.mailer
            .send(action_email(
                &user.email,
                "You've been invited to Huxley",
                &format!("Hi {},", user.display_name),
                &format!("{inviter} created a Huxley account for you. Choose a password to sign in. The link works once, for 3 days."),
                "Set your password",
                &self.link("/reset-password", &secret)?,
                "If you weren't expecting this, you can ignore this email.",
            ))
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "failed to send invitation");
                AuthError::EmailNotConfigured
            })
    }

    /// Emails the signed-in user a link to verify their address.
    pub async fn request_email_verification(
        &self,
        ctx: &RequestContext,
        user: &User,
    ) -> AuthResult<()> {
        if !self.email_enabled() {
            return Err(AuthError::EmailNotConfigured);
        }
        let mut tx = self.db.begin().await?;
        let secret = issue(&mut tx, user, Purpose::EmailVerification, VERIFY_TTL).await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.email_verification_sent").target("user", user.id),
        )
        .await?;
        tx.commit().await?;
        self.mailer.send_later(action_email(
            &user.email,
            "Verify your email for Huxley",
            &format!("Hi {},", user.display_name),
            "Confirm this is your email address so Huxley can send you notifications and password resets.",
            "Verify email",
            &self.link("/verify-email", &secret)?,
            "If you didn't ask for this, you can ignore this email.",
        ));
        Ok(())
    }

    /// Marks the address verified with a verification link, if it's still the
    /// user's address.
    pub async fn verify_email(&self, ctx: &RequestContext, secret: &str) -> AuthResult<User> {
        let mut tx = self.db.begin().await?;
        let (user_id, _, sent_to) = consume(&mut tx, secret, &[Purpose::EmailVerification]).await?;
        let updated = sqlx::query(
            "UPDATE users SET email_verified_at = coalesce(email_verified_at, now()) \
              WHERE id = $1 AND lower(email) = lower($2)",
        )
        .bind(user_id)
        .bind(&sent_to)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(AuthError::InvalidEmailToken);
        }
        audit::record(
            &mut tx,
            &ctx.clone().with_actor(Actor::User(user_id)),
            AuditRecord::new("auth.email_verified").target("user", user_id),
        )
        .await?;
        let user = find_user(&mut tx, user_id)
            .await?
            .ok_or(AuthError::InvalidEmailToken)?;
        tx.commit().await?;
        Ok(user)
    }
}
