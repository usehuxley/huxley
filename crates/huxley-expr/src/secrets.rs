//! `secrets.*` in expressions, and redaction of what they produce.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use minijinja::value::{Object, ObjectRepr};
use minijinja::{Error, ErrorKind, State, Value};
use serde_json::Value as Json;

/// What a redacted value is replaced with.
pub const REDACTED: &str = "«redacted»";

/// Shortest secret masked inside other text: masking very short strings
/// would mangle unrelated output and reveals little anyway.
const MIN_MASKED_LEN: usize = 4;

const GENERATE_ALPHABET: &[u8] =
    b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!#%+-=?@^_";

/// `secrets`: looking up a secret, or generating one, marks the evaluation
/// as tainted. The object itself is opaque: printing or serializing it gives
/// [`REDACTED`], and it can't be iterated, so `{{ secrets }}` reveals nothing.
#[derive(Debug)]
pub(crate) struct SecretsObject {
    values: Arc<BTreeMap<String, String>>,
    touched: Arc<AtomicBool>,
}

impl SecretsObject {
    pub(crate) fn new(values: Arc<BTreeMap<String, String>>, touched: Arc<AtomicBool>) -> Self {
        Self { values, touched }
    }
}

impl Object for SecretsObject {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let value = self.values.get(key.as_str()?)?;
        self.touched.store(true, Ordering::Relaxed);
        Some(Value::from(value.as_str()))
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &mut State<'_, '_>,
        method: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        if method != "generate" {
            return Err(Error::from(ErrorKind::UnknownMethod));
        }
        let length = match args {
            [] => 24,
            [n] => usize::try_from(n.clone())
                .ok()
                .filter(|n| (8..=256).contains(n))
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidOperation,
                        "secrets.generate takes a length from 8 to 256",
                    )
                })?,
            _ => return Err(Error::from(ErrorKind::TooManyArguments)),
        };
        self.touched.store(true, Ordering::Relaxed);
        Ok(Value::from(generate(length)))
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

/// An object some of whose keys are secret (e.g. `inputs` with a
/// `writeOnly` input): reading one, or iterating the object, taints the
/// evaluation like reading `secrets.*`.
#[derive(Debug)]
pub(crate) struct PartlySecret {
    values: BTreeMap<String, Value>,
    secret: std::collections::BTreeSet<String>,
    touched: Arc<AtomicBool>,
}

impl PartlySecret {
    pub(crate) fn new(
        values: BTreeMap<String, Value>,
        secret: std::collections::BTreeSet<String>,
        touched: Arc<AtomicBool>,
    ) -> Self {
        Self {
            values,
            secret,
            touched,
        }
    }
}

impl Object for PartlySecret {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Map
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let name = key.as_str()?;
        let value = self.values.get(name)?;
        if self.secret.contains(name) {
            self.touched.store(true, Ordering::Relaxed);
        }
        Some(value.clone())
    }

    fn enumerate(self: &Arc<Self>) -> minijinja::value::Enumerator {
        // Listing or serializing the whole object reads the secrets too.
        if self.values.keys().any(|k| self.secret.contains(k)) {
            self.touched.store(true, Ordering::Relaxed);
        }
        minijinja::value::Enumerator::Values(
            self.values
                .keys()
                .map(|k| Value::from(k.as_str()))
                .collect(),
        )
    }
}

/// A random password from an unambiguous alphabet.
fn generate(length: usize) -> String {
    // Rejection sampling keeps the distribution uniform.
    let limit = 256 - (256 % GENERATE_ALPHABET.len());
    let mut out = String::with_capacity(length);
    let mut buf = [0u8; 64];
    while out.len() < length {
        getrandom::fill(&mut buf).expect("the operating system RNG is available");
        for &b in &buf {
            if usize::from(b) < limit && out.len() < length {
                out.push(char::from(
                    GENERATE_ALPHABET[usize::from(b) % GENERATE_ALPHABET.len()],
                ));
            }
        }
    }
    out
}

/// Masks every occurrence of the given secret values in a JSON document
/// (in strings and object keys).
pub fn redact<'a>(value: &Json, secrets: impl IntoIterator<Item = &'a str>) -> Json {
    let mut secrets: Vec<&str> = secrets
        .into_iter()
        .filter(|s| s.len() >= MIN_MASKED_LEN)
        .collect();
    // Longest first, so a secret containing another is masked whole.
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    secrets.dedup();
    if secrets.is_empty() {
        return value.clone();
    }
    redact_with(value, &secrets)
}

fn mask(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_owned();
    for secret in secrets {
        if out.contains(secret) {
            out = out.replace(secret, REDACTED);
        }
    }
    out
}

fn redact_with(value: &Json, secrets: &[&str]) -> Json {
    match value {
        Json::String(s) => Json::String(mask(s, secrets)),
        Json::Array(items) => Json::Array(items.iter().map(|v| redact_with(v, secrets)).collect()),
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(k, v)| (mask(k, secrets), redact_with(v, secrets)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_nested_values_and_keys() {
        let doc = json!({
            "url": "https://x/?key=hunter22",
            "list": ["hunter22", "hunter2222", 5],
            "hunter22": true,
            "short": "abc",
        });
        let out = redact(&doc, ["hunter22", "hunter2222", "abc"]);
        assert_eq!(
            out,
            json!({
                "url": "https://x/?key=«redacted»",
                "list": ["«redacted»", "«redacted»", 5],
                "«redacted»": true,
                "short": "abc",
            })
        );
    }

    #[test]
    fn generated_secrets_use_the_alphabet() {
        let s = generate(64);
        assert_eq!(s.len(), 64);
        assert!(s.bytes().all(|b| GENERATE_ALPHABET.contains(&b)));
        assert_ne!(generate(24), generate(24));
    }
}
