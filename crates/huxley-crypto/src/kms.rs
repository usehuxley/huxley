//! Key-encryption keys held by a key management service (KMS).
//!
//! Huxley generates each data key itself and asks the KMS to encrypt
//! ("wrap") it; the KMS key never leaves the service. What the KMS encrypts is
//! framed with the data key's id and owning org, and checked on the way back,
//! so a wrapped key copied onto another row is rejected just as it is for a
//! configured KEK. The frame works with any provider, whether or not its API
//! takes associated data.
//!
//! Providers: HashiCorp Vault and OpenBao (Transit secrets engine).

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::CryptoError;

const FRAME_PREFIX: &[u8] = b"huxley-dek:v1:";
const FRAME_HEADER: usize = FRAME_PREFIX.len() + 32;

/// `prefix || dek_id || org_id || dek`: what a KMS is asked to encrypt.
pub(crate) fn frame(dek_id: Uuid, org_id: Uuid, dek: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(FRAME_HEADER + dek.len()));
    out.extend_from_slice(FRAME_PREFIX);
    out.extend_from_slice(dek_id.as_bytes());
    out.extend_from_slice(org_id.as_bytes());
    out.extend_from_slice(dek);
    out
}

/// Checks a decrypted frame belongs to this key and org; returns the DEK.
pub(crate) fn unframe(
    dek_id: Uuid,
    org_id: Uuid,
    plain: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let expected = frame(dek_id, org_id, &[]);
    if plain.len() <= FRAME_HEADER || plain[..FRAME_HEADER] != expected[..] {
        return Err(CryptoError::Decrypt);
    }
    Ok(Zeroizing::new(plain[FRAME_HEADER..].to_vec()))
}

/// Identifiers stored in `data_keys.kek_id` for KMS keys.
fn valid_id(id: &str) -> bool {
    let Some((provider, rest)) = id.split_once(':') else {
        return false;
    };
    !provider.is_empty()
        && provider.bytes().all(|b| b.is_ascii_lowercase())
        && (1..=200).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/@:-".contains(&b))
}

fn kms_error(provider: &str, error: reqwest::Error) -> CryptoError {
    // Without the URL: it can carry a namespace or key name, never a secret,
    // but the message is shorter and the config already says where it points.
    CryptoError::Kms(format!(
        "{provider} request failed: {}",
        error.without_url()
    ))
}

fn http_client(
    provider: &str,
    ca_cert_file: Option<&PathBuf>,
    timeout: Duration,
) -> Result<reqwest::Client, CryptoError> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    if let Some(path) = ca_cert_file {
        let pem = std::fs::read(path).map_err(|e| {
            CryptoError::Config(format!(
                "{provider}: cannot read CA certificate {}: {e}",
                path.display()
            ))
        })?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|_| {
            CryptoError::Config(format!(
                "{provider}: {} is not a PEM certificate bundle",
                path.display()
            ))
        })?;
        builder = builder.tls_certs_merge(certs);
    }
    builder
        .build()
        .map_err(|e| CryptoError::Config(format!("{provider}: cannot build HTTP client: {e}")))
}

fn default_transit_mount() -> String {
    "transit".into()
}
fn default_transit_key() -> String {
    "huxley".into()
}
fn default_approle_mount() -> String {
    "approle".into()
}
fn default_kubernetes_mount() -> String {
    "kubernetes".into()
}
fn default_kubernetes_jwt_file() -> PathBuf {
    "/var/run/secrets/kubernetes.io/serviceaccount/token".into()
}
fn default_timeout_secs() -> u64 {
    10
}

