//! Envelope encryption for secrets at rest (design doc §12, ADR-018).
//!
//! - A **key-encryption key** (KEK) comes from configuration (`crypto.kek`, or
//!   `HUXLEY_CRYPTO__KEK[_FILE]`) and never touches the database. Retired KEKs
//!   (`crypto.retired_keks`) can still unwrap old data keys until
//!   `huxley admin rewrap-keys` re-wraps them under the active one. KMS-backed
//!   KEKs (Azure Key Vault, Vault) plug in at [`KeyRing`] later.
//! - Each **provider organization** has its own data-encryption key (DEK),
//!   created on first use and stored wrapped by the KEK in `data_keys`.
//! - Values are sealed with AES-256-GCM under the DEK. The ciphertext names its
//!   DEK, and is bound to a [`Binding`] (what the value is and which record it
//!   belongs to), so a ciphertext copied onto another row fails to decrypt.
//!
//! Ciphertext layout: `0x01 || dek_id (16) || nonce (12) || ciphertext || tag (16)`.

mod keyring;
#[cfg(feature = "testing")]
pub mod testing;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{DbError, OrgScope, begin_scoped};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
use zeroize::Zeroizing;

use keyring::{AeadKey, random_key};
pub use keyring::{KEY_LEN, KeyRing, generate_key_base64};

const FORMAT_V1: u8 = 1;
const HEADER_LEN: usize = 1 + 16;

/// How long a process trusts its cached "active DEK" for an org. After a DEK
/// rotation, other processes keep encrypting with the previous (still valid)
/// key for at most this long.
const ACTIVE_KEY_TTL: Duration = Duration::from_secs(300);

/// Encryption settings (`[crypto]` in `huxley.toml`, or `HUXLEY_CRYPTO__*`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CryptoConfig {
    /// Active key-encryption key: 32 random bytes, base64. Generate one with
    /// `huxley admin generate-kek`. Supports `HUXLEY_CRYPTO__KEK_FILE`.
    pub kek: Option<SecretString>,
    /// Previous KEKs, kept so data keys wrapped by them can still be read
    /// until `huxley admin rewrap-keys` has run.
    pub retired_keks: Vec<SecretString>,
}

impl CryptoConfig {
    pub fn key_ring(&self) -> Result<KeyRing, CryptoError> {
        let kek = self.kek.as_ref().ok_or(CryptoError::MissingKek)?;
        KeyRing::new(kek, &self.retired_keks)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error(
        "no key-encryption key is configured: set HUXLEY_CRYPTO__KEK (or _FILE) to the output of `huxley admin generate-kek`"
    )]
    MissingKek,
    #[error("a configured key-encryption key is not 32 bytes of base64")]
    InvalidKek,
    #[error("invalid key material")]
    InvalidKey,
    #[error(
        "data key is wrapped by key-encryption key {0}, which is not configured (add it to crypto.retired_keks)"
    )]
    UnknownKek(String),
    #[error("decryption failed: wrong key, wrong binding, or corrupted data")]
    Decrypt,
    #[error("data key {0} not found")]
    UnknownDataKey(Uuid),
    #[error("organization {0} not found")]
    UnknownOrg(Uuid),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for CryptoError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(error))
    }
}

pub type CryptoResult<T> = Result<T, CryptoError>;

/// What a ciphertext is and which record it belongs to. Must be identical at
/// encryption and decryption.
#[derive(Debug, Clone, Copy)]
pub struct Binding<'a> {
    /// A stable name for the kind of value, e.g. `"user_totp_secret"`.
    pub purpose: &'a str,
    /// The id of the row that holds the value.
    pub record: Uuid,
}

impl Binding<'_> {
    fn aad(&self, dek_id: Uuid) -> Vec<u8> {
        let mut aad = b"huxley:v1:".to_vec();
        aad.extend_from_slice(dek_id.as_bytes());
        aad.extend_from_slice(self.record.as_bytes());
        aad.extend_from_slice(self.purpose.as_bytes());
        aad
    }
}

