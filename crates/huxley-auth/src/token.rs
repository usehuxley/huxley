//! Opaque bearer tokens (sessions now, API tokens later).
//!
//! A token is 256 random bits, base64url-encoded behind a short prefix that
//! identifies its kind (useful for secret scanners). Only the SHA-256 of the
//! token is stored, so a database leak does not reveal usable tokens. SHA-256
//! rather than a slow hash is fine here: the input is high-entropy random data,
//! not a human-chosen password.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};

const TOKEN_BYTES: usize = 32;

/// Prefix for session tokens.
pub const SESSION_PREFIX: &str = "hxs_";

/// A newly generated token. `secret` goes to the client once; `hash` is stored.
#[derive(Debug)]
pub struct NewToken {
    pub secret: String,
    pub hash: Vec<u8>,
}

/// Generates a token with the given prefix.
pub fn generate(prefix: &str) -> NewToken {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).expect("the operating system RNG is available");
    let secret = format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes));
    let hash = hash(&secret);
    NewToken { secret, hash }
}

/// The stored form of a token.
pub fn hash(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

/// Cheap shape check before touching the database: right prefix and length.
pub fn looks_valid(secret: &str, prefix: &str) -> bool {
    // 32 bytes encode to 43 base64url characters without padding.
    secret.len() == prefix.len() + 43
        && secret.starts_with(prefix)
        && secret[prefix.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_unique_and_well_formed() {
        let a = generate(SESSION_PREFIX);
        let b = generate(SESSION_PREFIX);
        assert_ne!(a.secret, b.secret);
        assert!(looks_valid(&a.secret, SESSION_PREFIX));
        assert_eq!(a.hash, hash(&a.secret));
        assert_eq!(a.hash.len(), 32);
    }

    #[test]
    fn rejects_malformed_tokens() {
        assert!(!looks_valid("hxs_short", SESSION_PREFIX));
        assert!(!looks_valid(
            &format!("xxx_{}", "a".repeat(43)),
            SESSION_PREFIX
        ));
        assert!(!looks_valid(
            &format!("hxs_{}", "!".repeat(43)),
            SESSION_PREFIX
        ));
    }
}
