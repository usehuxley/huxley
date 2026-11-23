//! Azure Key Vault (and Managed HSM), using an RSA key's wrap and unwrap
//! operations with RSA-OAEP-256.
//!
//! Sign-in, in order of preference: a client secret if one is configured;
//! workload identity (a federated token file, as AKS projects one); otherwise
//! managed identity (Container Apps / App Service endpoint, or the VM
//! instance metadata service).

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::StatusCode;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::{http_client, kms_error, valid_id};
use crate::CryptoError;

const AZURE: &str = "Azure Key Vault";
const API_VERSION: &str = "7.5";
const ALGORITHM: &str = "RSA-OAEP-256";
const WRAP_PREFIX: &str = "azkv1:";
const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com";
const IMDS_ENDPOINT: &str = "http://169.254.169.254/metadata/identity/oauth2/token";

fn default_key() -> String {
    "huxley".into()
}

/// An RSA key in Azure Key Vault (`[crypto.azure_key_vault]`, or
/// `HUXLEY_CRYPTO__AZURE_KEY_VAULT__*`).
#[derive(Debug, Clone, Deserialize)]
pub struct AzureKeyVaultConfig {
    /// The vault's URL, e.g. `https://contoso.vault.azure.net`, or a Managed
    /// HSM's (`https://contoso.managedhsm.azure.net`).
    pub vault_url: String,
    /// The key's name. RSA 3072 or 4096 is recommended.
    #[serde(default = "default_key")]
    pub key: String,
    /// Wrap with this version instead of the key's current one.
    pub key_version: Option<String>,
    #[serde(default)]
    pub auth: AzureAuthConfig,
    /// The identifier recorded against data keys wrapped by this key.
    /// Defaults to `azure:<vault name>/<key>`. Keep it stable.
    pub id: Option<String>,
    /// OAuth resource for tokens. Defaults to the vault host's parent domain
    /// (`https://vault.azure.net`, `https://managedhsm.azure.net`, or a
    /// sovereign cloud's equivalent).
    pub resource: Option<String>,
    #[serde(default = "super::default_timeout_secs")]
    pub timeout_secs: u64,
}

/// How Huxley signs in to Microsoft Entra ID. Empty means managed identity
/// (or workload identity, when `AZURE_FEDERATED_TOKEN_FILE` is set).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AzureAuthConfig {
    /// Defaults to `AZURE_TENANT_ID`.
    pub tenant_id: Option<String>,
    /// The app registration or user-assigned identity. Defaults to
    /// `AZURE_CLIENT_ID`; for system-assigned managed identity, leave unset.
    pub client_id: Option<String>,
    /// Supports `HUXLEY_CRYPTO__AZURE_KEY_VAULT__AUTH__CLIENT_SECRET_FILE`.
    pub client_secret: Option<SecretString>,
    /// Defaults to `AZURE_FEDERATED_TOKEN_FILE`.
    pub federated_token_file: Option<PathBuf>,
    /// Defaults to `AZURE_AUTHORITY_HOST`, then the public cloud.
    pub authority_host: Option<String>,
    /// Overrides the managed identity endpoint.
    pub managed_identity_endpoint: Option<String>,
}

#[derive(Debug)]
enum Credential {
    Secret {
        token_url: String,
        client_id: String,
        secret: SecretString,
    },
    Federated {
        token_url: String,
        client_id: String,
        token_file: PathBuf,
    },
    /// Container Apps / App Service: `IDENTITY_ENDPOINT` + `IDENTITY_HEADER`.
    AppService {
        endpoint: String,
        header: SecretString,
        client_id: Option<String>,
    },
    /// The VM instance metadata service.
    Imds {
        endpoint: String,
        client_id: Option<String>,
    },
}

#[derive(Debug)]
struct CachedToken {
    token: SecretString,
    refresh_at: Instant,
}

/// An Azure Key Vault key.
#[derive(Debug)]
pub(crate) struct AzureKek {
    id: String,
    client: reqwest::Client,
    key_url: String,
    key_version: Option<String>,
    resource: String,
    credential: Credential,
    token: Mutex<Option<CachedToken>>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn config_error(message: impl Into<String>) -> CryptoError {
    CryptoError::Config(format!("crypto.azure_key_vault: {}", message.into()))
}

impl AzureKek {
    pub(crate) fn new(config: &AzureKeyVaultConfig) -> Result<Self, CryptoError> {
        Self::with_env(config, env)
    }

