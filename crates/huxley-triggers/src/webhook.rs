//! Checking that a webhook request comes from who the trigger expects.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use huxley_hwf::{HmacAlgorithm, WebhookAuth};
use ring::hmac;
use subtle::ConstantTimeEq as _;

/// Oldest signed timestamp accepted, in seconds either way.
pub const REPLAY_WINDOW_SECS: i64 = 300;

/// Why a request was refused. Deliberately vague towards the caller.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Refused(pub &'static str);

/// The request, as far as verification needs it.
pub struct Incoming<'a> {
    /// Header lookup (case-insensitive name).
    pub header: &'a dyn Fn(&str) -> Option<String>,
    pub body: &'a [u8],
    pub now: i64,
}

impl std::fmt::Debug for Incoming<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Incoming")
            .field("body_len", &self.body.len())
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}

/// Secrets already evaluated from the trigger's expressions.
pub enum Resolved {
    Hmac {
        secret: String,
        header: String,
        algorithm: HmacAlgorithm,
        prefix: Option<String>,
        timestamp_header: Option<String>,
    },
    Bearer(String),
    Basic(String, String),
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Hmac { .. } => "Resolved::Hmac(..)",
            Self::Bearer(_) => "Resolved::Bearer(..)",
            Self::Basic(..) => "Resolved::Basic(..)",
        })
    }
}

/// The expressions the trigger's auth needs evaluated, in a fixed order, and
/// a way to rebuild it from their values.
pub fn expressions(auth: &WebhookAuth) -> Vec<&str> {
    match auth {
        WebhookAuth::Hmac { secret, .. } => vec![secret],
        WebhookAuth::Bearer { token } => vec![token],
        WebhookAuth::Basic { username, password } => vec![username, password],
    }
}

pub fn resolve(auth: &WebhookAuth, mut values: Vec<String>) -> Resolved {
    let mut next = || {
        if values.is_empty() {
            String::new()
        } else {
            values.remove(0)
        }
    };
    match auth {
        WebhookAuth::Hmac {
            header,
            algorithm,
            prefix,
            timestamp_header,
            ..
        } => Resolved::Hmac {
            secret: next(),
            header: header.clone(),
            algorithm: *algorithm,
            prefix: prefix.clone(),
            timestamp_header: timestamp_header.clone(),
        },
        WebhookAuth::Bearer { .. } => Resolved::Bearer(next()),
        WebhookAuth::Basic { .. } => {
            let user = next();
            Resolved::Basic(user, next())
        }
    }
}

fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

fn decode_signature(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    let hex = (text.len().is_multiple_of(2) && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
                .collect::<Option<Vec<u8>>>()
        })
        .flatten();
    hex.or_else(|| STANDARD.decode(text).ok())
}