/// HashiCorp Vault or OpenBao, using a Transit key (`[crypto.vault]`, or
/// `HUXLEY_CRYPTO__VAULT__*`). Set exactly one of `token`, `approle` or
/// `kubernetes`.
#[derive(Debug, Clone, Deserialize)]
pub struct VaultConfig {
    /// Server address, e.g. `https://vault.internal:8200`.
    pub address: String,
    /// Where the Transit engine is mounted.
    #[serde(default = "default_transit_mount")]
    pub mount: String,
    /// The Transit key's name. An `aes256-gcm96` key is recommended.
    #[serde(default = "default_transit_key")]
    pub key: String,
    /// Vault Enterprise / OpenBao namespace.
    pub namespace: Option<String>,
    /// A token (supports `HUXLEY_CRYPTO__VAULT__TOKEN_FILE`). Use a periodic
    /// token or `AppRole`/Kubernetes auth for long-running servers.
    pub token: Option<SecretString>,
    pub approle: Option<AppRoleConfig>,
    pub kubernetes: Option<KubernetesAuthConfig>,
    /// Extra CA certificates (PEM) to trust, for a privately issued server
    /// certificate. The system roots stay trusted.
    pub ca_cert_file: Option<PathBuf>,
    /// The identifier recorded against data keys wrapped by this key.
    /// Defaults to `vault:<namespace/><mount>/<key>`. Keep it stable: data
    /// keys are looked up by it, so changing it needs the old value kept.
    pub id: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

/// `AppRole` login.
#[derive(Debug, Clone, Deserialize)]
pub struct AppRoleConfig {
    pub role_id: String,
    /// Supports `HUXLEY_CRYPTO__VAULT__APPROLE__SECRET_ID_FILE`.
    pub secret_id: SecretString,
    #[serde(default = "default_approle_mount")]
    pub mount: String,
}

/// Kubernetes service account login.
#[derive(Debug, Clone, Deserialize)]
pub struct KubernetesAuthConfig {
    pub role: String,
    /// The projected service account token; read again at every login.
    #[serde(default = "default_kubernetes_jwt_file")]
    pub jwt_file: PathBuf,
    #[serde(default = "default_kubernetes_mount")]
    pub mount: String,
}

#[derive(Debug)]
enum VaultAuth {
    Token(SecretString),
    AppRole(AppRoleConfig),
    Kubernetes(KubernetesAuthConfig),
}

#[derive(Debug)]
struct CachedToken {
    token: SecretString,
    /// When to log in again; `None` for tokens that don't expire.
    refresh_at: Option<Instant>,
}

/// A Vault/OpenBao Transit key.
#[derive(Debug)]
pub(crate) struct VaultKek {
    id: String,
    client: reqwest::Client,
    base: String,
    mount: String,
    key: String,
    namespace: Option<String>,
    auth: VaultAuth,
    token: Mutex<Option<CachedToken>>,
}

const VAULT: &str = "Vault";

impl VaultKek {
    pub(crate) fn new(config: &VaultConfig) -> Result<Self, CryptoError> {
        let auth = match (&config.token, &config.approle, &config.kubernetes) {
            (Some(token), None, None) => VaultAuth::Token(token.clone()),
            (None, Some(approle), None) => VaultAuth::AppRole(approle.clone()),
            (None, None, Some(k8s)) => VaultAuth::Kubernetes(k8s.clone()),
            _ => {
                return Err(CryptoError::Config(
                    "crypto.vault needs exactly one of token, approle or kubernetes".into(),
                ));
            }
        };
        let base = config.address.trim_end_matches('/').to_owned();
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(CryptoError::Config(
                "crypto.vault.address must be an http(s) URL".into(),
            ));
        }
        let mount = config.mount.trim_matches('/').to_owned();
        let namespace = config
            .namespace
            .as_deref()
            .map(|n| n.trim_matches('/').to_owned())
            .filter(|n| !n.is_empty());
        let id = config.id.clone().unwrap_or_else(|| match &namespace {
            Some(ns) => format!("vault:{ns}/{mount}/{}", config.key),
            None => format!("vault:{mount}/{}", config.key),
        });
        if !valid_id(&id) {
            return Err(CryptoError::Config(format!(
                "crypto.vault: {id:?} is not a valid key id (letters, digits and ._/@:- after `vault:`)"
            )));
        }
        let static_token = match &auth {
            VaultAuth::Token(token) => Some(CachedToken {
                token: token.clone(),
                refresh_at: None,
            }),
            _ => None,
        };
        Ok(Self {
            id,
            client: http_client(
                VAULT,
                config.ca_cert_file.as_ref(),
                Duration::from_secs(config.timeout_secs.max(1)),
            )?,
            base,
            mount,
            key: config.key.clone(),
            namespace,
            auth,
            token: Mutex::new(static_token),
        })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) async fn wrap(&self, frame: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let plaintext = Zeroizing::new(STANDARD.encode(frame));
        let body = json!({ "plaintext": &*plaintext });
        let response = self
            .call(&format!("{}/encrypt/{}", self.mount, self.key), &body)
            .await?;
        let ciphertext = response["data"]["ciphertext"]
            .as_str()
            .filter(|c| c.starts_with("vault:"))
            .ok_or_else(|| CryptoError::Kms("Vault returned no ciphertext".into()))?;
        Ok(ciphertext.as_bytes().to_vec())
    }

