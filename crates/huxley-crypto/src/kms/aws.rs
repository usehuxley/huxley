//! AWS Key Management Service, with a symmetric KMS key's Encrypt and
//! Decrypt (ADR-092). Credentials come from the AWS SDK's default chain:
//! environment variables, ECS task roles, EKS Pod Identity and IRSA (web
//! identity), EC2 instance roles; or static keys in this configuration,
//! for Huxley running outside AWS.

use std::time::Duration;

use aws_config::BehaviorVersion;
use aws_config::timeout::TimeoutConfig;
use aws_sdk_kms::Client;
use aws_sdk_kms::config::{Credentials, Region};
use aws_sdk_kms::error::DisplayErrorContext;
use aws_sdk_kms::primitives::Blob;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use tokio::sync::OnceCell;
use zeroize::Zeroizing;

use super::valid_id;
use crate::CryptoError;

const AWS: &str = "AWS KMS";
const WRAP_PREFIX: &[u8] = b"awskms1:";
/// Bound into every wrap as KMS encryption context (and shown in
/// `CloudTrail`), on top of the data-key frame.
const CONTEXT_KEY: &str = "huxley";
const CONTEXT_VALUE: &str = "data-key";

/// A symmetric key in AWS KMS (`[crypto.aws_kms]`, or
/// `HUXLEY_CRYPTO__AWS_KMS__*`).
#[derive(Debug, Clone, Deserialize)]
pub struct AwsKmsConfig {
    /// The key: its id, ARN, or an alias (`alias/huxley` or its ARN).
    pub key_id: String,
    /// Defaults to `AWS_REGION` (or `AWS_DEFAULT_REGION`).
    pub region: Option<String>,
    /// A VPC endpoint, or a KMS-compatible service for testing.
    pub endpoint_url: Option<String>,
    /// Static credentials, for Huxley outside AWS. Leave unset to use the
    /// default chain (roles). Support `…__ACCESS_KEY_ID_FILE` and
    /// `…__SECRET_ACCESS_KEY_FILE`.
    pub access_key_id: Option<SecretString>,
    pub secret_access_key: Option<SecretString>,
    pub session_token: Option<SecretString>,
    /// The identifier recorded against data keys wrapped by this key.
    /// Defaults to `aws:<key_id>`. Keep it stable.
    pub id: Option<String>,
    #[serde(default = "super::default_timeout_secs")]
    pub timeout_secs: u64,
}

#[derive(Debug)]
pub(crate) struct AwsKmsKek {
    id: String,
    config: AwsKmsConfig,
    /// Built on first use: loading AWS credentials is async.
    client: OnceCell<Client>,
}

impl AwsKmsKek {
    pub(crate) fn new(config: &AwsKmsConfig) -> Result<Self, CryptoError> {
        Self::with_env(config, |name| std::env::var(name).ok())
    }