/// Verifies a request. With a timestamp header, the signed message is
/// `{timestamp}.{body}` and the timestamp (Unix seconds) must be within
/// five minutes of now.
pub fn verify(auth: &Resolved, request: &Incoming<'_>) -> Result<(), Refused> {
    match auth {
        Resolved::Hmac {
            secret,
            header,
            algorithm,
            prefix,
            timestamp_header,
        } => {
            if secret.is_empty() {
                return Err(Refused("the webhook's secret is empty"));
            }
            let signature = (request.header)(header).ok_or(Refused("missing signature"))?;
            let signature = match prefix {
                Some(p) => signature
                    .strip_prefix(p.as_str())
                    .ok_or(Refused("malformed signature"))?
                    .to_owned(),
                None => signature,
            };
            let tag = decode_signature(&signature).ok_or(Refused("malformed signature"))?;
            let mut message = Vec::with_capacity(request.body.len() + 16);
            if let Some(name) = timestamp_header {
                let stamp = (request.header)(name).ok_or(Refused("missing timestamp"))?;
                let at: i64 = stamp
                    .trim()
                    .parse()
                    .map_err(|_| Refused("malformed timestamp"))?;
                if (request.now - at).abs() > REPLAY_WINDOW_SECS {
                    return Err(Refused("the timestamp is too old"));
                }
                message.extend_from_slice(stamp.trim().as_bytes());
                message.push(b'.');
            }
            message.extend_from_slice(request.body);
            let algorithm = match algorithm {
                HmacAlgorithm::Sha256 => hmac::HMAC_SHA256,
                HmacAlgorithm::Sha512 => hmac::HMAC_SHA512,
                HmacAlgorithm::Sha1 => hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
            };
            let key = hmac::Key::new(algorithm, secret.as_bytes());
            hmac::verify(&key, &message, &tag).map_err(|_| Refused("bad signature"))
        }
        Resolved::Bearer(token) => {
            if token.is_empty() {
                return Err(Refused("the webhook's token is empty"));
            }
            let given = (request.header)("authorization").unwrap_or_default();
            let given = given.strip_prefix("Bearer ").unwrap_or_default();
            equal(given.as_bytes(), token.as_bytes())
                .then_some(())
                .ok_or(Refused("bad token"))
        }
        Resolved::Basic(user, password) => {
            if password.is_empty() {
                return Err(Refused("the webhook's password is empty"));
            }
            let given = (request.header)("authorization").unwrap_or_default();
            let decoded = given
                .strip_prefix("Basic ")
                .and_then(|b| STANDARD.decode(b.trim()).ok())
                .unwrap_or_default();
            let expected = format!("{user}:{password}");
            equal(&decoded, expected.as_bytes())
                .then_some(())
                .ok_or(Refused("bad credentials"))
        }
    }
}

