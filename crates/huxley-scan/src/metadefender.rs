//! OPSWAT MetaDefender, Core or Cloud: `POST {url}/file` with the file as
//! the body returns a `data_id`; `GET {url}/file/{data_id}` is asked until
//! `process_info.progress_percentage` reaches 100.
//!
//! Only part of the result format is confirmed from OPSWAT's documentation
//! (`scan_results.scan_all_result_i`: 0 no threat, 255 still scanning, 253
//! not scanned because of a rate limit). So anything else counts as not
//! clean: an unknown answer quarantines the file rather than passing it.

use std::time::Duration;

use reqwest::StatusCode;
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::Value as Json;

use crate::{MetaDefenderConfig, ScanError, Verdict, secs};

/// No threat found.
const NO_THREAT: i64 = 0;
/// Not scanned: the API key's rate limit was reached.
const RATE_LIMITED: i64 = 253;
/// Still being scanned.
const IN_PROGRESS: i64 = 255;

#[derive(Debug, Clone)]
pub struct MetaDefender {
    http: reqwest::Client,
    url: String,
    api_key: Option<SecretString>,
    rule: Option<String>,
    timeout: Duration,
    poll: Duration,
}

/// What one result says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Analysis {
    Pending,
    Done(Verdict),
    /// Scanning didn't happen (e.g. a rate limit); try later.
    Retry(String),
}