    fn with_env(
        config: &AwsKmsConfig,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, CryptoError> {
        let key_id = config.key_id.trim();
        if key_id.is_empty() {
            return Err(CryptoError::Config(format!("{AWS}: key_id is required")));
        }
        let region = config
            .region
            .clone()
            .or_else(|| env("AWS_REGION"))
            .or_else(|| env("AWS_DEFAULT_REGION"))
            .filter(|r| !r.trim().is_empty());
        if region.is_none() && !key_id.starts_with("arn:") {
            return Err(CryptoError::Config(format!(
                "{AWS}: set region (or AWS_REGION), or give key_id as an ARN"
            )));
        }
        if config.access_key_id.is_some() != config.secret_access_key.is_some() {
            return Err(CryptoError::Config(format!(
                "{AWS}: set both access_key_id and secret_access_key, or neither"
            )));
        }
        let id = config.id.clone().unwrap_or_else(|| format!("aws:{key_id}"));
        if !valid_id(&id) {
            return Err(CryptoError::Config(format!(
                "{AWS}: id {id:?} must look like aws:<name> (letters, digits, ._/@:-)"
            )));
        }
        Ok(Self {
            id,
            config: AwsKmsConfig {
                key_id: key_id.to_owned(),
                // An ARN names its region.
                region: region.or_else(|| key_id.split(':').nth(3).map(str::to_owned)),
                ..config.clone()
            },
            client: OnceCell::new(),
        })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    async fn client(&self) -> &Client {
        self.client
            .get_or_init(|| async {
                let timeout = Duration::from_secs(self.config.timeout_secs.max(1));
                let mut loader = aws_config::defaults(BehaviorVersion::latest()).timeout_config(
                    TimeoutConfig::builder()
                        .connect_timeout(timeout)
                        .operation_timeout(timeout)
                        .build(),
                );
                if let Some(region) = &self.config.region {
                    loader = loader.region(Region::new(region.clone()));
                }
                if let (Some(key), Some(secret)) =
                    (&self.config.access_key_id, &self.config.secret_access_key)
                {
                    loader = loader.credentials_provider(Credentials::new(
                        key.expose_secret(),
                        secret.expose_secret(),
                        self.config
                            .session_token
                            .as_ref()
                            .map(|t| t.expose_secret().to_owned()),
                        None,
                        "huxley-config",
                    ));
                }
                let shared = loader.load().await;
                let mut kms = aws_sdk_kms::config::Builder::from(&shared);
                if let Some(url) = &self.config.endpoint_url {
                    kms = kms.endpoint_url(url);
                }
                Client::from_conf(kms.build())
            })
            .await
    }

    pub(crate) async fn wrap(&self, frame: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let out = self
            .client()
            .await
            .encrypt()
            .key_id(&self.config.key_id)
            .plaintext(Blob::new(frame.to_vec()))
            .encryption_context(CONTEXT_KEY, CONTEXT_VALUE)
            .send()
            .await
            .map_err(|e| CryptoError::Kms(format!("{AWS}: {}", DisplayErrorContext(&e))))?;
        let blob = out
            .ciphertext_blob()
            .ok_or_else(|| CryptoError::Kms(format!("{AWS} returned no ciphertext")))?;
        let mut wrapped = WRAP_PREFIX.to_vec();
        wrapped.extend_from_slice(blob.as_ref());
        Ok(wrapped)
    }

    pub(crate) async fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let blob = wrapped
            .strip_prefix(WRAP_PREFIX)
            .filter(|b| !b.is_empty())
            .ok_or(CryptoError::Decrypt)?;
        // Naming the key makes KMS refuse ciphertext from any other key.
        let out = self
            .client()
            .await
            .decrypt()
            .key_id(&self.config.key_id)
            .ciphertext_blob(Blob::new(blob.to_vec()))
            .encryption_context(CONTEXT_KEY, CONTEXT_VALUE)
            .send()
            .await
            .map_err(|e| {
                let message = DisplayErrorContext(&e).to_string();
                if message.contains("InvalidCiphertextException")
                    || message.contains("IncorrectKeyException")
                {
                    CryptoError::Decrypt
                } else {
                    CryptoError::Kms(format!("{AWS}: {message}"))
                }
            })?;
        out.plaintext()
            .map(|p| Zeroizing::new(p.as_ref().to_vec()))
            .ok_or_else(|| CryptoError::Kms(format!("{AWS} returned no plaintext")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(value: serde_json::Value) -> AwsKmsConfig {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn settings_are_checked() {
        let none = |_: &str| None;
        let ok = AwsKmsKek::with_env(
            &config(serde_json::json!({ "key_id": "alias/huxley", "region": "eu-west-1" })),
            none,
        )
        .unwrap();
        assert_eq!(ok.id(), "aws:alias/huxley");
        // The region from the environment, or from an ARN.
        let from_env = AwsKmsKek::with_env(
            &config(serde_json::json!({ "key_id": "alias/huxley" })),
            |name| (name == "AWS_REGION").then(|| "us-east-2".to_owned()),
        )
        .unwrap();
        assert_eq!(from_env.config.region.as_deref(), Some("us-east-2"));
        let arn = AwsKmsKek::with_env(
            &config(serde_json::json!({ "key_id": "arn:aws:kms:ap-southeast-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab" })),
            none,
        )
        .unwrap();
        assert_eq!(arn.config.region.as_deref(), Some("ap-southeast-2"));
        assert!(
            AwsKmsKek::with_env(
                &config(serde_json::json!({ "key_id": "alias/huxley" })),
                none
            )
            .is_err()
        );
        assert!(
            AwsKmsKek::with_env(
                &config(serde_json::json!({ "key_id": "alias/x", "region": "us-east-1", "access_key_id": "AKIA" })),
                none
            )
            .is_err(),
            "keys come in pairs"
        );
        assert!(
            AwsKmsKek::with_env(
                &config(serde_json::json!({ "key_id": " ", "region": "us-east-1" })),
                none
            )
            .is_err()
        );
    }
}
