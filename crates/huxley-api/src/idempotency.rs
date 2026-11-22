//! `Idempotency-Key` request header.
//!
//! Clients may send `Idempotency-Key` on POST/PATCH/DELETE so that retrying a
//! request never applies its effect twice. Phase 0 validates and exposes the
//! key. Storing responses per key arrives with the first mutating endpoints in
//! Phase 1.

use axum::extract::FromRequestParts;
use http::request::Parts;

use crate::ApiError;

pub const HEADER: &str = "idempotency-key";
const MAX_LEN: usize = 255;

/// The validated `Idempotency-Key`, if the client sent one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyKey(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for IdempotencyKey {
    type Rejection = ApiError;

    // Validation is synchronous, so return a ready future rather than an
    // `async fn` with no awaits.
    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(extract(parts))
    }
}

fn extract(parts: &Parts) -> Result<IdempotencyKey, ApiError> {
    let Some(value) = parts.headers.get(HEADER) else {
        return Ok(IdempotencyKey(None));
    };
    let key = value.to_str().ok().filter(|k| is_valid(k)).ok_or_else(|| {
        ApiError::bad_request(format!(
            "Idempotency-Key must be 1-{MAX_LEN} visible ASCII characters."
        ))
    })?;
    Ok(IdempotencyKey(Some(key.to_owned())))
}

fn is_valid(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_LEN && key.bytes().all(|b| b.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::is_valid;

    #[test]
    fn validates_keys() {
        assert!(is_valid("8e03978e-40d5-43e8-bc93-6894a57f9324"));
        assert!(!is_valid(""));
        assert!(!is_valid("has space"));
        assert!(!is_valid(&"a".repeat(256)));
    }
}
