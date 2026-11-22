//! Authentication for Huxley (design doc §11.2).
//!
//! This slice covers local accounts: Argon2id passwords, server-side sessions
//! with idle and absolute timeouts, and login lockout with backoff. MFA, OIDC,
//! and API tokens build on the same pieces.

pub mod password;
pub mod token;

use huxley_core::{Actor, RequestContext};
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{DbError, violated_constraint};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub use password::PasswordPolicyError;

/// Authentication settings (`[auth]` in `huxley.toml`, or `HUXLEY_AUTH__*`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// A session expires after this many minutes without activity.
    pub session_idle_minutes: i64,
    /// A session expires this many hours after login regardless of activity.
    pub session_absolute_hours: i64,
    /// Failed attempts before the account locks temporarily.
    pub lockout_threshold: i32,
    /// First lockout duration; doubles with each further failure.
    pub lockout_base_seconds: i64,
    /// Upper bound on a single lockout.
    pub lockout_max_seconds: i64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_idle_minutes: 8 * 60,
            session_absolute_hours: 7 * 24,
            lockout_threshold: 5,
            lockout_base_seconds: 60,
            lockout_max_seconds: 3600,
        }
    }
}

impl AuthConfig {
    fn idle(&self) -> Duration {
        Duration::minutes(self.session_idle_minutes)
    }

    fn absolute(&self) -> Duration {
        Duration::hours(self.session_absolute_hours)
    }

    /// Lockout after the `failures`th consecutive failure, if any.
    fn lockout_for(&self, failures: i32) -> Option<Duration> {
        let over = failures - self.lockout_threshold;
        if over < 0 {
            return None;
        }
        let exponent = u32::try_from(over.min(20)).unwrap_or(20);
        let seconds = self
            .lockout_base_seconds
            .saturating_mul(1_i64 << exponent)
            .min(self.lockout_max_seconds);
        Some(Duration::seconds(seconds))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "user_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub status: UserStatus,
    pub last_login_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id: Uuid,
    pub user_id: Uuid,
    pub created_at: OffsetDateTime,
    pub last_seen_at: Option<OffsetDateTime>,
    pub absolute_expires_at: OffsetDateTime,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

/// A validated session and its user.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub session_id: Uuid,
    pub user: User,
}

/// A successful login. `token` must be sent to the client exactly once.
#[derive(Debug)]
pub struct LoginSuccess {
    pub user: User,
    pub session_id: Uuid,
    pub token: String,
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Wrong email or password, disabled, or locked. Deliberately one variant:
    /// responses must not reveal which.
    #[error("invalid email or password")]
    InvalidCredentials,
    #[error("an account with this email already exists")]
    EmailTaken,
    #[error("invalid email address")]
    InvalidEmail,
    #[error("invalid display name")]
    InvalidDisplayName,
    #[error(transparent)]
    WeakPassword(#[from] PasswordPolicyError),
    #[error("session not found")]
    SessionNotFound,
    #[error("user not found")]
    UserNotFound,
    #[error("current password is incorrect")]
    WrongCurrentPassword,
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for AuthError {
    fn from(error: sqlx::Error) -> Self {
        match violated_constraint(&error) {
            Some("users_email_unique") => Self::EmailTaken,
            _ => Self::Db(DbError::Sqlx(error)),
        }
    }
}

pub type AuthResult<T> = Result<T, AuthError>;

/// Sessions are only "touched" (idle timeout extended) this often, to avoid a
/// write on every request.
const TOUCH_INTERVAL: Duration = Duration::seconds(60);

fn normalize_email(email: &str) -> AuthResult<String> {
    let email = email.trim();
    let valid = email.len() <= 320
        && !email.chars().any(char::is_whitespace)
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    if valid {
        Ok(email.to_owned())
    } else {
        Err(AuthError::InvalidEmail)
    }
}

fn normalize_display_name(name: &str) -> AuthResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
        return Err(AuthError::InvalidDisplayName);
    }
    Ok(name.to_owned())
}

// sqlx requires static SQL.
macro_rules! select_user {
    ($rest:literal) => {
        concat!(
            "SELECT id, email, display_name, status, last_login_at, created_at FROM users ",
            $rest
        )
    };
}

/// Creates a local user with a password. Runs in the caller's transaction.
pub async fn create_user(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    email: &str,
    display_name: &str,
    password: &str,
) -> AuthResult<User> {
    let email = normalize_email(email)?;
    let display_name = normalize_display_name(display_name)?;
    password::validate(password)?;
    let hash = password::hash(password);

    let id = huxley_core::new_id();
    let user: User = sqlx::query_as(
        "INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3) \
         RETURNING id, email, display_name, status, last_login_at, created_at",
    )
    .bind(id)
    .bind(&email)
    .bind(&display_name)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query("INSERT INTO user_passwords (user_id, hash) VALUES ($1, $2)")
        .bind(id)
        .bind(&hash)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("user.created")
            .target("user", id)
            .details(json!({ "after": { "email": email, "displayName": display_name } })),
    )
    .await?;
    Ok(user)
}