#[derive(Debug, Default)]
struct Cache {
    /// Unwrapped DEKs by id. Small: one or a few per provider org.
    keys: HashMap<Uuid, Arc<AeadKey>>,
    /// Active DEK id per org (any org, resolved to its provider's key).
    active: HashMap<Uuid, (Uuid, Instant)>,
}

/// Encrypts and decrypts secrets. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Crypto {
    db: PgPool,
    ring: Arc<KeyRing>,
    cache: Arc<Mutex<Cache>>,
}

#[derive(sqlx::FromRow)]
struct DataKeyRow {
    id: Uuid,
    org_id: Uuid,
    kek_id: String,
    wrapped_key: Vec<u8>,
}

impl Crypto {
    pub fn new(db: PgPool, ring: KeyRing) -> Self {
        Self {
            db,
            ring: Arc::new(ring),
            cache: Arc::default(),
        }
    }

    /// Encrypts `plaintext` under the DEK of `org_id`'s provider organization
    /// (the org itself, or its nearest provider ancestor).
    pub async fn encrypt(
        &self,
        org_id: Uuid,
        binding: Binding<'_>,
        plaintext: &[u8],
    ) -> CryptoResult<Vec<u8>> {
        let (dek_id, key) = self.active_key(org_id).await?;
        let mut out = Vec::with_capacity(HEADER_LEN + plaintext.len() + 28);
        out.push(FORMAT_V1);
        out.extend_from_slice(dek_id.as_bytes());
        out.extend_from_slice(&key.seal(&binding.aad(dek_id), plaintext));
        Ok(out)
    }

    /// Decrypts a value produced by [`Crypto::encrypt`] with the same binding.
    pub async fn decrypt(
        &self,
        binding: Binding<'_>,
        ciphertext: &[u8],
    ) -> CryptoResult<Zeroizing<Vec<u8>>> {
        if ciphertext.len() < HEADER_LEN || ciphertext[0] != FORMAT_V1 {
            return Err(CryptoError::Decrypt);
        }
        let dek_id =
            Uuid::from_slice(&ciphertext[1..HEADER_LEN]).map_err(|_| CryptoError::Decrypt)?;
        let key = self.key_by_id(dek_id).await?;
        key.open(&binding.aad(dek_id), &ciphertext[HEADER_LEN..])
    }

    /// Checks that every stored data key can be unwrapped with the configured
    /// KEKs. Run at startup so a missing KEK fails fast instead of on first use.
    pub async fn verify_keys(&self) -> CryptoResult<()> {
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        let rows: Vec<DataKeyRow> =
            sqlx::query_as("SELECT id, org_id, kek_id, wrapped_key FROM data_keys")
                .fetch_all(&mut *tx)
                .await?;
        tx.commit().await?;
        for row in rows {
            self.ring
                .unwrap(&row.kek_id, row.id, row.org_id, &row.wrapped_key)?;
        }
        Ok(())
    }

