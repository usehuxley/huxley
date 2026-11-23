//! AES-256-GCM primitives and the key-encryption key (KEK) ring.
//!
//! Everything here is pure computation; [`crate::Crypto`] adds the database.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use secrecy::{ExposeSecret as _, SecretString};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::CryptoError;

/// AES-256 key length in bytes.
pub const KEY_LEN: usize = 32;

/// Generates a random 256-bit key, base64-encoded (what `crypto.kek` expects).
pub fn generate_key_base64() -> String {
    let key = random_key();
    STANDARD.encode(key.as_slice())
}

pub(crate) fn random_key() -> Zeroizing<[u8; KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::fill(&mut *key).expect("the operating system RNG is available");
    key
}

/// An AES-256-GCM key. Nonces are random: at 96 bits they are safe for far
/// more messages per key than a DEK will ever encrypt.
pub(crate) struct AeadKey(LessSafeKey);

impl std::fmt::Debug for AeadKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AeadKey(..)")
    }
}

impl AeadKey {
    pub(crate) fn new(bytes: &[u8]) -> Result<Self, CryptoError> {
        let key = UnboundKey::new(&AES_256_GCM, bytes).map_err(|_| CryptoError::InvalidKey)?;
        Ok(Self(LessSafeKey::new(key)))
    }

    /// Returns `nonce || ciphertext || tag`.
    pub(crate) fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).expect("the operating system RNG is available");
        let mut out = Vec::with_capacity(NONCE_LEN + plaintext.len() + AES_256_GCM.tag_len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(plaintext);
        let mut body = out.split_off(NONCE_LEN);
        self.0
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                &mut body,
            )
            .expect("AES-GCM sealing cannot fail for in-range lengths");
        out.extend_from_slice(&body);
        out
    }

    /// Opens `nonce || ciphertext || tag`. Any tampering, a wrong key, or a
    /// wrong `aad` gives [`CryptoError::Decrypt`].
    pub(crate) fn open(
        &self,
        aad: &[u8],
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        if sealed.len() < NONCE_LEN + AES_256_GCM.tag_len() {
            return Err(CryptoError::Decrypt);
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| CryptoError::Decrypt)?;
        let mut buf = Zeroizing::new(body.to_vec());
        let len = self
            .0
            .open_in_place(nonce, Aad::from(aad), &mut buf)
            .map_err(|_| CryptoError::Decrypt)?
            .len();
        buf.truncate(len);
        Ok(buf)
    }
}

/// A key-encryption key and its fingerprint.
#[derive(Debug)]
pub(crate) struct Kek {
    pub(crate) id: String,
    key: AeadKey,
}

impl Kek {
    fn parse(encoded: &str) -> Result<Self, CryptoError> {
        let bytes = Zeroizing::new(
            STANDARD
                .decode(encoded.trim())
                .map_err(|_| CryptoError::InvalidKek)?,
        );
        if bytes.len() != KEY_LEN {
            return Err(CryptoError::InvalidKek);
        }
        // A fingerprint, not a secret: identifies which KEK wrapped a DEK.
        // Domain-separated so it can't be confused with any other hash of the key.
        let digest = Sha256::new()
            .chain_update(b"huxley-kek-id:")
            .chain_update(&*bytes)
            .finalize();
        let id = digest[..8]
            .iter()
            .fold(String::with_capacity(16), |mut id, b| {
                use std::fmt::Write as _;
                let _ = write!(id, "{b:02x}");
                id
            });
        Ok(Self {
            id,
            key: AeadKey::new(&bytes)?,
        })
    }
}

/// The active KEK plus retired ones that can still unwrap older DEKs.
#[derive(Debug)]
pub struct KeyRing {
    active: Kek,
    retired: Vec<Kek>,
}

/// Binds a wrapped DEK to its id and owner, so a row can't be swapped onto
/// another org or key id.
fn wrap_aad(dek_id: uuid::Uuid, org_id: uuid::Uuid) -> Vec<u8> {
    let mut aad = b"huxley-dek:v1:".to_vec();
    aad.extend_from_slice(dek_id.as_bytes());
    aad.extend_from_slice(org_id.as_bytes());
    aad
}

impl KeyRing {
    /// Builds the ring from base64-encoded 256-bit keys.
    pub fn new(active: &SecretString, retired: &[SecretString]) -> Result<Self, CryptoError> {
        let active = Kek::parse(active.expose_secret())?;
        let retired = retired
            .iter()
            .map(|k| Kek::parse(k.expose_secret()))
            .filter(|k| k.as_ref().map_or(true, |k| k.id != active.id))
            .collect::<Result<_, _>>()?;
        Ok(Self { active, retired })
    }