pub async fn find_user(conn: &mut PgConnection, id: Uuid) -> AuthResult<Option<User>> {
    Ok(sqlx::query_as(select_user!("WHERE id = $1"))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// Users ordered by creation (UUIDv7 ids sort by time), after `after` if given.
/// Returns up to `limit + 1` rows so the caller can tell whether more exist.
pub async fn list_users(
    conn: &mut PgConnection,
    after: Option<Uuid>,
    limit: u32,
) -> AuthResult<Vec<User>> {
    Ok(sqlx::query_as(select_user!(
        "WHERE ($1::uuid IS NULL OR id > $1) ORDER BY id LIMIT $2"
    ))
    .bind(after)
    .bind(i64::from(limit) + 1)
    .fetch_all(conn)
    .await?)
}

/// Changes a user's display name and/or status. Disabling revokes every
/// session in the same transaction, so access ends immediately.
pub async fn update_user(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    display_name: Option<&str>,
    status: Option<UserStatus>,
) -> AuthResult<User> {
    let before = find_user(conn, id).await?.ok_or(AuthError::UserNotFound)?;
    let display_name = display_name.map(normalize_display_name).transpose()?;
    let user: User = sqlx::query_as(
        "UPDATE users SET display_name = coalesce($2, display_name), \
                status = coalesce($3, status), version = version + 1 \
          WHERE id = $1 \
         RETURNING id, email, display_name, status, last_login_at, created_at",
    )
    .bind(id)
    .bind(display_name.as_deref())
    .bind(status)
    .fetch_one(&mut *conn)
    .await?;
    let mut revoked = 0;
    if before.status == UserStatus::Active && user.status == UserStatus::Disabled {
        revoked = sqlx::query(
            "UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("user.updated")
            .target("user", id)
            .details(json!({
                "before": { "displayName": before.display_name, "status": before.status },
                "after": { "displayName": user.display_name, "status": user.status },
                "sessionsRevoked": revoked,
            })),
    )
    .await?;
    Ok(user)
}

/// Authentication operations that own their transactions.
#[derive(Debug, Clone)]
pub struct AuthService {
    db: PgPool,
    config: AuthConfig,
}

#[derive(sqlx::FromRow)]
struct LoginRow {
    id: Uuid,
    status: UserStatus,
    failed_login_count: i32,
    locked_until: Option<OffsetDateTime>,
    hash: Option<String>,
}

impl AuthService {
    pub fn new(db: PgPool, config: AuthConfig) -> Self {
        Self { db, config }
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    /// Verifies credentials and starts a session.
    ///
    /// Every failure returns [`AuthError::InvalidCredentials`] and takes about
    /// the same time, whether the email is unknown, the password is wrong, or
    /// the account is disabled or locked.
    pub async fn login(
        &self,
        ctx: &RequestContext,
        email: &str,
        password: &str,
    ) -> AuthResult<LoginSuccess> {
        let email = email.trim();
        let mut tx = self.db.begin().await?;
        let row: Option<LoginRow> = sqlx::query_as(
            "SELECT u.id, u.status, u.failed_login_count, u.locked_until, p.hash \
               FROM users u LEFT JOIN user_passwords p ON p.user_id = u.id \
              WHERE lower(u.email) = lower($1) \
              FOR UPDATE OF u",
        )
        .bind(email)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            password::verify_dummy(password);
            self.audit_failure(&mut tx, ctx, None, email, "unknown_user")
                .await?;
            tx.commit().await?;
            return Err(AuthError::InvalidCredentials);
        };

        let now = OffsetDateTime::now_utc();
        let password_ok = if let Some(hash) = &row.hash {
            password::verify(password, hash)
        } else {
            password::verify_dummy(password);
            false
        };

        let reason = if row.status == UserStatus::Disabled {
            Some("disabled")
        } else if row.locked_until.is_some_and(|until| until > now) {
            Some("locked")
        } else if !password_ok {
            Some("bad_password")
        } else {
            None
        };

        if let Some(reason) = reason {
            self.record_failure(&mut tx, ctx, &row, email, reason, now)
                .await?;
            tx.commit().await?;
            return Err(AuthError::InvalidCredentials);
        }

        let (session_id, token, absolute_expires_at) =
            self.start_session(&mut tx, ctx, row.id, now).await?;

        let user = find_user(&mut tx, row.id)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        tx.commit().await?;

        Ok(LoginSuccess {
            user,
            session_id,
            token,
            expires_at: absolute_expires_at,
        })
    }

    /// Records a failed attempt. Only wrong passwords count toward lockout, so
    /// a locked account's lock isn't extended forever by repeated attempts.
    async fn record_failure(
        &self,
        conn: &mut PgConnection,
        ctx: &RequestContext,
        row: &LoginRow,
        email: &str,
        reason: &str,
        now: OffsetDateTime,
    ) -> AuthResult<()> {
        if reason == "bad_password" {
            let failures = row.failed_login_count + 1;
            let locked_until = self.config.lockout_for(failures).map(|d| now + d);
            sqlx::query(
                "UPDATE users SET failed_login_count = $2, \
                 locked_until = coalesce($3, locked_until) WHERE id = $1",
            )
            .bind(row.id)
            .bind(failures)
            .bind(locked_until)
            .execute(&mut *conn)
            .await?;
            if let Some(until) = locked_until {
                audit::record(
                    conn,
                    ctx,
                    AuditRecord::new("auth.account_locked")
                        .target("user", row.id)
                        .details(json!({ "failures": failures, "until": until.unix_timestamp() })),
                )
                .await?;
            }
        }
        self.audit_failure(conn, ctx, Some(row.id), email, reason)
            .await
    }

    /// Resets failure counters and creates a session. Returns (id, token, expiry).
    async fn start_session(
        &self,
        conn: &mut PgConnection,
        ctx: &RequestContext,
        user_id: Uuid,
        now: OffsetDateTime,
    ) -> AuthResult<(Uuid, String, OffsetDateTime)> {
        sqlx::query(
            "UPDATE users SET failed_login_count = 0, locked_until = NULL, last_login_at = now() \
             WHERE id = $1",
        )
        .bind(user_id)
        .execute(&mut *conn)
        .await?;

        let token = token::generate(token::SESSION_PREFIX);
        let session_id = huxley_core::new_id();
        let absolute_expires_at = now + self.config.absolute();
        let idle_expires_at = (now + self.config.idle()).min(absolute_expires_at);
        sqlx::query(
            "INSERT INTO sessions \
                (id, user_id, token_hash, idle_expires_at, absolute_expires_at, ip, user_agent) \
             VALUES ($1, $2, $3, $4, $5, $6::inet, $7)",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(&token.hash)
        .bind(idle_expires_at)
        .bind(absolute_expires_at)
        .bind(ctx.ip.map(|ip| ip.to_string()))
        .bind(ctx.user_agent_truncated())
        .execute(&mut *conn)
        .await?;

        audit::record(
            conn,
            &ctx.clone().with_actor(Actor::User(user_id)),
            AuditRecord::new("auth.login_succeeded")
                .target("session", session_id)
                .details(json!({ "method": "password" })),
        )
        .await?;
        Ok((session_id, token.secret, absolute_expires_at))
    }

    async fn audit_failure(
        &self,
        conn: &mut PgConnection,
        ctx: &RequestContext,
        user_id: Option<Uuid>,
        email: &str,
        reason: &str,
    ) -> AuthResult<()> {
        let mut record = AuditRecord::new("auth.login_failed").details(json!({
            "reason": reason,
            // Truncated so an attacker can't stuff arbitrary data into the log.
            "email": email.chars().take(320).collect::<String>(),
        }));
        if let Some(id) = user_id {
            record = record.target("user", id);
        }
        audit::record(conn, &ctx.clone().with_actor(Actor::Anonymous), record).await?;
        Ok(())
    }

    /// Resolves a session token to its user, extending the idle timeout.
    /// Returns `None` for unknown, expired, or revoked sessions and disabled users.
    pub async fn authenticate(&self, secret: &str) -> AuthResult<Option<Authenticated>> {
        if !token::looks_valid(secret, token::SESSION_PREFIX) {
            return Ok(None);
        }
        let hash = token::hash(secret);
        let row: Option<(Uuid, Uuid, Option<OffsetDateTime>, OffsetDateTime)> = sqlx::query_as(
            "SELECT s.id, s.user_id, s.last_seen_at, s.absolute_expires_at \
               FROM sessions s JOIN users u ON u.id = s.user_id \
              WHERE s.token_hash = $1 AND s.revoked_at IS NULL \
                AND s.idle_expires_at > now() AND s.absolute_expires_at > now() \
                AND u.status = 'active'",
        )
        .bind(&hash)
        .fetch_optional(&self.db)
        .await?;
        let Some((session_id, user_id, last_seen_at, absolute_expires_at)) = row else {
            return Ok(None);
        };

        let now = OffsetDateTime::now_utc();
        if last_seen_at.is_none_or(|seen| now - seen > TOUCH_INTERVAL) {
            let idle_expires_at = (now + self.config.idle()).min(absolute_expires_at);
            sqlx::query(
                "UPDATE sessions SET last_seen_at = $2, idle_expires_at = $3 WHERE id = $1",
            )
            .bind(session_id)
            .bind(now)
            .bind(idle_expires_at)
            .execute(&self.db)
            .await?;
        }

        let mut conn = self.db.acquire().await?;
        Ok(find_user(&mut conn, user_id)
            .await?
            .map(|user| Authenticated { session_id, user }))
    }

    /// Ends the current session.
    pub async fn logout(&self, ctx: &RequestContext, session_id: Uuid) -> AuthResult<()> {
        self.revoke(ctx, session_id, "auth.logout").await
    }

    /// Changes the user's password after checking the current one. Every
    /// other session of the user is revoked; `keep_session` stays signed in.
    pub async fn change_password(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        keep_session: Uuid,
        current: &str,
        new: &str,
    ) -> AuthResult<()> {
        password::validate(new)?;
        let mut tx = self.db.begin().await?;
        let row: Option<(String,)> =
            sqlx::query_as("SELECT hash FROM user_passwords WHERE user_id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let current_ok = match &row {
            Some((hash,)) => password::verify(current, hash),
            None => false,
        };
        if !current_ok {
            audit::record(
                &mut tx,
                ctx,
                AuditRecord::new("auth.password_change_failed").target("user", user_id),
            )
            .await?;
            tx.commit().await?;
            return Err(AuthError::WrongCurrentPassword);
        }
        sqlx::query(
            "INSERT INTO user_passwords (user_id, hash) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET hash = excluded.hash, updated_at = now()",
        )
        .bind(user_id)
        .bind(password::hash(new))
        .execute(&mut *tx)
        .await?;
        let revoked = sqlx::query(
            "UPDATE sessions SET revoked_at = now() \
             WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(keep_session)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.password_changed")
                .target("user", user_id)
                .details(json!({ "otherSessionsRevoked": revoked })),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The user's active sessions, newest first.
    pub async fn list_sessions(&self, user_id: Uuid) -> AuthResult<Vec<Session>> {
        Ok(sqlx::query_as(
            "SELECT id, user_id, created_at, last_seen_at, absolute_expires_at, \
                    host(ip) AS ip, user_agent \
               FROM sessions \
              WHERE user_id = $1 AND revoked_at IS NULL \
                AND idle_expires_at > now() AND absolute_expires_at > now() \
              ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await?)
    }

    /// Revokes one of the user's own sessions (e.g. "sign out that device").
    pub async fn revoke_own_session(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        session_id: Uuid,
    ) -> AuthResult<()> {
        let owner: Option<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM sessions WHERE id = $1 AND revoked_at IS NULL")
                .bind(session_id)
                .fetch_optional(&self.db)
                .await?;
        match owner {
            Some((owner,)) if owner == user_id => {
                self.revoke(ctx, session_id, "auth.session_revoked").await
            }
            _ => Err(AuthError::SessionNotFound),
        }
    }

    async fn revoke(&self, ctx: &RequestContext, session_id: Uuid, action: &str) -> AuthResult<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new(action).target("session", session_id),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockout_backs_off_exponentially_and_caps() {
        let config = AuthConfig::default();
        assert_eq!(config.lockout_for(4), None);
        assert_eq!(config.lockout_for(5), Some(Duration::seconds(60)));
        assert_eq!(config.lockout_for(6), Some(Duration::seconds(120)));
        assert_eq!(config.lockout_for(7), Some(Duration::seconds(240)));
        assert_eq!(config.lockout_for(50), Some(Duration::seconds(3600)));
    }

    #[test]
    fn emails_are_validated() {
        assert_eq!(
            normalize_email("  a@example.com ").unwrap(),
            "a@example.com"
        );
        assert!(normalize_email("no-at-sign").is_err());
        assert!(normalize_email("@example.com").is_err());
        assert!(normalize_email("a@localhost").is_err());
        assert!(normalize_email("a b@example.com").is_err());
    }

    #[test]
    fn display_names_are_validated() {
        assert_eq!(normalize_display_name(" Ada ").unwrap(), "Ada");
        assert!(normalize_display_name("").is_err());
        assert!(normalize_display_name("a\nb").is_err());
    }
}
