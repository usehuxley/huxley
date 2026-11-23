//! The runner registry (ADR-025): the instance CA (its key sealed with the
//! root organization's data key), enrolment tokens, and the runners. Run
//! as the system: runners belong to the instance. Callers check
//! `platform:runners:manage` for the admin operations.

use huxley_core::RequestContext;
use huxley_crypto::{Binding, Crypto, CryptoError};
use huxley_db::audit::{self, AuditRecord};
use ring::digest::{SHA256, digest};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;
use x509_parser::prelude::{FromDer, X509Certificate};
use zeroize::Zeroizing;

use crate::ca::{self, Ca, CaError};

/// How long an enrolment token works.
pub const ENROLMENT_HOURS: i64 = 24;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("runner not found")]
    NotFound,
    #[error("this enrolment token is unknown, used or expired")]
    BadToken,
    #[error("a runner is already called {0:?}")]
    NameTaken(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the instance has no root organization yet")]
    NoRoot,
    #[error(transparent)]
    Ca(#[from] CaError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type RegistryResult<T> = Result<T, RegistryError>;

fn db(e: huxley_db::DbError) -> RegistryError {
    match e {
        huxley_db::DbError::Sqlx(e) => RegistryError::Db(e),
        other => RegistryError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "runner_source", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RunnerSource {
    /// Enrolled with a token; Huxley issues its certificates.
    Huxley,
    /// Uses a certificate from the operator's own CA.
    External,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RunnerView {
    pub id: Uuid,
    pub name: String,
    pub source: RunnerSource,
    pub languages: Vec<String>,
    pub version: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub cert_expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const VIEW: &str = "id, name, source, languages, version, cert_expires_at, last_seen_at, \
                    revoked_at, created_at";

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

fn hash(token: &str) -> Vec<u8> {
    digest(&SHA256, token.as_bytes()).as_ref().to_vec()
}

const CA_PURPOSE: &str = "runner_ca_key";

/// The instance's runner CA, made on first use.
pub async fn ca(conn: &mut PgConnection, crypto: &Crypto) -> RegistryResult<Ca> {
    let existing: Option<(String, Vec<u8>, Uuid)> =
        sqlx::query_as("SELECT cert_pem, key_sealed, org_id FROM runner_ca")
            .fetch_optional(&mut *conn)
            .await?;
    if let Some((cert_pem, sealed, org_id)) = existing {
        let binding = Binding {
            purpose: CA_PURPOSE,
            record: org_id,
        };
        let key = Zeroizing::new(crypto.decrypt(binding, &sealed).await?);
        let key = std::str::from_utf8(&key)
            .map_err(|_| RegistryError::Invalid("the CA key isn't text".into()))?;
        return Ok(Ca::load(&cert_pem, key)?);
    }
    let root: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE parent_id IS NULL")
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(RegistryError::NoRoot)?;
    let new = ca::create()?;
    let binding = Binding {
        purpose: CA_PURPOSE,
        record: root,
    };
    let sealed = crypto
        .encrypt(
            root,
            binding,
            Zeroizing::new(new.key_pem.clone()).as_bytes(),
        )
        .await?;
    // Two processes may race to make it: the first one wins, both use it.
    sqlx::query(
        "INSERT INTO runner_ca (org_id, cert_pem, key_sealed) VALUES ($1, $2, $3) \
         ON CONFLICT (singleton) DO NOTHING",
    )
    .bind(root)
    .bind(&new.cert_pem)
    .bind(&sealed)
    .execute(&mut *conn)
    .await?;
    Box::pin(ca(conn, crypto)).await
}

/// A single-use enrolment token for a runner to be called `name`. The
/// token is shown once.
pub async fn create_enrollment(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    name: &str,
) -> RegistryResult<(String, OffsetDateTime)> {
    if !valid_name(name) {
        return Err(RegistryError::Invalid(
            "a runner name is lowercase letters, digits, dots and hyphens".into(),
        ));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM runners WHERE name = $1)")
        .bind(name)
        .fetch_one(&mut *conn)
        .await?;
    if exists {
        return Err(RegistryError::NameTaken(name.to_owned()));
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random source works");
    let token = format!(
        "hxr_{}",
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
    );
    let expires_at = OffsetDateTime::now_utc() + Duration::hours(ENROLMENT_HOURS);
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO runner_enrollments (id, token_hash, name, expires_at, created_by) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(hash(&token))
    .bind(name)
    .bind(expires_at)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("runner.enrollment_created")
            .target("runner_enrollment", id)
            .details(json!({ "name": name })),
    )
    .await
    .map_err(db)?;
    Ok((token, expires_at))
}

/// What a runner gets when it enrols.
#[derive(Debug, Clone)]
pub struct Enrolled {
    pub runner: RunnerView,
    pub certificate_pem: String,
    pub ca_pem: String,
}

fn serial_of(cert_pem: &str) -> RegistryResult<String> {
    let der = pem_der(cert_pem)?;
    let (_, cert) = X509Certificate::from_der(&der)
        .map_err(|e| RegistryError::Invalid(format!("issued certificate: {e}")))?;
    Ok(cert.raw_serial_as_string())
}

fn pem_der(pem: &str) -> RegistryResult<Vec<u8>> {
    let parsed = x509_parser::pem::parse_x509_pem(pem.as_bytes())
        .map_err(|e| RegistryError::Invalid(format!("not a PEM certificate: {e}")))?;
    Ok(parsed.1.contents)
}

/// Uses an enrolment token: registers the runner and issues its first
/// certificate for the key in `csr_pem`.
pub async fn enroll(
    conn: &mut PgConnection,
    crypto: &Crypto,
    token: &str,
    csr_pem: &str,
) -> RegistryResult<Enrolled> {
    let found: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM runner_enrollments \
          WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() FOR UPDATE",
    )
    .bind(hash(token))
    .fetch_optional(&mut *conn)
    .await?;
    let (enrollment, name) = found.ok_or(RegistryError::BadToken)?;
    let ca = ca(&mut *conn, crypto).await?;
    let id = huxley_core::new_id();
    let issued = ca.issue_runner(csr_pem, &id.to_string())?;
    let inserted = sqlx::query(
        "INSERT INTO runners (id, name, source, cert_serial, cert_expires_at) \
         VALUES ($1, $2, 'huxley', $3, $4) ON CONFLICT (name) DO NOTHING",
    )
    .bind(id)
    .bind(&name)
    .bind(serial_of(&issued.cert_pem)?)
    .bind(issued.expires_at)
    .execute(&mut *conn)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(RegistryError::NameTaken(name));
    }
    sqlx::query("UPDATE runner_enrollments SET used_at = now(), runner_id = $2 WHERE id = $1")
        .bind(enrollment)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        &RequestContext::system(huxley_core::Via::System),
        AuditRecord::new("runner.enrolled")
            .target("runner", id)
            .details(json!({ "name": name })),
    )
    .await
    .map_err(db)?;
    Ok(Enrolled {
        runner: get(conn, id).await?,
        certificate_pem: issued.cert_pem,
        ca_pem: ca.cert_pem,
    })
}

/// A new certificate for a Huxley-enrolled runner's new key. Older
/// certificates stop working.
pub async fn renew(
    conn: &mut PgConnection,
    crypto: &Crypto,
    runner_id: Uuid,
    csr_pem: &str,
) -> RegistryResult<ca::Issued> {
    let runner = get(&mut *conn, runner_id).await?;
    if runner.source != RunnerSource::Huxley || runner.revoked_at.is_some() {
        return Err(RegistryError::Invalid(
            "only active runners Huxley enrolled renew here".into(),
        ));
    }
    let ca = ca(&mut *conn, crypto).await?;
    let issued = ca.issue_runner(csr_pem, &runner_id.to_string())?;
    sqlx::query("UPDATE runners SET cert_serial = $2, cert_expires_at = $3 WHERE id = $1")
        .bind(runner_id)
        .bind(serial_of(&issued.cert_pem)?)
        .bind(issued.expires_at)
        .execute(conn)
        .await?;
    Ok(issued)
}

/// Registers a runner that uses a certificate from the operator's CA: its
/// certificate's common name must be `name`.
pub async fn register_external(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    name: &str,
) -> RegistryResult<RunnerView> {
    if !valid_name(name) {
        return Err(RegistryError::Invalid(
            "a runner name is lowercase letters, digits, dots and hyphens".into(),
        ));
    }
    let id = huxley_core::new_id();
    let inserted = sqlx::query(
        "INSERT INTO runners (id, name, source, created_by) VALUES ($1, $2, 'external', $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(id)
    .bind(name)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(RegistryError::NameTaken(name.to_owned()));
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("runner.registered")
            .target("runner", id)
            .details(json!({ "name": name })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

/// Stops a runner at once: the gateway refuses its certificate.
pub async fn revoke(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> RegistryResult<RunnerView> {
    let updated =
        sqlx::query("UPDATE runners SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
    if updated.rows_affected() == 0 {
        return Err(RegistryError::NotFound);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("runner.revoked").target("runner", id),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> RegistryResult<RunnerView> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM runners WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(RegistryError::NotFound)
}

pub async fn list(conn: &mut PgConnection) -> RegistryResult<Vec<RunnerView>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VIEW} FROM runners ORDER BY revoked_at IS NOT NULL, name"
    )))
    .fetch_all(conn)
    .await?)
}

/// Who a certificate (already verified by TLS against the trusted CAs)
/// belongs to: a Huxley runner whose current serial it carries, or an
/// external runner registered under its common name. `None` for anything
/// else, including revoked runners.
pub async fn identify(
    conn: &mut PgConnection,
    cert_der: &[u8],
) -> RegistryResult<Option<RunnerView>> {
    let Ok((_, cert)) = X509Certificate::from_der(cert_der) else {
        return Ok(None);
    };
    let Some(cn) = cert
        .subject()
        .iter_common_name()
        .next()
        .and_then(|c| c.as_str().ok())
    else {
        return Ok(None);
    };
    let serial = cert.raw_serial_as_string();
    let found: Option<Uuid> = match cn.parse::<Uuid>() {
        // Huxley names its runners by id, and only the latest certificate
        // it issued counts.
        Ok(id) => {
            sqlx::query_scalar(
                "SELECT id FROM runners WHERE id = $1 AND source = 'huxley' AND cert_serial = $2 \
               AND revoked_at IS NULL",
            )
            .bind(id)
            .bind(&serial)
            .fetch_optional(&mut *conn)
            .await?
        }
        Err(_) => sqlx::query_scalar(
            "SELECT id FROM runners WHERE name = $1 AND source = 'external' AND revoked_at IS NULL",
        )
        .bind(cn)
        .fetch_optional(&mut *conn)
        .await?,
    };
    match found {
        Some(id) => Ok(Some(get(conn, id).await?)),
        None => Ok(None),
    }
}

/// Records a heartbeat: when the runner was last seen, and what it runs.
pub async fn seen(
    conn: &mut PgConnection,
    id: Uuid,
    languages: &[String],
    version: &str,
) -> RegistryResult<()> {
    sqlx::query(
        "UPDATE runners SET last_seen_at = now(), languages = $2, version = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(languages)
    .bind(version)
    .execute(conn)
    .await?;
    Ok(())
}