    fn with_env(
        config: &AzureKeyVaultConfig,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, CryptoError> {
        let vault = config.vault_url.trim_end_matches('/');
        let host = vault
            .strip_prefix("https://")
            .or_else(|| {
                vault
                    .strip_prefix("http://")
                    .filter(|h| h.starts_with("127.0.0.1") || h.starts_with("localhost"))
            })
            .filter(|h| !h.is_empty() && !h.contains('/'))
            .ok_or_else(|| config_error("vault_url must be https://<name>.vault.azure.net"))?;
        let (name, parent) = host.split_once('.').unwrap_or((host, host));
        let resource = config
            .resource
            .clone()
            .unwrap_or_else(|| format!("https://{parent}"));
        let id = config
            .id
            .clone()
            .unwrap_or_else(|| format!("azure:{name}/{}", config.key));
        if !valid_id(&id) {
            return Err(config_error(format!("{id:?} is not a valid key id")));
        }
        if config.key.is_empty()
            || !config
                .key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(config_error("key must be a Key Vault key name"));
        }
        Ok(Self {
            id,
            client: http_client(AZURE, None, Duration::from_secs(config.timeout_secs.max(1)))?,
            key_url: format!("{vault}/keys/{}", config.key),
            key_version: config.key_version.clone().filter(|v| !v.is_empty()),
            resource,
            credential: credential(&config.auth, &env)?,
            token: Mutex::new(None),
        })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) async fn wrap(&self, frame: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let url = match &self.key_version {
            Some(version) => format!("{}/{version}/wrapkey", self.key_url),
            None => format!("{}/wrapkey", self.key_url),
        };
        let value = Zeroizing::new(URL_SAFE_NO_PAD.encode(frame));
        let body = json!({ "alg": ALGORITHM, "value": &*value });
        let response = self.call(&url, &body).await?;
        let wrapped = response["value"].as_str();
        let version = response["kid"]
            .as_str()
            .and_then(|kid| kid.rsplit('/').next());
        match (wrapped, version) {
            (Some(wrapped), Some(version)) if !version.is_empty() => {
                Ok(format!("{WRAP_PREFIX}{version}:{wrapped}").into_bytes())
            }
            _ => Err(CryptoError::Kms(format!("{AZURE} returned no wrapped key"))),
        }
    }

    pub(crate) async fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let (version, value) = std::str::from_utf8(wrapped)
            .ok()
            .and_then(|w| w.strip_prefix(WRAP_PREFIX))
            .and_then(|w| w.split_once(':'))
            .filter(|(v, _)| v.bytes().all(|b| b.is_ascii_alphanumeric()))
            .ok_or(CryptoError::Decrypt)?;
        let url = format!("{}/{version}/unwrapkey", self.key_url);
        let response = self
            .call(&url, &json!({ "alg": ALGORITHM, "value": value }))
            .await?;
        let plain = response["value"]
            .as_str()
            .ok_or_else(|| CryptoError::Kms(format!("{AZURE} returned no key")))?;
        URL_SAFE_NO_PAD
            .decode(plain)
            .map(Zeroizing::new)
            .map_err(|_| CryptoError::Decrypt)
    }

    async fn call(&self, url: &str, body: &Value) -> Result<Value, CryptoError> {
        let url = format!("{url}?api-version={API_VERSION}");
        let mut retried = false;
        loop {
            let token = self.token().await?;
            let response = self
                .client
                .post(&url)
                .bearer_auth(token.expose_secret())
                .json(body)
                .send()
                .await
                .map_err(|e| kms_error(AZURE, e))?;
            if response.status() == StatusCode::UNAUTHORIZED && !retried {
                retried = true;
                self.token.lock().expect("token lock").take();
                continue;
            }
            return parse(response).await;
        }
    }

