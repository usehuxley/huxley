//! Test support. Integration tests share one database, and data keys created
//! for shared orgs (the root) must be readable by every test, so they all use
//! this fixed KEK instead of a random one.

use secrecy::SecretString;

use crate::KeyRing;

/// Fixed test-only KEK ("huxley-test-kek-not-for-real-use", base64).
const TEST_KEK: &str = "aHV4bGV5LXRlc3Qta2VrLW5vdC1mb3ItcmVhbC11c2U=";

/// The shared test key ring.
pub fn key_ring() -> KeyRing {
    KeyRing::new(&SecretString::from(TEST_KEK), &[]).expect("valid test key")
}