    pub(crate) async fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let ciphertext = std::str::from_utf8(wrapped).map_err(|_| CryptoError::Decrypt)?;
        let body = json!({ "ciphertext": ciphertext });
        let response = self
            .call(&format!("{}/decrypt/{}", self.mount, self.key), &body)
            .await?;
        let plaintext = response["data"]["plaintext"]
            .as_str()
            .ok_or_else(|| CryptoError::Kms("Vault returned no plaintext".into()))?;
        STANDARD
            .decode(plaintext)
            .map(Zeroizing::new)
            .map_err(|_| CryptoError::Decrypt)
    }

    /// POSTs to `/v1/<path>`, logging in again once if the token was revoked
    /// or expired early.
    async fn call(&self, path: &str, body: &Value) -> Result<Value, CryptoError> {
        let url = format!("{}/v1/{path}", self.base);
        let mut retried = false;
        loop {
            let token = self.token().await?;
            let response = self
                .request(&url)
                .header("X-Vault-Token", token.expose_secret())
                .json(body)
                .send()
                .await
                .map_err(|e| kms_error(VAULT, e))?;
            let status = response.status();
            if status == StatusCode::FORBIDDEN
                && !retried
                && !matches!(self.auth, VaultAuth::Token(_))
            {
                retried = true;
                self.token.lock().expect("token lock").take();
                continue;
            }
            return parse(response).await;
        }
    }

    fn request(&self, url: &str) -> reqwest::RequestBuilder {
        let builder = self.client.post(url);
        match &self.namespace {
            Some(ns) => builder.header("X-Vault-Namespace", ns),
            None => builder,
        }
    }

    async fn token(&self) -> Result<SecretString, CryptoError> {
        {
            let cached = self.token.lock().expect("token lock");
            if let Some(cached) = cached.as_ref()
                && cached.refresh_at.is_none_or(|at| Instant::now() < at)
            {
                return Ok(cached.token.clone());
            }
        }
        let (mount, body) = match &self.auth {
            VaultAuth::Token(token) => return Ok(token.clone()),
            VaultAuth::AppRole(approle) => (
                &approle.mount,
                json!({
                    "role_id": approle.role_id,
                    "secret_id": approle.secret_id.expose_secret(),
                }),
            ),
            VaultAuth::Kubernetes(k8s) => {
                let jwt = std::fs::read_to_string(&k8s.jwt_file).map_err(|e| {
                    CryptoError::Kms(format!(
                        "Vault: cannot read service account token {}: {e}",
                        k8s.jwt_file.display()
                    ))
                })?;
                (&k8s.mount, json!({ "role": k8s.role, "jwt": jwt.trim() }))
            }
        };
        let url = format!("{}/v1/auth/{}/login", self.base, mount.trim_matches('/'));
        let response = self
            .request(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| kms_error(VAULT, e))?;
        let response = parse(response).await?;
        let token = response["auth"]["client_token"]
            .as_str()
            .ok_or_else(|| CryptoError::Kms("Vault login returned no token".into()))?;
        let token = SecretString::from(token);
        let lease = response["auth"]["lease_duration"].as_u64().unwrap_or(0);
        // Log in again at 80% of the lease, before the token expires.
        let refresh_at = (lease > 0).then(|| Instant::now() + Duration::from_secs(lease * 4 / 5));
        *self.token.lock().expect("token lock") = Some(CachedToken {
            token: token.clone(),
            refresh_at,
        });
        tracing::debug!(lease_secs = lease, "logged in to Vault");
        Ok(token)
    }
}

async fn parse(response: reqwest::Response) -> Result<Value, CryptoError> {
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let errors = body["errors"]
        .as_array()
        .map(|e| {
            e.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    Err(CryptoError::Kms(format!(
        "Vault returned {status}: {errors}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_bind_ids() {
        let (dek, org) = (Uuid::now_v7(), Uuid::now_v7());
        let framed = frame(dek, org, b"key bytes");
        assert_eq!(&**unframe(dek, org, &framed).unwrap(), b"key bytes");
        assert!(unframe(Uuid::now_v7(), org, &framed).is_err());
        assert!(unframe(dek, Uuid::now_v7(), &framed).is_err());
        assert!(unframe(dek, org, &framed[..FRAME_HEADER]).is_err());
        assert!(unframe(dek, org, b"short").is_err());
    }

    #[test]
    fn ids_are_validated() {
        assert!(valid_id("vault:transit/huxley"));
        assert!(valid_id("vault:team-a/transit/huxley@2"));
        assert!(!valid_id("transit/huxley"));
        assert!(!valid_id("Vault:x"));
        assert!(!valid_id("vault:"));
        assert!(!valid_id("vault:has space"));
    }

    fn config(extra: &str) -> VaultConfig {
        let toml = format!("address = \"http://127.0.0.1:8200/\"\n{extra}");
        serde_json::from_value(toml_to_json(&toml)).unwrap()
    }

    /// Tiny `key = "value"` parser so tests don't need a TOML dependency.
    fn toml_to_json(toml: &str) -> Value {
        let mut map = serde_json::Map::new();
        for line in toml.lines().filter(|l| !l.trim().is_empty()) {
            let (k, v) = line.split_once('=').unwrap();
            map.insert(
                k.trim().into(),
                Value::String(v.trim().trim_matches('"').into()),
            );
        }
        Value::Object(map)
    }

    #[test]
    fn config_needs_one_auth_method_and_derives_the_id() {
        assert!(VaultKek::new(&config("")).is_err());
        let kek = VaultKek::new(&config("token = \"t\"")).unwrap();
        assert_eq!(kek.id(), "vault:transit/huxley");
        assert_eq!(kek.base, "http://127.0.0.1:8200");
        let kek = VaultKek::new(&config(
            "token = \"t\"\nnamespace = \"/team-a/\"\nkey = \"k\"",
        ))
        .unwrap();
        assert_eq!(kek.id(), "vault:team-a/transit/k");
        assert!(VaultKek::new(&config("token = \"t\"\nid = \"bad id\"")).is_err());
    }
}
