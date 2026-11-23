//! Passkeys (WebAuthn) via `webauthn-rs`: registration, use as a second factor
//! after the password, and passwordless sign-in (design doc §11.2).
//!
//! Every ceremony has two halves. The first returns options for the browser's
//! `navigator.credentials` call and stores the server-side state (which holds
//! the challenge) in `webauthn_ceremonies`; the second consumes that row, so a
//! response can be used once, and only before it expires.
//!
//! Passkeys always require user verification (a PIN or biometric on the
//! device), so a passkey sign-in is multi-factor on its own.

use huxley_core::{Actor, RequestContext};
use huxley_db::audit::{self, AuditRecord};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use webauthn_rs::prelude::{
    DiscoverableAuthentication, Passkey, PasskeyAuthentication, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, Url, Webauthn, WebauthnBuilder,
};

use crate::{AuthConfig, AuthError, AuthResult, AuthService, LoginSuccess, User, UserStatus, mfa};

/// How long the browser has to answer a ceremony.
const CEREMONY_TTL: Duration = Duration::minutes(5);
/// Expired ceremonies and challenges are swept this long after expiry.
const SWEEP_AFTER: Duration = Duration::hours(1);
/// The relying party name shown by some authenticators.
const RP_NAME: &str = "Huxley";

/// Builds the WebAuthn relying party from `auth.public_url`, or `None` when
/// passkeys aren't configured.
pub(crate) fn relying_party(config: &AuthConfig) -> AuthResult<Option<Webauthn>> {
    let Some(public_url) = config.public_url.as_deref() else {
        return Ok(None);
    };
    let origin = Url::parse(public_url).map_err(|_| AuthError::InvalidPublicUrl)?;
    let rp_id = origin
        .host_str()
        .ok_or(AuthError::InvalidPublicUrl)?
        .to_owned();
    let webauthn = WebauthnBuilder::new(&rp_id, &origin)
        .and_then(|b| {
            b.rp_name(RP_NAME)
                .timeout(std::time::Duration::from_secs(
                    CEREMONY_TTL.whole_seconds().unsigned_abs(),
                ))
                .build()
        })
        .map_err(|_| AuthError::InvalidPublicUrl)?;
    Ok(Some(webauthn))
}

/// A registered passkey, without its key material.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PasskeyInfo {
    pub id: Uuid,
    pub name: String,
    pub created_at: OffsetDateTime,
    pub last_used_at: Option<OffsetDateTime>,
}

/// The first half of a ceremony: pass `options` to the browser and send
/// `id` back with its answer.
#[derive(Debug)]
pub struct Ceremony {
    pub id: Uuid,
    /// `CreationChallengeResponse` or `RequestChallengeResponse` JSON.
    pub options: Value,
}

/// A new passkey and, if it was the user's first second factor, their
/// recovery codes (shown once).
#[derive(Debug)]
pub struct RegisteredPasskey {
    pub passkey: PasskeyInfo,
    pub recovery_codes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "webauthn_ceremony_kind", rename_all = "lowercase")]
enum CeremonyKind {
    Registration,
    Mfa,
    Login,
}

fn normalize_name(name: &str) -> AuthResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(AuthError::InvalidPasskeyName);
    }
    Ok(name.to_owned())
}

fn to_json<T: Serialize>(value: &T) -> AuthResult<Value> {
    serde_json::to_value(value).map_err(|_| AuthError::InvalidPasskey)
}

fn from_json<T: DeserializeOwned>(value: Value) -> AuthResult<T> {
    serde_json::from_value(value).map_err(|_| AuthError::InvalidPasskey)
}