    async fn token(&self) -> Result<SecretString, CryptoError> {
        {
            let cached = self.token.lock().expect("token lock");
            if let Some(cached) = cached.as_ref()
                && Instant::now() < cached.refresh_at
            {
                return Ok(cached.token.clone());
            }
        }
        let request = match &self.credential {
            Credential::Secret {
                token_url,
                client_id,
                secret,
            } => self.client.post(token_url).form_body(&[
                ("grant_type", "client_credentials"),
                ("client_id", client_id),
                ("client_secret", secret.expose_secret()),
                ("scope", &format!("{}/.default", self.resource)),
            ]),
            Credential::Federated {
                token_url,
                client_id,
                token_file,
            } => {
                let assertion = std::fs::read_to_string(token_file).map_err(|e| {
                    CryptoError::Kms(format!(
                        "{AZURE}: cannot read federated token {}: {e}",
                        token_file.display()
                    ))
                })?;
                self.client.post(token_url).form_body(&[
                    ("grant_type", "client_credentials"),
                    ("client_id", client_id),
                    (
                        "client_assertion_type",
                        "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                    ),
                    ("client_assertion", assertion.trim()),
                    ("scope", &format!("{}/.default", self.resource)),
                ])
            }
            Credential::AppService {
                endpoint,
                header,
                client_id,
            } => {
                let mut query = vec![("api-version", "2019-08-01"), ("resource", &self.resource)];
                if let Some(id) = client_id {
                    query.push(("client_id", id));
                }
                self.client
                    .get(with_query(endpoint, &query)?)
                    .header("X-IDENTITY-HEADER", header.expose_secret())
            }
            Credential::Imds {
                endpoint,
                client_id,
            } => {
                let mut query = vec![("api-version", "2018-02-01"), ("resource", &self.resource)];
                if let Some(id) = client_id {
                    query.push(("client_id", id));
                }
                self.client
                    .get(with_query(endpoint, &query)?)
                    .header("Metadata", "true")
            }
        };
        let response = request.send().await.map_err(|e| kms_error(AZURE, e))?;
        let response = parse(response).await?;
        let token = response["access_token"]
            .as_str()
            .ok_or_else(|| CryptoError::Kms(format!("{AZURE}: sign-in returned no token")))?;
        let token = SecretString::from(token);
        // Entra returns a number, managed identity endpoints a string.
        let expires_in = match &response["expires_in"] {
            Value::Number(n) => n.as_u64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
        .unwrap_or(300);
        let refresh_at = Instant::now() + Duration::from_secs(expires_in * 4 / 5);
        *self.token.lock().expect("token lock") = Some(CachedToken {
            token: token.clone(),
            refresh_at,
        });
        tracing::debug!(expires_in, "signed in to Microsoft Entra ID for Key Vault");
        Ok(token)
    }
}

fn credential(
    auth: &AzureAuthConfig,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<Credential, CryptoError> {
    let client_id = auth.client_id.clone().or_else(|| env("AZURE_CLIENT_ID"));
    let token_url = || -> Result<String, CryptoError> {
        let tenant = auth
            .tenant_id
            .clone()
            .or_else(|| env("AZURE_TENANT_ID"))
            .filter(|t| {
                t.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b))
            })
            .ok_or_else(|| config_error("auth.tenant_id is required"))?;
        let authority = auth
            .authority_host
            .clone()
            .or_else(|| env("AZURE_AUTHORITY_HOST"))
            .unwrap_or_else(|| DEFAULT_AUTHORITY.into());
        Ok(format!(
            "{}/{tenant}/oauth2/v2.0/token",
            authority.trim_end_matches('/')
        ))
    };
    if let Some(secret) = &auth.client_secret {
        return Ok(Credential::Secret {
            token_url: token_url()?,
            client_id: client_id
                .ok_or_else(|| config_error("auth.client_id is required with a client secret"))?,
            secret: secret.clone(),
        });
    }
    let federated = auth
        .federated_token_file
        .clone()
        .or_else(|| env("AZURE_FEDERATED_TOKEN_FILE").map(PathBuf::from));
    if let Some(token_file) = federated {
        return Ok(Credential::Federated {
            token_url: token_url()?,
            client_id: client_id
                .ok_or_else(|| config_error("auth.client_id is required for workload identity"))?,
            token_file,
        });
    }
    if let Some(endpoint) = &auth.managed_identity_endpoint {
        return Ok(Credential::Imds {
            endpoint: endpoint.clone(),
            client_id,
        });
    }
    if let (Some(endpoint), Some(header)) = (env("IDENTITY_ENDPOINT"), env("IDENTITY_HEADER")) {
        return Ok(Credential::AppService {
            endpoint,
            header: SecretString::from(header),
            client_id,
        });
    }
    Ok(Credential::Imds {
        endpoint: IMDS_ENDPOINT.into(),
        client_id,
    })
}

fn with_query(endpoint: &str, pairs: &[(&str, &str)]) -> Result<url::Url, CryptoError> {
    url::Url::parse_with_params(endpoint, pairs)
        .map_err(|_| config_error("the managed identity endpoint is not a URL"))
}

trait FormBody {
    fn form_body(self, pairs: &[(&str, &str)]) -> Self;
}

impl FormBody for reqwest::RequestBuilder {
    fn form_body(self, pairs: &[(&str, &str)]) -> Self {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        self.header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
    }
}

async fn parse(response: reqwest::Response) -> Result<Value, CryptoError> {
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    // Key Vault: {"error":{"code","message"}}; Entra: {"error","error_description"}.
    let message = body["error"]["message"]
        .as_str()
        .or_else(|| body["error_description"].as_str())
        .or_else(|| body["error"].as_str())
        .unwrap_or_default();
    Err(CryptoError::Kms(format!(
        "{AZURE} returned {status}: {message}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(value: Value) -> AzureKeyVaultConfig {
        serde_json::from_value(value).unwrap()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn derives_id_and_resource_from_the_vault_url() {
        let kek = AzureKek::with_env(
            &config(json!({ "vault_url": "https://contoso.vault.azure.net/" })),
            no_env,
        )
        .unwrap();
        assert_eq!(kek.id(), "azure:contoso/huxley");
        assert_eq!(kek.resource, "https://vault.azure.net");
        assert_eq!(kek.key_url, "https://contoso.vault.azure.net/keys/huxley");
        assert!(matches!(kek.credential, Credential::Imds { .. }));

        let hsm = AzureKek::with_env(
            &config(json!({ "vault_url": "https://hsm1.managedhsm.azure.net", "key": "k1" })),
            no_env,
        )
        .unwrap();
        assert_eq!(hsm.resource, "https://managedhsm.azure.net");
        assert_eq!(hsm.id(), "azure:hsm1/k1");

        for bad in ["http://contoso.vault.azure.net", "contoso", "https://x/y"] {
            assert!(AzureKek::with_env(&config(json!({ "vault_url": bad })), no_env).is_err());
        }
    }

    #[test]
    fn picks_the_credential() {
        let url = "https://contoso.vault.azure.net";
        // A secret needs a tenant and client id.
        assert!(
            AzureKek::with_env(
                &config(json!({ "vault_url": url, "auth": { "client_secret": "s" } })),
                no_env
            )
            .is_err()
        );
        let kek = AzureKek::with_env(
            &config(
                json!({ "vault_url": url, "auth": { "client_secret": "s", "client_id": "c" } }),
            ),
            |name| (name == "AZURE_TENANT_ID").then(|| "t-1".to_owned()),
        )
        .unwrap();
        assert!(matches!(
            &kek.credential,
            Credential::Secret { token_url, .. }
                if token_url == "https://login.microsoftonline.com/t-1/oauth2/v2.0/token"
        ));
        // AKS workload identity from the environment.
        let aks = |name: &str| match name {
            "AZURE_FEDERATED_TOKEN_FILE" => Some("/var/run/token".to_owned()),
            "AZURE_TENANT_ID" => Some("t".to_owned()),
            "AZURE_CLIENT_ID" => Some("c".to_owned()),
            _ => None,
        };
        let kek = AzureKek::with_env(&config(json!({ "vault_url": url })), aks).unwrap();
        assert!(matches!(kek.credential, Credential::Federated { .. }));
        // Container Apps managed identity.
        let aca = |name: &str| match name {
            "IDENTITY_ENDPOINT" => Some("http://localhost:42356/msi/token".to_owned()),
            "IDENTITY_HEADER" => Some("h".to_owned()),
            _ => None,
        };
        let kek = AzureKek::with_env(&config(json!({ "vault_url": url })), aca).unwrap();
        assert!(matches!(kek.credential, Credential::AppService { .. }));
    }
}
