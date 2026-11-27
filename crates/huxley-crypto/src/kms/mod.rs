//! Key-encryption keys held by a key management service (KMS).
//!
//! Huxley generates each data key itself and asks the KMS to encrypt
//! ("wrap") it; the KMS key never leaves the service. What the KMS encrypts is
//! framed with the data key's id and owning org, and checked on the way back,
//! so a wrapped key copied onto another row is rejected just as it is for a
//! configured KEK. The frame works with any provider, whether or not its API
//! takes associated data.
//!
//! Providers: HashiCorp Vault and OpenBao (Transit), Azure Key Vault, and
//! AWS KMS.

mod aws;
mod azure;
mod vault;

use std::path::PathBuf;
use std::time::Duration;

use uuid::Uuid;
use zeroize::Zeroizing;

use crate::CryptoError;

pub use aws::AwsKmsConfig;
pub(crate) use aws::AwsKmsKek;
pub(crate) use azure::AzureKek;
pub use azure::{AzureAuthConfig, AzureKeyVaultConfig};
pub(crate) use vault::VaultKek;
pub use vault::{AppRoleConfig, KubernetesAuthConfig, VaultConfig};

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
pub(super) fn valid_id(id: &str) -> bool {
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

pub(super) fn kms_error(provider: &str, error: reqwest::Error) -> CryptoError {
    // Without the URL: it can carry a namespace or key name, never a secret,
    // but the message is shorter and the config already says where it points.
    CryptoError::Kms(format!(
        "{provider} request failed: {}",
        error.without_url()
    ))
}

pub(super) fn http_client(
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

pub(super) fn default_timeout_secs() -> u64 {
    10
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
}