impl MetaDefender {
    pub(crate) fn new(config: &MetaDefenderConfig) -> Result<Self, ScanError> {
        let url = config.url.trim().trim_end_matches('/').to_owned();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(ScanError(format!(
                "uploads.scan.metadefender.url: expected an http(s) URL, got {url:?}"
            )));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| ScanError(format!("couldn't build the HTTP client: {e}")))?;
        Ok(Self {
            http,
            url,
            api_key: config.api_key.clone(),
            rule: config.rule.clone().filter(|r| !r.trim().is_empty()),
            timeout: secs(config.timeout_seconds),
            poll: Duration::from_millis(config.poll_milliseconds.clamp(100, 60_000)),
        })
    }

    fn with_key(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => request.header("apikey", key.expose_secret()),
            None => request,
        }
    }

    async fn json(&self, request: reqwest::RequestBuilder, what: &str) -> Result<Json, ScanError> {
        let response =
            self.with_key(request).send().await.map_err(|e| {
                ScanError(format!("MetaDefender {what} failed: {}", without_url(&e)))
            })?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ScanError(format!(
                "MetaDefender refused the API key ({status})"
            )));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(ScanError("MetaDefender's rate limit was reached".into()));
        }
        if !status.is_success() {
            return Err(ScanError(format!("MetaDefender {what} answered {status}")));
        }
        response
            .json()
            .await
            .map_err(|e| ScanError(format!("MetaDefender {what} wasn't JSON: {e}")))
    }

    pub(crate) async fn scan(&self, data: &[u8], filename: &str) -> Result<Verdict, ScanError> {
        let mut submit = self
            .http
            .post(format!("{}/file", self.url))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header("filename", header_safe(filename))
            .body(data.to_vec());
        if let Some(rule) = &self.rule {
            submit = submit.header("rule", rule);
        }
        let submitted = self.json(submit, "upload").await?;
        let id = submitted
            .get("data_id")
            .and_then(Json::as_str)
            .filter(|id| {
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
            .ok_or_else(|| ScanError("MetaDefender didn't return a data_id".into()))?
            .to_owned();
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let result = self
                .json(self.http.get(format!("{}/file/{id}", self.url)), "result")
                .await?;
            match interpret(&result) {
                Analysis::Done(verdict) => return Ok(verdict),
                Analysis::Retry(why) => return Err(ScanError(why)),
                Analysis::Pending => {}
            }
            if tokio::time::Instant::now() + self.poll > deadline {
                return Err(ScanError(format!(
                    "MetaDefender hadn't finished after {} seconds",
                    self.timeout.as_secs()
                )));
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Core answers `GET /version`; Cloud describes the key at `GET /apikey/`.
    pub(crate) async fn check(&self) -> Result<String, ScanError> {
        let cloud = self.url.contains("metadefender.com");
        let path = if cloud { "apikey/" } else { "version" };
        let answer = self
            .json(self.http.get(format!("{}/{path}", self.url)), "check")
            .await?;
        Ok(answer.get("version").and_then(Json::as_str).map_or_else(
            || {
                if cloud {
                    "MetaDefender Cloud".to_owned()
                } else {
                    "MetaDefender".to_owned()
                }
            },
            |v| format!("MetaDefender {v}"),
        ))
    }
}

/// Reads a `GET /file/{data_id}` result.
pub fn interpret(result: &Json) -> Analysis {
    let progress = result
        .pointer("/process_info/progress_percentage")
        .and_then(Json::as_i64);
    let code = result
        .pointer("/scan_results/scan_all_result_i")
        .and_then(Json::as_i64);
    if code == Some(RATE_LIMITED) {
        return Analysis::Retry("MetaDefender didn't scan it: its rate limit was reached".into());
    }
    if progress.is_some_and(|p| p < 100) || code == Some(IN_PROGRESS) {
        return Analysis::Pending;
    }
    let Some(code) = code else {
        return if progress.is_none() {
            // Not a result Huxley recognizes at all.
            Analysis::Retry("MetaDefender's result had no scan_results.scan_all_result_i".into())
        } else {
            Analysis::Done(Verdict::Infected("MetaDefender gave no scan result".into()))
        };
    };
    let result_word = result
        .pointer("/process_info/result")
        .and_then(Json::as_str);
    let described = result
        .pointer("/scan_results/scan_all_result_a")
        .and_then(Json::as_str)
        .unwrap_or("not clean");
    if code == NO_THREAT && result_word.is_none_or(|w| w.eq_ignore_ascii_case("allowed")) {
        return Analysis::Done(Verdict::Clean);
    }
    let reason = match (result_word, code) {
        (Some(word), NO_THREAT) => format!("MetaDefender: {word}"),
        _ => format!("{described} (MetaDefender result {code})"),
    };
    Analysis::Done(Verdict::Infected(reason))
}

/// A filename that's safe as a header value (printable ASCII).
fn header_safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .take(200)
        .collect();
    if safe.trim().is_empty() {
        "upload".into()
    } else {
        safe
    }
}

/// A request error without its URL (which may include a Core host's
/// details); the cause is enough.
fn without_url(e: &reqwest::Error) -> String {
    let mut message = if e.is_timeout() {
        "timed out".to_owned()
    } else if e.is_connect() {
        "couldn't connect".to_owned()
    } else {
        "request error".to_owned()
    };
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        message = format!("{message}: {s}");
        source = s.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn result(progress: i64, code: i64, word: Option<&str>) -> Json {
        let mut r = json!({
            "data_id": "abc",
            "process_info": { "progress_percentage": progress },
            "scan_results": { "scan_all_result_i": code, "scan_all_result_a": "Infected" }
        });
        if let Some(w) = word {
            r["process_info"]["result"] = json!(w);
        }
        r
    }

    #[test]
    fn results() {
        assert_eq!(interpret(&result(40, 255, None)), Analysis::Pending);
        assert_eq!(interpret(&result(100, 255, None)), Analysis::Pending);
        assert_eq!(
            interpret(&result(100, 0, None)),
            Analysis::Done(Verdict::Clean)
        );
        assert_eq!(
            interpret(&result(100, 0, Some("Allowed"))),
            Analysis::Done(Verdict::Clean)
        );
        assert!(matches!(
            interpret(&result(100, 253, None)),
            Analysis::Retry(_)
        ));
        // A threat, an unknown code, or a block by a workflow rule: not clean.
        assert!(
            matches!(interpret(&result(100, 1, Some("Blocked"))), Analysis::Done(Verdict::Infected(r)) if r.contains("result 1"))
        );
        assert!(matches!(
            interpret(&result(100, 42, None)),
            Analysis::Done(Verdict::Infected(_))
        ));
        assert!(
            matches!(interpret(&result(100, 0, Some("Blocked"))), Analysis::Done(Verdict::Infected(r)) if r.contains("Blocked"))
        );
        // Something that isn't a result at all: retried, never clean.
        assert!(matches!(
            interpret(&json!({ "error": "x" })),
            Analysis::Retry(_)
        ));
    }

    #[test]
    fn filenames_fit_in_headers() {
        assert_eq!(header_safe("report é.pdf"), "report _.pdf");
        assert_eq!(header_safe("a\r\nb"), "a__b");
        assert_eq!(header_safe(""), "upload");
    }
}