/// What identifies a signed delivery, for refusing replays: the signed
/// timestamp and a digest of the signature. Only timestamped HMAC requests
/// have one: without a timestamp nothing bounds how long a signature stays
/// valid, and identical bodies may legitimately arrive more than once.
/// Call after [`verify`] has accepted the request.
pub fn delivery(auth: &Resolved, request: &Incoming<'_>) -> Option<(i64, Vec<u8>)> {
    let Resolved::Hmac {
        header,
        prefix,
        timestamp_header: Some(stamp_header),
        ..
    } = auth
    else {
        return None;
    };
    let at: i64 = (request.header)(stamp_header)?.trim().parse().ok()?;
    let signature = (request.header)(header)?;
    let signature = match prefix {
        Some(p) => signature.strip_prefix(p.as_str())?.to_owned(),
        None => signature,
    };
    // Decoded, so the same tag in hex or base64 counts once.
    let tag = decode_signature(&signature)?;
    Some((
        at,
        ring::digest::digest(&ring::digest::SHA256, &tag)
            .as_ref()
            .to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, String)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name.to_ascii_lowercase())
                .map(|(_, v)| v.clone())
        }
    }

    fn sign(secret: &str, message: &[u8]) -> String {
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        hmac::sign(&key, message)
            .as_ref()
            .iter()
            .fold(String::new(), |mut s, b| {
                use std::fmt::Write as _;
                let _ = write!(s, "{b:02x}");
                s
            })
    }

    fn hmac_auth(timestamp: bool) -> Resolved {
        Resolved::Hmac {
            secret: "s3cret".into(),
            header: "X-Signature".into(),
            algorithm: HmacAlgorithm::Sha256,
            prefix: Some("sha256=".into()),
            timestamp_header: timestamp.then(|| "X-Timestamp".into()),
        }
    }

    #[test]
    fn hmac_signatures() {
        let body = br#"{"a":1}"#;
        let good = headers(&[("X-Signature", format!("sha256={}", sign("s3cret", body)))]);
        let request = Incoming {
            header: &good,
            body,
            now: 0,
        };
        assert_eq!(verify(&hmac_auth(false), &request), Ok(()));

        let wrong = headers(&[("X-Signature", format!("sha256={}", sign("other", body)))]);
        let request = Incoming {
            header: &wrong,
            body,
            now: 0,
        };
        assert!(verify(&hmac_auth(false), &request).is_err());
        let tampered = Incoming {
            header: &good,
            body: b"{}",
            now: 0,
        };
        assert!(verify(&hmac_auth(false), &tampered).is_err());
        let none = headers(&[]);
        assert!(
            verify(
                &hmac_auth(false),
                &Incoming {
                    header: &none,
                    body,
                    now: 0
                }
            )
            .is_err()
        );
    }

    #[test]
    fn timestamps_bound_replays() {
        let body = b"hello";
        let signed = |at: i64| {
            headers(&[
                ("X-Timestamp", at.to_string()),
                (
                    "X-Signature",
                    format!(
                        "sha256={}",
                        sign("s3cret", format!("{at}.hello").as_bytes())
                    ),
                ),
            ])
        };
        let fresh = signed(1_000);
        assert_eq!(
            verify(
                &hmac_auth(true),
                &Incoming {
                    header: &fresh,
                    body,
                    now: 1_100
                }
            ),
            Ok(())
        );
        assert_eq!(
            verify(
                &hmac_auth(true),
                &Incoming {
                    header: &fresh,
                    body,
                    now: 2_000
                }
            ),
            Err(Refused("the timestamp is too old"))
        );
    }

    #[test]
    fn deliveries_identify_timestamped_signatures() {
        let at = 1_000;
        let tag = sign("s3cret", format!("{at}.hello").as_bytes());
        let hex = headers(&[
            ("X-Timestamp", at.to_string()),
            ("X-Signature", format!("sha256={tag}")),
        ]);
        let request = |h| Incoming {
            header: h,
            body: b"hello",
            now: at,
        };
        let (stamp, digest) = delivery(&hmac_auth(true), &request(&hex)).unwrap();
        assert_eq!(stamp, at);
        // The same tag in base64 is the same delivery.
        let bytes: Vec<u8> = (0..tag.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&tag[i..i + 2], 16).unwrap())
            .collect();
        let b64 = headers(&[
            ("X-Timestamp", at.to_string()),
            ("X-Signature", format!("sha256={}", STANDARD.encode(bytes))),
        ]);
        assert_eq!(
            delivery(&hmac_auth(true), &request(&b64)).map(|d| d.1),
            Some(digest)
        );
        // Without a timestamp header there's nothing to bound it.
        assert_eq!(delivery(&hmac_auth(false), &request(&hex)), None);
        assert_eq!(
            delivery(&Resolved::Bearer("t".into()), &request(&hex)),
            None
        );
    }

    #[test]
    fn bearer_and_basic() {
        let bearer = Resolved::Bearer("tok".into());
        let ok = headers(&[("Authorization", "Bearer tok".into())]);
        assert!(
            verify(
                &bearer,
                &Incoming {
                    header: &ok,
                    body: b"",
                    now: 0
                }
            )
            .is_ok()
        );
        let bad = headers(&[("Authorization", "Bearer tik".into())]);
        assert!(
            verify(
                &bearer,
                &Incoming {
                    header: &bad,
                    body: b"",
                    now: 0
                }
            )
            .is_err()
        );
        assert!(
            verify(
                &Resolved::Bearer(String::new()),
                &Incoming {
                    header: &headers(&[]),
                    body: b"",
                    now: 0
                }
            )
            .is_err()
        );

        let basic = Resolved::Basic("u".into(), "p".into());
        let ok = headers(&[("Authorization", format!("Basic {}", STANDARD.encode("u:p")))]);
        assert!(
            verify(
                &basic,
                &Incoming {
                    header: &ok,
                    body: b"",
                    now: 0
                }
            )
            .is_ok()
        );
        let bad = headers(&[("Authorization", format!("Basic {}", STANDARD.encode("u:x")))]);
        assert!(
            verify(
                &basic,
                &Incoming {
                    header: &bad,
                    body: b"",
                    now: 0
                }
            )
            .is_err()
        );
    }
}