/// The user's passkeys as webauthn-rs values, with their row ids.
async fn load_passkeys(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<Vec<(Uuid, Passkey)>> {
    let rows: Vec<(Uuid, Value)> =
        sqlx::query_as("SELECT id, passkey FROM user_passkeys WHERE user_id = $1 ORDER BY id")
            .bind(user_id)
            .fetch_all(conn)
            .await?;
    rows.into_iter()
        .map(|(id, value)| Ok((id, from_json(value)?)))
        .collect()
}

pub(crate) async fn has_passkeys(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<bool> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT true FROM user_passkeys WHERE user_id = $1 LIMIT 1")
            .bind(user_id)
            .fetch_optional(conn)
            .await?;
    Ok(row.is_some())
}

pub(crate) async fn passkey_count(conn: &mut PgConnection, user_id: Uuid) -> AuthResult<i64> {
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_passkeys WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(conn)
        .await?;
    Ok(count)
}

async fn store_ceremony(
    conn: &mut PgConnection,
    kind: CeremonyKind,
    user_id: Option<Uuid>,
    mfa_challenge_id: Option<Uuid>,
    state: Value,
) -> AuthResult<Uuid> {
    sqlx::query("DELETE FROM webauthn_ceremonies WHERE expires_at < $1")
        .bind(OffsetDateTime::now_utc() - SWEEP_AFTER)
        .execute(&mut *conn)
        .await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO webauthn_ceremonies (id, kind, user_id, mfa_challenge_id, state, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(kind)
    .bind(user_id)
    .bind(mfa_challenge_id)
    .bind(state)
    .bind(OffsetDateTime::now_utc() + CEREMONY_TTL)
    .execute(conn)
    .await?;
    Ok(id)
}

/// Consumes a ceremony. `user_id` and `mfa_challenge_id` must match what it
/// was started for. Returns its state, or `InvalidCeremony`.
async fn take_ceremony<T: DeserializeOwned>(
    conn: &mut PgConnection,
    id: Uuid,
    kind: CeremonyKind,
    user_id: Option<Uuid>,
    mfa_challenge_id: Option<Uuid>,
) -> AuthResult<T> {
    let row: Option<(Value,)> = sqlx::query_as(
        "DELETE FROM webauthn_ceremonies \
          WHERE id = $1 AND kind = $2 AND expires_at > now() \
            AND user_id IS NOT DISTINCT FROM $3 AND mfa_challenge_id IS NOT DISTINCT FROM $4 \
         RETURNING state",
    )
    .bind(id)
    .bind(kind)
    .bind(user_id)
    .bind(mfa_challenge_id)
    .fetch_optional(conn)
    .await?;
    let (state,) = row.ok_or(AuthError::InvalidCeremony)?;
    from_json(state)
}

/// Stores an updated passkey (signature counter, backup state) after use.
async fn record_use(
    conn: &mut PgConnection,
    row_id: Uuid,
    passkey: &mut Passkey,
    result: &webauthn_rs::prelude::AuthenticationResult,
) -> AuthResult<()> {
    let changed = passkey.update_credential(result).unwrap_or(false);
    let value = changed.then(|| to_json(passkey)).transpose()?;
    sqlx::query(
        "UPDATE user_passkeys SET last_used_at = now(), passkey = coalesce($2, passkey) WHERE id = $1",
    )
    .bind(row_id)
    .bind(value)
    .execute(conn)
    .await?;
    Ok(())
}

impl AuthService {
    fn webauthn(&self) -> AuthResult<&Webauthn> {
        self.webauthn
            .as_deref()
            .ok_or(AuthError::PasskeysNotConfigured)
    }

    /// Whether `auth.public_url` is set, so passkeys can be used.
    pub fn passkeys_enabled(&self) -> bool {
        self.webauthn.is_some()
    }

    pub async fn list_passkeys(&self, user_id: Uuid) -> AuthResult<Vec<PasskeyInfo>> {
        Ok(sqlx::query_as(
            "SELECT id, name, created_at, last_used_at FROM user_passkeys \
              WHERE user_id = $1 ORDER BY created_at",
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await?)
    }

    /// Starts adding a passkey after re-checking the password.
    pub async fn begin_passkey_registration(
        &self,
        ctx: &RequestContext,
        user: &User,
        password: &str,
    ) -> AuthResult<Ceremony> {
        let webauthn = self.webauthn()?;
        let mut tx = self.db.begin().await?;
        self.reauthenticate(&mut tx, ctx, user.id, password).await?;
        let existing = load_passkeys(&mut tx, user.id).await?;
        let exclude = existing.iter().map(|(_, p)| p.cred_id().clone()).collect();
        let (options, state): (_, PasskeyRegistration) = webauthn
            .start_passkey_registration(user.id, &user.email, &user.display_name, Some(exclude))
            .map_err(|_| AuthError::InvalidPasskey)?;
        let mut options = to_json(&options)?;
        // Require a discoverable credential so every passkey also works for
        // passwordless sign-in (as passkeys.dev recommends). Platform
        // authenticators and current security keys all support it.
        if let Some(selection) = options.pointer_mut("/publicKey/authenticatorSelection") {
            selection["residentKey"] = json!("required");
            selection["requireResidentKey"] = json!(true);
        }
        let id = store_ceremony(
            &mut tx,
            CeremonyKind::Registration,
            Some(user.id),
            None,
            to_json(&state)?,
        )
        .await?;
        tx.commit().await?;
        Ok(Ceremony { id, options })
    }

    /// Finishes adding a passkey with the browser's answer.
    pub async fn finish_passkey_registration(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        ceremony: Uuid,
        name: &str,
        credential: &RegisterPublicKeyCredential,
    ) -> AuthResult<RegisteredPasskey> {
        let webauthn = self.webauthn()?;
        let name = normalize_name(name)?;
        let mut tx = self.db.begin().await?;
        let state: PasskeyRegistration = take_ceremony(
            &mut tx,
            ceremony,
            CeremonyKind::Registration,
            Some(user_id),
            None,
        )
        .await?;
        let passkey = webauthn
            .finish_passkey_registration(credential, &state)
            .map_err(|_| AuthError::InvalidPasskey)?;
        let first_factor = !mfa::has_mfa(&mut tx, user_id).await?;
        let id = huxley_core::new_id();
        let info: PasskeyInfo = sqlx::query_as(
            "INSERT INTO user_passkeys (id, user_id, credential_id, name, passkey) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id, name, created_at, last_used_at",
        )
        .bind(id)
        .bind(user_id)
        .bind(AsRef::<[u8]>::as_ref(passkey.cred_id()))
        .bind(&name)
        .bind(to_json(&passkey)?)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match huxley_db::violated_constraint(&e) {
            Some("user_passkeys_credential_id_key") => AuthError::PasskeyAlreadyRegistered,
            _ => AuthError::from(e),
        })?;
        let recovery_codes = if first_factor {
            Some(mfa::replace_recovery_codes(&mut tx, user_id).await?)
        } else {
            None
        };
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.passkey_added")
                .target("user", user_id)
                .details(json!({ "passkey": id, "name": name, "firstFactor": first_factor })),
        )
        .await?;
        tx.commit().await?;
        Ok(RegisteredPasskey {
            passkey: info,
            recovery_codes,
        })
    }

    pub async fn rename_passkey(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        id: Uuid,
        name: &str,
    ) -> AuthResult<PasskeyInfo> {
        let name = normalize_name(name)?;
        let mut tx = self.db.begin().await?;
        let info: PasskeyInfo = sqlx::query_as(
            "UPDATE user_passkeys SET name = $3 WHERE id = $1 AND user_id = $2 \
             RETURNING id, name, created_at, last_used_at",
        )
        .bind(id)
        .bind(user_id)
        .bind(&name)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AuthError::PasskeyNotFound)?;
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.passkey_renamed")
                .target("user", user_id)
                .details(json!({ "passkey": id, "name": name })),
        )
        .await?;
        tx.commit().await?;
        Ok(info)
    }

    /// Removes a passkey. If it was the user's last second factor, their
    /// recovery codes go too.
    pub async fn delete_passkey(
        &self,
        ctx: &RequestContext,
        user_id: Uuid,
        id: Uuid,
    ) -> AuthResult<()> {
        let mut tx = self.db.begin().await?;
        let deleted = sqlx::query("DELETE FROM user_passkeys WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if deleted == 0 {
            return Err(AuthError::PasskeyNotFound);
        }
        let last_factor = !mfa::has_mfa(&mut tx, user_id).await?;
        if last_factor {
            mfa::delete_recovery_codes(&mut tx, user_id).await?;
        }
        audit::record(
            &mut tx,
            ctx,
            AuditRecord::new("auth.passkey_removed")
                .target("user", user_id)
                .details(json!({ "passkey": id, "mfaRemaining": !last_factor })),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Options for answering an MFA challenge with one of the user's passkeys.
    pub async fn begin_mfa_passkey(&self, challenge: &str) -> AuthResult<Ceremony> {
        let webauthn = self.webauthn()?;
        let mut tx = self.db.begin().await?;
        let (challenge_id, user_id) = mfa::find_open_challenge(&mut tx, challenge).await?;
        let passkeys: Vec<Passkey> = load_passkeys(&mut tx, user_id)
            .await?
            .into_iter()
            .map(|(_, p)| p)
            .collect();
        if passkeys.is_empty() {
            return Err(AuthError::MfaNotEnabled);
        }
        let (options, state): (_, PasskeyAuthentication) = webauthn
            .start_passkey_authentication(&passkeys)
            .map_err(|_| AuthError::InvalidPasskey)?;
        let id = store_ceremony(
            &mut tx,
            CeremonyKind::Mfa,
            Some(user_id),
            Some(challenge_id),
            to_json(&state)?,
        )
        .await?;
        tx.commit().await?;
        Ok(Ceremony {
            id,
            options: to_json(&options)?,
        })
    }

    /// Verifies a passkey answer to an MFA challenge. Returns `Ok(false)` for
    /// a wrong or unknown credential so the caller counts the failure.
    pub(crate) async fn check_mfa_passkey(
        &self,
        conn: &mut PgConnection,
        user_id: Uuid,
        challenge_id: Uuid,
        ceremony: Uuid,
        credential: &PublicKeyCredential,
    ) -> AuthResult<bool> {
        let webauthn = self.webauthn()?;
        let state: PasskeyAuthentication = match take_ceremony(
            conn,
            ceremony,
            CeremonyKind::Mfa,
            Some(user_id),
            Some(challenge_id),
        )
        .await
        {
            Ok(state) => state,
            Err(AuthError::InvalidCeremony) => return Ok(false),
            Err(e) => return Err(e),
        };
        let Ok(result) = webauthn.finish_passkey_authentication(credential, &state) else {
            return Ok(false);
        };
        let mut passkeys = load_passkeys(conn, user_id).await?;
        let Some((row_id, passkey)) = passkeys
            .iter_mut()
            .find(|(_, p)| p.cred_id() == result.cred_id())
        else {
            return Ok(false);
        };
        record_use(conn, *row_id, passkey, &result).await?;
        Ok(true)
    }

    /// Options for passwordless sign-in. The browser lets the user pick any
    /// passkey they have for this site.
    pub async fn begin_passkey_login(&self) -> AuthResult<Ceremony> {
        let webauthn = self.webauthn()?;
        let (options, state): (_, DiscoverableAuthentication) = webauthn
            .start_discoverable_authentication()
            .map_err(|_| AuthError::InvalidPasskey)?;
        let mut options = to_json(&options)?;
        // webauthn-rs asks for conditional mediation (autofill). The client
        // decides how to present the prompt, so leave that to it.
        if let Some(object) = options.as_object_mut() {
            object.remove("mediation");
        }
        let mut conn = self.db.acquire().await?;
        let id =
            store_ceremony(&mut conn, CeremonyKind::Login, None, None, to_json(&state)?).await?;
        Ok(Ceremony { id, options })
    }

    /// Finishes passwordless sign-in. Every failure is `InvalidPasskey`, so
    /// responses don't reveal whether an account exists.
    pub async fn finish_passkey_login(
        &self,
        ctx: &RequestContext,
        ceremony: Uuid,
        credential: &PublicKeyCredential,
    ) -> AuthResult<LoginSuccess> {
        let webauthn = self.webauthn()?;
        let mut tx = self.db.begin().await?;
        let state: DiscoverableAuthentication =
            take_ceremony(&mut tx, ceremony, CeremonyKind::Login, None, None)
                .await
                .map_err(|_| AuthError::InvalidPasskey)?;
        let (user_id, cred_id) = webauthn
            .identify_discoverable_authentication(credential)
            .map_err(|_| AuthError::InvalidPasskey)?;
        let row: Option<(Uuid, Value, UserStatus, Option<OffsetDateTime>)> = sqlx::query_as(
            "SELECT p.id, p.passkey, u.status, u.locked_until \
               FROM user_passkeys p JOIN users u ON u.id = p.user_id \
              WHERE p.credential_id = $1 AND p.user_id = $2 \
              FOR UPDATE OF p, u",
        )
        .bind(cred_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let now = OffsetDateTime::now_utc();
        let failure = match &row {
            None => Some("unknown_passkey"),
            Some((_, _, UserStatus::Disabled, _)) => Some("disabled"),
            Some((_, _, _, Some(until))) if *until > now => Some("locked"),
            Some(_) => None,
        };
        let verified = match (&row, failure) {
            (Some((row_id, value, _, _)), None) => {
                let mut passkey: Passkey = from_json(value.clone())?;
                match webauthn.finish_discoverable_authentication_v2(credential, state, &passkey) {
                    Ok(result) => {
                        record_use(&mut tx, *row_id, &mut passkey, &result).await?;
                        Ok(())
                    }
                    Err(_) => Err("bad_signature"),
                }
            }
            (_, Some(reason)) => Err(reason),
            (None, None) => Err("unknown_passkey"),
        };
        if let Err(reason) = verified {
            let mut record = AuditRecord::new("auth.login_failed")
                .details(json!({ "method": "passkey", "reason": reason }));
            if row.is_some() {
                record = record.target("user", user_id);
            }
            audit::record(&mut tx, &ctx.clone().with_actor(Actor::Anonymous), record).await?;
            tx.commit().await?;
            return Err(AuthError::InvalidPasskey);
        }
        let success = self
            .finish_login(&mut tx, ctx, user_id, now, "passkey")
            .await?;
        tx.commit().await?;
        Ok(success)
    }
}