    /// A ring with a random KEK that exists only in this process. For tests
    /// and throwaway instances: anything it encrypts is lost on exit.
    pub fn ephemeral() -> Self {
        Self::new(&SecretString::from(generate_key_base64()), &[])
            .expect("a generated key is valid")
    }

    /// Fingerprint of the active KEK.
    pub fn active_id(&self) -> &str {
        &self.active.id
    }

    fn find(&self, id: &str) -> Option<&Kek> {
        std::iter::once(&self.active)
            .chain(&self.retired)
            .find(|k| k.id == id)
    }

    /// Wraps a DEK with the active KEK. Returns (KEK id, wrapped bytes).
    pub(crate) fn wrap(
        &self,
        dek_id: uuid::Uuid,
        org_id: uuid::Uuid,
        dek: &[u8],
    ) -> (String, Vec<u8>) {
        let wrapped = self.active.key.seal(&wrap_aad(dek_id, org_id), dek);
        (self.active.id.clone(), wrapped)
    }

    pub(crate) fn unwrap(
        &self,
        kek_id: &str,
        dek_id: uuid::Uuid,
        org_id: uuid::Uuid,
        wrapped: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let kek = self
            .find(kek_id)
            .ok_or_else(|| CryptoError::UnknownKek(kek_id.to_owned()))?;
        kek.key.open(&wrap_aad(dek_id, org_id), wrapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn secret(s: String) -> SecretString {
        SecretString::from(s)
    }

    #[test]
    fn seal_and_open_round_trip() {
        let key = AeadKey::new(&*random_key()).unwrap();
        let sealed = key.seal(b"ctx", b"hello");
        assert_eq!(&**key.open(b"ctx", &sealed).unwrap(), b"hello");
        // Fresh nonce every time.
        assert_ne!(sealed, key.seal(b"ctx", b"hello"));
    }

    #[test]
    fn open_rejects_tampering_wrong_aad_and_wrong_key() {
        let key = AeadKey::new(&*random_key()).unwrap();
        let sealed = key.seal(b"ctx", b"hello");
        assert!(key.open(b"other", &sealed).is_err());
        let mut flipped = sealed.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert!(key.open(b"ctx", &flipped).is_err());
        assert!(key.open(b"ctx", &sealed[..10]).is_err());
        let other = AeadKey::new(&*random_key()).unwrap();
        assert!(other.open(b"ctx", &sealed).is_err());
    }

    #[test]
    fn kek_must_be_256_bits_of_base64() {
        assert!(KeyRing::new(&secret("not base64!".into()), &[]).is_err());
        assert!(KeyRing::new(&secret(STANDARD.encode([0u8; 16])), &[]).is_err());
        assert!(KeyRing::new(&secret(generate_key_base64()), &[]).is_ok());
    }

    #[test]
    fn retired_keks_still_unwrap() {
        let old = generate_key_base64();
        let new = generate_key_base64();
        let (dek_id, org_id) = (Uuid::now_v7(), Uuid::now_v7());

        let before = KeyRing::new(&secret(old.clone()), &[]).unwrap();
        let (kek_id, wrapped) = before.wrap(dek_id, org_id, b"dek bytes");

        let after = KeyRing::new(&secret(new), &[secret(old)]).unwrap();
        assert_ne!(after.active_id(), kek_id);
        assert_eq!(
            &**after.unwrap(&kek_id, dek_id, org_id, &wrapped).unwrap(),
            b"dek bytes"
        );
        // Bound to its ids.
        assert!(
            after
                .unwrap(&kek_id, Uuid::now_v7(), org_id, &wrapped)
                .is_err()
        );
        assert!(
            after
                .unwrap(&kek_id, dek_id, Uuid::now_v7(), &wrapped)
                .is_err()
        );
    }

    #[test]
    fn unknown_kek_is_reported_by_id() {
        let ring = KeyRing::new(&secret(generate_key_base64()), &[]).unwrap();
        let err = ring
            .unwrap("0000000000000000", Uuid::nil(), Uuid::nil(), b"x")
            .unwrap_err();
        assert!(matches!(err, CryptoError::UnknownKek(id) if id == "0000000000000000"));
    }

    #[test]
    fn fingerprints_are_stable_and_distinct() {
        let k = generate_key_base64();
        let a = KeyRing::new(&secret(k.clone()), &[]).unwrap();
        let b = KeyRing::new(&secret(k), &[]).unwrap();
        let c = KeyRing::new(&secret(generate_key_base64()), &[]).unwrap();
        assert_eq!(a.active_id(), b.active_id());
        assert_ne!(a.active_id(), c.active_id());
        assert_eq!(a.active_id().len(), 16);
    }
}
