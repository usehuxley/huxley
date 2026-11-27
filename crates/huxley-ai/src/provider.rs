//! Calling AI providers over HTTP: one request shape, four providers.
//! Anthropic's Messages API; OpenAI's Chat Completions (also Azure OpenAI
//! and OpenAI-compatible servers). Structured output uses each provider's
//! JSON-schema mode.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use utoipa::ToSchema;

/// Anthropic API version sent with every request.
const ANTHROPIC_VERSION: &str = "2026-10-21";
/// Server-side refusal fallback (the API reroutes declined requests).
const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
pub(crate) const AZURE_API_VERSION: &str = "2024-10-21";
/// How long one call may take.
pub const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    AzureOpenai,
    OpenaiCompatible,
    /// Voyage AI: embeddings only.
    Voyage,
}

impl ProviderKind {
    /// The model used when none is set.
    pub const fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("claude-opus-5-5"),
            // OpenAI-style providers name their models (or deployments).
            _ => None,
        }
    }
}

/// Where and how to call a provider (its key is passed separately).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub kind: ProviderKind,
    pub model: String,
    /// The API's base URL (default: the provider's own). Required for
    /// Azure OpenAI (`https://<resource>.openai.azure.com`) and
    /// OpenAI-compatible servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Azure OpenAI: the deployment's name (default: the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
}

/// One request: instructions, the input, and optionally a JSON Schema the
/// answer must follow.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub system: Option<String>,
    pub prompt: String,
    pub schema: Option<Json>,
    pub max_tokens: u32,
    /// A file shown with the prompt (a photo or a PDF).
    pub attachment: Option<Attachment>,
}

/// A file sent with a request: its media type and base64 content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// e.g. `image/jpeg` or `application/pdf`.
    pub media_type: String,
    pub base64: String,
}

impl Attachment {
    fn is_pdf(&self) -> bool {
        self.media_type == "application/pdf"
    }

    /// Anthropic content: the file, then the prompt.
    fn anthropic(&self, prompt: &str) -> Json {
        let kind = if self.is_pdf() { "document" } else { "image" };
        json!([
            { "type": kind, "source": { "type": "base64", "media_type": self.media_type, "data": self.base64 } },
            { "type": "text", "text": prompt }
        ])
    }

    /// OpenAI-style content: the file, then the prompt.
    fn openai(&self, prompt: &str) -> Json {
        let url = format!("data:{};base64,{}", self.media_type, self.base64);
        let file = if self.is_pdf() {
            json!({ "type": "file", "file": { "filename": "file.pdf", "file_data": url } })
        } else {
            json!({ "type": "image_url", "image_url": { "url": url } })
        };
        json!([file, { "type": "text", "text": prompt }])
    }
}

/// A provider's answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Completion {
    pub text: String,
    /// The parsed answer, when a schema was given.
    pub data: Option<Json>,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum AiError {
    #[error("the model declined: {0}")]
    Refused(String),
    #[error("the answer was cut off at the token limit")]
    Truncated,
    #[error("{provider} answered {status}: {message}")]
    Provider {
        provider: &'static str,
        status: u16,
        message: String,
        /// Worth trying again later (rate limited, overloaded, a server error).
        retryable: bool,
    },
    #[error("couldn't reach the provider: {0}")]
    Network(String),
    #[error("{0}")]
    Invalid(String),
    #[error("no AI provider is set up for this organization (or above it)")]
    NotConfigured,
    #[error("no embeddings provider is set up for this organization (or above it)")]
    NoEmbeddings,
    #[error("the AI provider's monthly budget is used up ({used} of {budget} tokens)")]
    OverBudget { used: i64, budget: i64 },
    #[error("{0}")]
    Internal(String),
}

fn name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "Anthropic",
        ProviderKind::Openai => "OpenAI",
        ProviderKind::AzureOpenai => "Azure OpenAI",
        ProviderKind::OpenaiCompatible => "the AI provider",
        ProviderKind::Voyage => "Voyage AI",
    }
}

