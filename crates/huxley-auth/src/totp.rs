//! RFC 6238 time-based one-time passwords (the six-digit codes from
//! authenticator apps) and recovery codes.
//!
//! Parameters are the ones every authenticator app supports: HMAC-SHA1,
//! 6 digits, 30-second steps. One step of clock skew is accepted either way.

use std::fmt::Write as _;

use ring::hmac;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

/// Secret length in bytes (160 bits, as RFC 4226 recommends).
pub const SECRET_LEN: usize = 20;
const STEP_SECONDS: i64 = 30;
const DIGITS: u32 = 6;
/// Steps of clock skew tolerated before and after the current one.
const SKEW: i64 = 1;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn generate_secret() -> Zeroizing<Vec<u8>> {
    let mut secret = Zeroizing::new(vec![0u8; SECRET_LEN]);
    getrandom::fill(&mut secret).expect("the operating system RNG is available");
    secret
}

/// RFC 4648 base32 without padding: what authenticator apps expect.
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The time step containing `unix_seconds`.
pub fn step_at(unix_seconds: i64) -> i64 {
    unix_seconds.div_euclid(STEP_SECONDS)
}

/// The code for one time step.
pub fn code_at_step(secret: &[u8], step: i64) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let digest = tag.as_ref();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!(
        "{:0width$}",
        binary % 10u32.pow(DIGITS),
        width = DIGITS as usize
    )
}

/// Checks `code` against the steps around `now`, skipping any at or before
/// `last_used_step`. Returns the matching step, which the caller must store
/// as the new `last_used_step` so the code can't be used again.
pub fn verify(
    secret: &[u8],
    code: &str,
    now_unix: i64,
    last_used_step: Option<i64>,
) -> Option<i64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let current = step_at(now_unix);
    let mut matched = None;
    // Check every candidate step so timing doesn't reveal which one matched.
    for step in (current - SKEW)..=(current + SKEW) {
        let ok = bool::from(code_at_step(secret, step).as_bytes().ct_eq(code.as_bytes()));
        if ok && last_used_step.is_none_or(|last| step > last) && matched.is_none() {
            matched = Some(step);
        }
    }
    matched
}

/// The `otpauth://` URI authenticator apps scan from a QR code.
pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let issuer_enc = percent_encode(issuer);
    format!(
        "otpauth://totp/{issuer_enc}:{}?secret={}&issuer={issuer_enc}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        percent_encode(account),
        base32_encode(secret),
    )
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'@') {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// How many recovery codes a user gets at a time.
pub const RECOVERY_CODE_COUNT: usize = 10;
/// Crockford base32: 32 symbols, no I, L, O, or U. When a code is typed back,
/// O reads as 0 and I or L as 1, so look-alike mistakes still work.
const RECOVERY_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const RECOVERY_CHARS: usize = 10;

/// A new recovery code, formatted `XXXXX-XXXXX` (50 bits of entropy).
pub fn generate_recovery_code() -> String {
    let mut bytes = [0u8; RECOVERY_CHARS];
    getrandom::fill(&mut bytes).expect("the operating system RNG is available");
    let chars: String = bytes
        .iter()
        .map(|b| RECOVERY_ALPHABET[(b & 31) as usize] as char)
        .collect();
    format!("{}-{}", &chars[..5], &chars[5..])
}

/// The stored form of a recovery code. Case, spaces, dashes, and look-alike
/// letters are normalized so users can type it however it's easiest.
pub fn hash_recovery_code(code: &str) -> Vec<u8> {
    let normalized: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    Sha256::new()
        .chain_update(b"huxley-recovery:")
        .chain_update(normalized.as_bytes())
        .finalize()
        .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 Appendix B, SHA-1 key, truncated to 6 digits.
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_test_vectors() {
        for (time, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(code_at_step(RFC_SECRET, step_at(time)), code, "t={time}");
        }
    }

    #[test]
    fn base32_matches_rfc4648() {
        assert_eq!(base32_encode(b""), "");
        assert_eq!(base32_encode(b"f"), "MY");
        assert_eq!(base32_encode(b"fo"), "MZXQ");
        assert_eq!(base32_encode(b"foo"), "MZXW6");
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_encode(&[0u8; SECRET_LEN]).len(), 32);
    }

    #[test]
    fn verify_accepts_skew_and_rejects_replay() {
        let now = 1_111_111_111;
        let step = step_at(now);
        let code = code_at_step(RFC_SECRET, step);
        assert_eq!(verify(RFC_SECRET, &code, now, None), Some(step));
        assert_eq!(
            verify(
                RFC_SECRET,
                &format!(" {} {} ", &code[..3], &code[3..]),
                now,
                None
            ),
            Some(step)
        );
        // Same code again after it was used: rejected.
        assert_eq!(verify(RFC_SECRET, &code, now, Some(step)), None);
        // Previous and next steps are accepted.
        let prev = code_at_step(RFC_SECRET, step - 1);
        assert_eq!(verify(RFC_SECRET, &prev, now, None), Some(step - 1));
        let next = code_at_step(RFC_SECRET, step + 1);
        assert_eq!(verify(RFC_SECRET, &next, now, Some(step)), Some(step + 1));
        // Two steps away: rejected.
        let old = code_at_step(RFC_SECRET, step - 2);
        assert_eq!(verify(RFC_SECRET, &old, now, None), None);
        // Malformed.
        assert_eq!(verify(RFC_SECRET, "12345", now, None), None);
        assert_eq!(verify(RFC_SECRET, "abcdef", now, None), None);
    }

    #[test]
    fn otpauth_uri_is_encoded() {
        let uri = otpauth_uri("Huxley", "ada lovelace@example.com", b"foo");
        assert_eq!(
            uri,
            "otpauth://totp/Huxley:ada%20lovelace@example.com?secret=MZXW6&issuer=Huxley&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn recovery_codes_are_formatted_and_normalized() {
        let code = generate_recovery_code();
        assert_eq!(code.len(), 11);
        assert_eq!(code.as_bytes()[5], b'-');
        assert_ne!(code, generate_recovery_code());
        let typed = code.to_lowercase().replace('-', " ");
        assert_eq!(hash_recovery_code(&code), hash_recovery_code(&typed));
        assert_eq!(
            hash_recovery_code("10000-00000"),
            hash_recovery_code("lOooo oOOOO")
        );
        assert_ne!(hash_recovery_code(&code), hash_recovery_code("AAAAA-AAAAA"));
    }
}
