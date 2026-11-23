//! Huxley's filters and functions, on top of minijinja's builtins and the
//! contrib set (Python-style methods, date formatting, `now()`).

// minijinja passes keyword arguments by value.
#![allow(clippy::needless_pass_by_value)]

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use minijinja::value::Kwargs;
use minijinja::{Environment, Error, ErrorKind, Value};
use sha2::{Digest as _, Sha256, Sha512};

/// Longest pattern accepted by the regex filters.
const MAX_PATTERN_LEN: usize = 1000;
/// Compiled-size cap, so a pattern can't use unbounded memory.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

pub(crate) fn register(env: &mut Environment<'static>) {
    env.add_filter("from_json", from_json);
    env.add_filter("to_json", to_json);
    env.add_filter("b64encode", b64encode);
    env.add_filter("b64decode", b64decode);
    env.add_filter("hash", hash);
    env.add_filter("slugify", slugify);
    env.add_filter("regex_match", regex_match);
    env.add_filter("regex_search", regex_search);
    env.add_filter("regex_replace", regex_replace);
    env.add_filter("regex_findall", regex_findall);
    env.add_filter("default_if_none", default_if_none);
    env.add_function("uuid", uuid);
}

fn invalid(message: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::new(ErrorKind::InvalidOperation, message)
}

fn from_json(text: &str) -> Result<Value, Error> {
    let parsed: serde_json::Value =
        serde_json::from_str(text).map_err(|e| invalid(format!("from_json: {e}")))?;
    Ok(Value::from(minijinja::value::Serde(&parsed)))
}

fn to_json(value: &Value, kwargs: Kwargs) -> Result<String, Error> {
    let pretty: Option<bool> = kwargs.get("pretty")?;
    kwargs.assert_all_used()?;
    let result = if pretty.unwrap_or(false) {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    result.map_err(|e| invalid(format!("to_json: {e}")))
}

fn b64encode(text: &str, kwargs: Kwargs) -> Result<String, Error> {
    let urlsafe: Option<bool> = kwargs.get("urlsafe")?;
    kwargs.assert_all_used()?;
    Ok(if urlsafe.unwrap_or(false) {
        URL_SAFE_NO_PAD.encode(text)
    } else {
        STANDARD.encode(text)
    })
}

fn b64decode(text: &str) -> Result<String, Error> {
    let trimmed = text.trim();
    let bytes = STANDARD
        .decode(trimmed)
        .or_else(|_| URL_SAFE_NO_PAD.decode(trimmed.trim_end_matches('=')))
        .map_err(|_| invalid("b64decode: not base64"))?;
    String::from_utf8(bytes).map_err(|_| invalid("b64decode: the decoded bytes aren't text"))
}

fn hash(text: &str, algorithm: Option<&str>) -> Result<String, Error> {
    let digest = match algorithm.unwrap_or("sha256") {
        "sha256" => Sha256::digest(text.as_bytes()).to_vec(),
        "sha512" => Sha512::digest(text.as_bytes()).to_vec(),
        other => {
            return Err(invalid(format!(
                "hash: unknown algorithm {other:?} (sha256, sha512)"
            )));
        }
    };
    Ok(digest
        .iter()
        .fold(String::with_capacity(digest.len() * 2), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        }))
}

fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut dash = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn regex(pattern: &str) -> Result<regex::Regex, Error> {
    if pattern.len() > MAX_PATTERN_LEN {
        return Err(invalid("regex: the pattern is too long"));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|e| invalid(format!("regex: {e}")))
}

/// Whether the whole string matches.
fn regex_match(text: &str, pattern: &str) -> Result<bool, Error> {
    let re = regex(pattern)?;
    Ok(re
        .find(text)
        .is_some_and(|m| m.start() == 0 && m.end() == text.len()))
}

/// The first match, or none.
fn regex_search(text: &str, pattern: &str) -> Result<Value, Error> {
    Ok(regex(pattern)?
        .find(text)
        .map_or(Value::from(()), |m| Value::from(m.as_str())))
}

fn regex_replace(text: &str, pattern: &str, replacement: &str) -> Result<String, Error> {
    Ok(regex(pattern)?.replace_all(text, replacement).into_owned())
}

/// Every match; with capture groups, the first group of each.
fn regex_findall(text: &str, pattern: &str) -> Result<Value, Error> {
    let re = regex(pattern)?;
    let found: Vec<Value> = re
        .captures_iter(text)
        .map(|c| {
            let m = c.get(1).or_else(|| c.get(0)).map_or("", |m| m.as_str());
            Value::from(m)
        })
        .collect();
    Ok(Value::from(found))
}

fn default_if_none(value: &Value, default: Value) -> Value {
    if value.is_undefined() || value.is_none() {
        default
    } else {
        value.clone()
    }
}

fn uuid() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_and_hash() {
        assert_eq!(slugify("  Hello, World! Ünïcode  "), "hello-world-n-code");
        assert_eq!(
            hash("abc", None).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(hash("abc", Some("md5")).is_err());
    }

    #[test]
    fn regexes_are_bounded() {
        assert!(regex(&"a".repeat(MAX_PATTERN_LEN + 1)).is_err());
        assert!(regex("(a{1000}){1000}").is_err(), "compiled size limit");
        assert!(regex_match("abc", "a.c").unwrap());
        assert!(!regex_match("xabc", "a.c").unwrap());
    }
}