    /// Re-wraps every data key (or only `org_id`'s) not already under the
    /// active KEK, all or nothing. Returns how many were re-wrapped. Afterwards
    /// the retired KEKs can be removed from configuration.
    pub async fn rewrap(&self, ctx: &RequestContext, org_id: Option<Uuid>) -> CryptoResult<u64> {
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        let rows: Vec<DataKeyRow> = sqlx::query_as(
            "SELECT id, org_id, kek_id, wrapped_key FROM data_keys \
              WHERE kek_id <> $1 AND ($2::uuid IS NULL OR org_id = $2) \
              ORDER BY id FOR UPDATE",
        )
        .bind(self.ring.active_id())
        .bind(org_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut from: Vec<String> = Vec::new();
        for row in &rows {
            let dek = self
                .ring
                .unwrap(&row.kek_id, row.id, row.org_id, &row.wrapped_key)?;
            let (kek_id, wrapped) = self.ring.wrap(row.id, row.org_id, &dek);
            sqlx::query(
                "UPDATE data_keys SET kek_id = $2, wrapped_key = $3, rewrapped_at = now() \
                 WHERE id = $1",
            )
            .bind(row.id)
            .bind(kek_id)
            .bind(wrapped)
            .execute(&mut *tx)
            .await?;
            if !from.contains(&row.kek_id) {
                from.push(row.kek_id.clone());
            }
        }
        let count = rows.len() as u64;
        if count > 0 {
            audit::record(
                &mut tx,
                ctx,
                AuditRecord::new("crypto.keys_rewrapped").details(json!({
                    "count": count,
                    "fromKeks": from,
                    "toKek": self.ring.active_id(),
                })),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(count)
    }

    async fn key_by_id(&self, dek_id: Uuid) -> CryptoResult<Arc<AeadKey>> {
        if let Some(key) = self.cache.lock().expect("cache lock").keys.get(&dek_id) {
            return Ok(key.clone());
        }
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        let row: Option<DataKeyRow> =
            sqlx::query_as("SELECT id, org_id, kek_id, wrapped_key FROM data_keys WHERE id = $1")
                .bind(dek_id)
                .fetch_optional(&mut *tx)
                .await?;
        tx.commit().await?;
        let row = row.ok_or(CryptoError::UnknownDataKey(dek_id))?;
        self.load(&row)
    }

    fn load(&self, row: &DataKeyRow) -> CryptoResult<Arc<AeadKey>> {
        let dek = self
            .ring
            .unwrap(&row.kek_id, row.id, row.org_id, &row.wrapped_key)?;
        let key = Arc::new(AeadKey::new(&dek)?);
        self.cache
            .lock()
            .expect("cache lock")
            .keys
            .insert(row.id, key.clone());
        Ok(key)
    }

    /// The active DEK for `org_id`'s provider org, creating it on first use.
    async fn active_key(&self, org_id: Uuid) -> CryptoResult<(Uuid, Arc<AeadKey>)> {
        let cached = {
            let cache = self.cache.lock().expect("cache lock");
            cache
                .active
                .get(&org_id)
                .filter(|(_, at)| at.elapsed() < ACTIVE_KEY_TTL)
                .and_then(|(id, _)| cache.keys.get(id).map(|k| (*id, k.clone())))
        };
        if let Some(hit) = cached {
            return Ok(hit);
        }

        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        let provider: Option<(Uuid,)> = sqlx::query_as(
            "SELECT a.id FROM organizations o \
               JOIN organizations a ON a.path @> o.path AND a.kind = 'provider' \
              WHERE o.id = $1 ORDER BY nlevel(a.path) DESC LIMIT 1",
        )
        .bind(org_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (provider,) = provider.ok_or(CryptoError::UnknownOrg(org_id))?;

        let select = "SELECT id, org_id, kek_id, wrapped_key FROM data_keys \
                      WHERE org_id = $1 AND status = 'active'";
        let mut row: Option<DataKeyRow> = sqlx::query_as(select)
            .bind(provider)
            .fetch_optional(&mut *tx)
            .await?;
        if row.is_none() {
            let id = huxley_core::new_id();
            let dek = random_key();
            let (kek_id, wrapped) = self.ring.wrap(id, provider, &*dek);
            let inserted = sqlx::query(
                "INSERT INTO data_keys (id, org_id, kek_id, wrapped_key) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (org_id) WHERE status = 'active' DO NOTHING",
            )
            .bind(id)
            .bind(provider)
            .bind(&kek_id)
            .bind(&wrapped)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if inserted == 1 {
                audit::record(
                    &mut tx,
                    &RequestContext::system(Via::System),
                    AuditRecord::new("crypto.data_key_created")
                        .org(provider)
                        .target("data_key", id)
                        .details(json!({ "kek": kek_id })),
                )
                .await?;
            }
            // Either ours, or one a concurrent caller created first.
            row = sqlx::query_as(select)
                .bind(provider)
                .fetch_optional(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        let row = row.ok_or(CryptoError::UnknownOrg(provider))?;
        let key = self.load(&row)?;
        self.cache
            .lock()
            .expect("cache lock")
            .active
            .insert(org_id, (row.id, Instant::now()));
        Ok((row.id, key))
    }
}