pub(crate) fn base(endpoint: &Endpoint, default: &str) -> Result<String, AiError> {
    let url = endpoint
        .base_url
        .as_deref()
        .unwrap_or(default)
        .trim_end_matches('/');
    if url.is_empty() {
        return Err(AiError::Invalid(format!(
            "{} needs a base URL",
            name(endpoint.kind)
        )));
    }
    Ok(url.to_owned())
}

/// An HTTP request to a provider.
pub(crate) struct Http {
    pub(crate) url: String,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: Json,
}

/// The HTTP request for `request`.
pub(crate) fn build(endpoint: &Endpoint, key: &str, request: &Request) -> Result<Http, AiError> {
    let max_tokens = if request.max_tokens == 0 {
        16_000
    } else {
        request.max_tokens
    };
    match endpoint.kind {
        ProviderKind::Anthropic => {
            let mut body = json!({
                "model": endpoint.model,
                "max_tokens": max_tokens,
                "messages": [{ "role": "user", "content": request.attachment.as_ref()
                    .map_or_else(|| json!(request.prompt), |a| a.anthropic(&request.prompt)) }],
                // Declined requests are retried on a fallback model.
                "fallbacks": "default",
            });
            if let Some(system) = &request.system {
                body["system"] = json!(system);
            }
            if let Some(schema) = &request.schema {
                body["output_config"] =
                    json!({ "format": { "type": "json_schema", "schema": schema } });
            }
            let url = format!(
                "{}/v1/messages",
                base(endpoint, "https://api.anthropic.com")?
            );
            let headers = vec![
                ("x-api-key", key.to_owned()),
                ("anthropic-version", ANTHROPIC_VERSION.to_owned()),
                ("anthropic-beta", ANTHROPIC_FALLBACK_BETA.to_owned()),
            ];
            Ok(Http { url, headers, body })
        }
        ProviderKind::Voyage => Err(AiError::Invalid("Voyage AI only makes embeddings".into())),
        kind => {
            let mut messages = Vec::new();
            if let Some(system) = &request.system {
                messages.push(json!({ "role": "system", "content": system }));
            }
            let content = request
                .attachment
                .as_ref()
                .map_or_else(|| json!(request.prompt), |a| a.openai(&request.prompt));
            messages.push(json!({ "role": "user", "content": content }));
            let mut body = json!({ "model": endpoint.model, "messages": messages, "max_completion_tokens": max_tokens });
            if let Some(schema) = &request.schema {
                body["response_format"] = json!({ "type": "json_schema", "json_schema": { "name": "answer", "schema": schema, "strict": true } });
            }
            let (url, headers) = match kind {
                ProviderKind::AzureOpenai => {
                    let deployment = endpoint.deployment.as_deref().unwrap_or(&endpoint.model);
                    let url = format!(
                        "{}/openai/deployments/{deployment}/chat/completions?api-version={AZURE_API_VERSION}",
                        base(endpoint, "")?
                    );
                    (url, vec![("api-key", key.to_owned())])
                }
                ProviderKind::Openai => (
                    format!(
                        "{}/v1/chat/completions",
                        base(endpoint, "https://api.openai.com")?
                    ),
                    vec![("authorization", format!("Bearer {key}"))],
                ),
                _ => (
                    format!("{}/chat/completions", base(endpoint, "")?),
                    vec![("authorization", format!("Bearer {key}"))],
                ),
            };
            Ok(Http { url, headers, body })
        }
    }
}

