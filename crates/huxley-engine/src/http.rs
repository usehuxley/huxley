//! The `http.request` step.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use huxley_hwf::{BodyType, HttpMethod, HttpRequest};
use serde_json::{Map, Value as Json, json};

use crate::egress::{Egress, blocked_reason};
use crate::walk::error;

/// Longest response body kept in an error, for diagnosis.
const ERROR_BODY_CHARS: usize = 2000;

/// Makes the request. `Ok` holds `{status, headers, body}`; `Err` a step
/// error (`kind`: `egress`, `network`, `timeout`, `http` with `status`).
pub(crate) async fn request(egress: &Egress, input: &Json) -> Result<Json, Json> {
    let spec: HttpRequest = serde_json::from_value(input.clone())
        .map_err(|e| error("definition", format!("invalid request: {e}")))?;
    let mut url = reqwest::Url::parse(&spec.url)
        .map_err(|e| error("definition", format!("{:?} isn't a URL: {e}", spec.url)))?;
    if !spec.query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in &spec.query {
            match value {
                Json::Null => {}
                Json::String(s) => {
                    pairs.append_pair(key, s);
                }
                Json::Array(items) => {
                    for item in items {
                        pairs.append_pair(key, &scalar(item));
                    }
                }
                other => {
                    pairs.append_pair(key, &scalar(other));
                }
            }
        }
    }
    egress
        .policy
        .check_url(&url)
        .map_err(|b| error("egress", b.0))?;

    let builder = build(egress, &spec, url)?;
    let response = builder.send().await.map_err(classify)?;
    read(
        response,
        egress.max_response_bytes(),
        spec.allow_error_status,
    )
    .await
}

/// The request, with method, headers and body.
fn build(
    egress: &Egress,
    spec: &HttpRequest,
    url: reqwest::Url,
) -> Result<reqwest::RequestBuilder, Json> {
    let method = match spec.method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Patch => reqwest::Method::PATCH,
        HttpMethod::Delete => reqwest::Method::DELETE,
        HttpMethod::Head => reqwest::Method::HEAD,
        HttpMethod::Options => reqwest::Method::OPTIONS,
    };
    let mut builder = egress.client.request(method, url);
    for (name, value) in &spec.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    if let Some(body) = &spec.body {
        builder = match spec.body_type {
            BodyType::Json => builder.json(body),
            BodyType::Text => builder
                .header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(match body {
                    Json::String(s) => s.clone(),
                    other => other.to_string(),
                }),
            BodyType::Form => {
                let Json::Object(fields) = body else {
                    return Err(error("definition", "a form body must be an object"));
                };
                let encoded = fields
                    .iter()
                    .fold(
                        url::form_urlencoded::Serializer::new(String::new()),
                        |mut s, (k, v)| {
                            s.append_pair(k, &scalar(v));
                            s
                        },
                    )
                    .finish();
                builder
                    .header(
                        reqwest::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(encoded)
            }
        };
    }

    Ok(builder)
}

/// Reads the response into `{status, headers, body}`, or an error for a
/// 4xx/5xx status (unless allowed) or a body over `limit`.
async fn read(
    mut response: reqwest::Response,
    limit: usize,
    allow_error_status: bool,
) -> Result<Json, Json> {
    let status = response.status().as_u16();
    let mut headers = Map::new();
    for (name, value) in response.headers() {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        match headers.get_mut(name.as_str()) {
            Some(Json::String(existing)) => {
                existing.push_str(", ");
                existing.push_str(&value);
            }
            _ => {
                headers.insert(name.as_str().to_owned(), Json::String(value));
            }
        }
    }
    let is_json = headers
        .get("content-type")
        .and_then(Json::as_str)
        .is_some_and(|t| t.contains("json"));

    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify)? {
        if bytes.len() + chunk.len() > limit {
            return Err(error(
                "limit",
                format!("the response is larger than {limit} bytes"),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = if bytes.is_empty() {
        Json::Null
    } else if is_json {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| text_or_base64(bytes))
    } else {
        text_or_base64(bytes)
    };

    if status >= 400 && !allow_error_status {
        let mut failure = error("http", format!("the server responded {status}"));
        failure["status"] = json!(status);
        failure["body"] = match body {
            Json::String(s) => Json::String(s.chars().take(ERROR_BODY_CHARS).collect()),
            other if other.to_string().len() <= ERROR_BODY_CHARS => other,
            _ => Json::String("(too long to show)".into()),
        };
        return Err(failure);
    }
    Ok(json!({ "status": status, "headers": headers, "body": body }))
}

fn scalar(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

fn text_or_base64(bytes: Vec<u8>) -> Json {
    match String::from_utf8(bytes) {
        Ok(text) => Json::String(text),
        Err(e) => json!({ "base64": STANDARD.encode(e.into_bytes()) }),
    }
}

fn classify(e: reqwest::Error) -> Json {
    if let Some(reason) = blocked_reason(&e) {
        return error("egress", reason);
    }
    if e.is_timeout() {
        return error("timeout", "the request timed out");
    }
    let network = e.is_connect() || e.is_request();
    let message = e.without_url().to_string();
    error(if network { "network" } else { "http" }, message)
}
