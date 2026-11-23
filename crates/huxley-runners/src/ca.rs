//! The instance's runner CA (ADR-025): issues runners' client certificates
//! and the gateway's server certificate. Its key is kept encrypted by the
//! caller (see `registry`); this module only does the X.509 work.

use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DistinguishedName,
    DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use time::{Duration, OffsetDateTime};

/// How long a runner's certificate is valid.
pub const RUNNER_CERT_DAYS: i64 = 7;

#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error("certificate error: {0}")]
    Rcgen(#[from] rcgen::Error),
    #[error("{0}")]
    Invalid(String),
}

/// A newly made CA: its certificate and key, as PEM.
pub struct NewCa {
    pub cert_pem: String,
    pub key_pem: String,
}

// Never print the key.
impl std::fmt::Debug for NewCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewCa")
            .field("cert_pem", &self.cert_pem)
            .finish_non_exhaustive()
    }
}

/// Makes a self-signed CA for runners (ECDSA P-256, ten years).
pub fn create() -> Result<NewCa, CaError> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "Huxley runner CA");
    params.distinguished_name = name;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::hours(1);
    params.not_after = now + Duration::days(3650);
    let cert = params.self_signed(&key)?;
    Ok(NewCa {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

/// A loaded CA that can issue certificates.
pub struct Ca {
    issuer: Issuer<'static, KeyPair>,
    pub cert_pem: String,
}

impl std::fmt::Debug for Ca {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ca")
            .field("cert_pem", &self.cert_pem)
            .finish_non_exhaustive()
    }
}

/// An issued certificate, as PEM, and when it expires.
#[derive(Debug)]
pub struct Issued {
    pub cert_pem: String,
    pub expires_at: OffsetDateTime,
}

impl Ca {
    pub fn load(cert_pem: &str, key_pem: &str) -> Result<Self, CaError> {
        let key = KeyPair::from_pem(key_pem)?;
        let issuer = Issuer::from_ca_cert_pem(cert_pem, key)?;
        Ok(Self {
            issuer,
            cert_pem: cert_pem.to_owned(),
        })
    }

    /// A client certificate for the runner `runner_id`, for the key in the
    /// CSR. The CSR's own names are ignored: Huxley names the runner.
    pub fn issue_runner(&self, csr_pem: &str, runner_id: &str) -> Result<Issued, CaError> {
        let mut csr = CertificateSigningRequestParams::from_pem(csr_pem)
            .map_err(|e| CaError::Invalid(format!("not a certificate signing request: {e}")))?;
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, runner_id);
        csr.params.distinguished_name = name;
        csr.params.subject_alt_names = vec![SanType::URI(
            format!("urn:huxley:runner:{runner_id}")
                .try_into()
                .map_err(|_| CaError::Invalid("runner id".into()))?,
        )];
        csr.params.is_ca = IsCa::ExplicitNoCa;
        csr.params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        csr.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let now = OffsetDateTime::now_utc();
        csr.params.not_before = now - Duration::minutes(5);
        csr.params.not_after = now + Duration::days(RUNNER_CERT_DAYS);
        let expires_at = csr.params.not_after;
        let cert = csr.signed_by(&self.issuer)?;
        Ok(Issued {
            cert_pem: cert.pem(),
            expires_at,
        })
    }

    /// The gateway's server certificate and key for `hosts` (names or IPs).
    pub fn issue_server(&self, hosts: &[String]) -> Result<(String, String), CaError> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(hosts.to_vec())?;
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, "Huxley runner gateway");
        params.distinguished_name = name;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::minutes(5);
        params.not_after = now + Duration::days(90);
        let cert = params.signed_by(&key, &self.issuer)?;
        Ok((cert.pem(), key.serialize_pem()))
    }
}

/// A runner's key and the CSR for it (the runner side of enrolment).
pub fn runner_key_and_csr() -> Result<(String, String), CaError> {
    let key = KeyPair::generate()?;
    let params = CertificateParams::default();
    let csr = params.serialize_request(&key)?;
    Ok((key.serialize_pem(), csr.pem()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_runner_and_server_certificates() {
        let new = create().unwrap();
        let ca = Ca::load(&new.cert_pem, &new.key_pem).unwrap();
        let (_, csr) = runner_key_and_csr().unwrap();
        let issued = ca.issue_runner(&csr, "0190-runner").unwrap();
        assert!(issued.cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        let days = (issued.expires_at - OffsetDateTime::now_utc()).whole_days();
        assert!((6..=7).contains(&days));
        let (server, key) = ca.issue_server(&["localhost".into()]).unwrap();
        assert!(server.contains("BEGIN CERTIFICATE") && key.contains("PRIVATE KEY"));
        assert!(ca.issue_runner("not a csr", "x").is_err());
    }
}
