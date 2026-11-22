//! Password hashing with Argon2id (OWASP-recommended parameters) and policy.

use std::sync::LazyLock;

use argon2::{Algorithm, Argon2, Params, PasswordHasher as _, PasswordVerifier as _, Version};

/// OWASP minimums for Argon2id: 19 MiB memory, 2 iterations, 1 lane.
const M_COST_KIB: u32 = 19_456;
const T_COST: u32 = 2;
const P_COST: u32 = 1;

/// NIST SP 800-63B: at least 8 characters; we require 12. No composition
/// rules. An upper bound stops very long inputs from being used to burn CPU.
pub const MIN_LENGTH: usize = 12;
pub const MAX_LENGTH: usize = 256;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PasswordPolicyError {
    #[error("password must be at least {MIN_LENGTH} characters")]
    TooShort,
    #[error("password must be at most {MAX_LENGTH} characters")]
    TooLong,
}

fn hasher() -> Argon2<'static> {
    let params = Params::new(M_COST_KIB, T_COST, P_COST, None).expect("valid Argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Checks a new password against the policy. Length counts characters, not bytes.
pub fn validate(password: &str) -> Result<(), PasswordPolicyError> {
    let len = password.chars().count();
    if len < MIN_LENGTH {
        Err(PasswordPolicyError::TooShort)
    } else if len > MAX_LENGTH {
        Err(PasswordPolicyError::TooLong)
    } else {
        Ok(())
    }
}

/// Hashes a password into a PHC string (`$argon2id$v=19$m=...`).
pub fn hash(password: &str) -> String {
    hasher()
        .hash_password(password.as_bytes())
        .expect("Argon2 hashing with valid parameters cannot fail")
        .to_string()
}

/// Verifies a password against a stored PHC hash. Malformed hashes never match.
pub fn verify(password: &str, phc: &str) -> bool {
    // Over-long inputs are rejected before hashing; they can never be valid.
    if password.chars().count() > MAX_LENGTH {
        return false;
    }
    hasher().verify_password(password.as_bytes(), phc).is_ok()
}

/// A hash of a random password, verified against when the user doesn't exist
/// so that response time does not reveal whether an email is registered.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| hash("dummy-password-for-timing"));

/// Spends the same time as a real verification, then fails.
pub fn verify_dummy(password: &str) {
    let _ = verify(password, &DUMMY_HASH);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_round_trip() {
        let phc = hash("correct horse battery staple");
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify("correct horse battery staple", &phc));
        assert!(!verify("wrong horse battery staple", &phc));
    }

    #[test]
    fn hashes_are_salted() {
        assert_ne!(hash("same password here"), hash("same password here"));
    }

    #[test]
    fn malformed_hash_never_matches() {
        assert!(!verify("anything at all", "not-a-phc-string"));
    }

    #[test]
    fn policy() {
        assert_eq!(validate("short"), Err(PasswordPolicyError::TooShort));
        assert_eq!(
            validate(&"a".repeat(257)),
            Err(PasswordPolicyError::TooLong)
        );
        assert!(validate("twelve chars").is_ok());
        // Characters, not bytes: 12 multibyte characters pass.
        assert!(validate(&"é".repeat(12)).is_ok());
    }
}