/// The error a failed HTTP response means.
pub(crate) fn failure(kind: ProviderKind, status: u16, body: &Json) -> AiError {
    let message = body["error"]["message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("no details")
        .to_owned();
    AiError::Provider {
        provider: name(kind),
        status,
        message,
        // 429 rate limited, 5xx server errors, 529 overloaded (Anthropic).
        retryable: status == 429 || status >= 500,
    }
}

fn parse_data(text: &str, wanted: bool) -> Result<Option<Json>, AiError> {
    if !wanted {
        return Ok(None);
    }
    serde_json::from_str(text)
        .map(Some)
        .map_err(|e| AiError::Invalid(format!("the answer wasn't the JSON asked for: {e}")))
}

/// A successful response's completion.
fn read(endpoint: &Endpoint, body: &Json, wanted: bool) -> Result<Completion, AiError> {
    if endpoint.kind == ProviderKind::Anthropic {
        {
            match body["stop_reason"].as_str() {
                Some("refusal") => {
                    let why = body["stop_details"]["explanation"]
                        .as_str()
                        .unwrap_or("no reason given");
                    return Err(AiError::Refused(why.to_owned()));
                }
                Some("max_tokens") => return Err(AiError::Truncated),
                _ => {}
            }
            let text: String = body["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect();
            Ok(Completion {
                data: parse_data(&text, wanted)?,
                text,
                model: body["model"].as_str().unwrap_or(&endpoint.model).to_owned(),
                input_tokens: body["usage"]["input_tokens"].as_i64().unwrap_or(0),
                output_tokens: body["usage"]["output_tokens"].as_i64().unwrap_or(0),
            })
        }
    } else {
        {
            let choice = &body["choices"][0];
            match choice["finish_reason"].as_str() {
                Some("length") => return Err(AiError::Truncated),
                Some("content_filter") => {
                    return Err(AiError::Refused("the provider's content filter".into()));
                }
                _ => {}
            }
            if let Some(refusal) = choice["message"]["refusal"].as_str() {
                return Err(AiError::Refused(refusal.to_owned()));
            }
            let text = choice["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            Ok(Completion {
                data: parse_data(&text, wanted)?,
                text,
                model: body["model"].as_str().unwrap_or(&endpoint.model).to_owned(),
                input_tokens: body["usage"]["prompt_tokens"].as_i64().unwrap_or(0),
                output_tokens: body["usage"]["completion_tokens"].as_i64().unwrap_or(0),
            })
        }
    }
}

/// Sends `request` to the provider with `key`, through `http` (the
/// caller's client, e.g. one that enforces an egress policy).
pub async fn complete(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    key: &str,
    request: &Request,
) -> Result<Completion, AiError> {
    if request.prompt.trim().is_empty() {
        return Err(AiError::Invalid("the prompt is empty".into()));
    }
    let Http { url, headers, body } = build(endpoint, key, request)?;
    let mut call = http.post(&url).timeout(TIMEOUT).json(&body);
    for (name, value) in headers {
        call = call.header(name, value);
    }
    let response = call
        .send()
        .await
        .map_err(|e| AiError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    let body: Json = response.json().await.unwrap_or(Json::Null);
    if !(200..300).contains(&status) {
        return Err(failure(endpoint.kind, status, &body));
    }
    read(endpoint, &body, request.schema.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(kind: ProviderKind) -> Endpoint {
        Endpoint {
            kind,
            model: "m".into(),
            base_url: Some("https://example.test".into()),
            deployment: Some("d".into()),
        }
    }

    #[test]
    fn attachments_go_before_the_prompt() {
        let request = Request {
            prompt: "Read it".into(),
            attachment: Some(Attachment {
                media_type: "application/pdf".into(),
                base64: "JVBERg==".into(),
            }),
            ..Request::default()
        };
        let anthropic = build(&endpoint(ProviderKind::Anthropic), "k", &request)
            .unwrap()
            .body;
        let content = &anthropic["messages"][0]["content"];
        assert_eq!(content[0]["type"], "document");
        assert_eq!(content[0]["source"]["data"], "JVBERg==");
        assert_eq!(content[1]["text"], "Read it");
        let photo = Request {
            attachment: Some(Attachment {
                media_type: "image/png".into(),
                base64: "iVBO".into(),
            }),
            ..request
        };
        let openai = build(&endpoint(ProviderKind::Openai), "k", &photo)
            .unwrap()
            .body;
        let content = &openai["messages"][0]["content"];
        assert_eq!(content[0]["image_url"]["url"], "data:image/png;base64,iVBO");
    }

    #[test]
    fn requests_take_each_providers_shape() {
        let request = Request {
            system: Some("Be brief.".into()),
            prompt: "Hi".into(),
            schema: Some(json!({ "type": "object" })),
            max_tokens: 0,
            attachment: None,
        };
        let Http { url, headers, body } =
            build(&endpoint(ProviderKind::Anthropic), "k", &request).unwrap();
        assert_eq!(url, "https://example.test/v1/messages");
        assert!(headers.contains(&("x-api-key", "k".into())));
        assert!(headers.contains(&("anthropic-version", "2026-10-21".into())));
        assert_eq!(body["system"], "Be brief.");
        assert_eq!(body["max_tokens"], 16_000);
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");

        let Http { url, headers, body } =
            build(&endpoint(ProviderKind::AzureOpenai), "k", &request).unwrap();
        assert_eq!(
            url,
            "https://example.test/openai/deployments/d/chat/completions?api-version=2024-10-21"
        );
        assert!(headers.contains(&("api-key", "k".into())));
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);

        let Http { url, headers, .. } =
            build(&endpoint(ProviderKind::OpenaiCompatible), "k", &request).unwrap();
        assert_eq!(url, "https://example.test/chat/completions");
        assert!(headers.contains(&("authorization", "Bearer k".into())));
        let mut no_base = endpoint(ProviderKind::AzureOpenai);
        no_base.base_url = None;
        assert!(build(&no_base, "k", &request).is_err());
    }

    #[test]
    fn answers_refusals_and_errors_are_read() {
        let anthropic = endpoint(ProviderKind::Anthropic);
        let ok = json!({ "model": "claude-opus-5-5", "stop_reason": "end_turn",
                         "content": [{ "type": "thinking", "thinking": "" }, { "type": "text", "text": "{\"label\":\"spam\"}" }],
                         "usage": { "input_tokens": 12, "output_tokens": 5 } });
        let done = read(&anthropic, &ok, true).unwrap();
        assert_eq!(done.data, Some(json!({ "label": "spam" })));
        assert_eq!((done.input_tokens, done.output_tokens), (12, 5));
        let refused = json!({ "stop_reason": "refusal", "stop_details": { "explanation": "no" }, "content": [] });
        assert_eq!(
            read(&anthropic, &refused, false),
            Err(AiError::Refused("no".into()))
        );
        assert_eq!(
            read(&anthropic, &json!({ "stop_reason": "max_tokens" }), false),
            Err(AiError::Truncated)
        );

        let openai = endpoint(ProviderKind::Openai);
        let ok = json!({ "model": "gpt", "choices": [{ "finish_reason": "stop", "message": { "content": "hello" } }],
                         "usage": { "prompt_tokens": 3, "completion_tokens": 1 } });
        assert_eq!(read(&openai, &ok, false).unwrap().text, "hello");
        assert!(matches!(
            read(
                &openai,
                &json!({ "choices": [{ "message": { "content": "not json" } }] }),
                true
            ),
            Err(AiError::Invalid(_))
        ));

        let busy = failure(
            ProviderKind::Anthropic,
            529,
            &json!({ "error": { "message": "Overloaded" } }),
        );
        assert!(matches!(
            busy,
            AiError::Provider {
                retryable: true,
                ..
            }
        ));
        let bad = failure(
            ProviderKind::Openai,
            401,
            &json!({ "error": { "message": "bad key" } }),
        );
        assert!(matches!(
            bad,
            AiError::Provider {
                retryable: false,
                status: 401,
                ..
            }
        ));
    }
}
