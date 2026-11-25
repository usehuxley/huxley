//! Embeddings: text to vectors, for search by meaning. From OpenAI, Azure
//! OpenAI, an OpenAI-compatible server or Voyage AI (Anthropic has no
//! embeddings API, so an organization sets an embeddings provider apart
//! from its AI provider).

use serde_json::{Value as Json, json};

use crate::provider::{AiError, Endpoint, Http, ProviderKind, base, failure};

/// The most texts sent in one call.
pub const MAX_BATCH: usize = 64;

/// What the texts are for (Voyage embeds them differently; others ignore it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputType {
    /// Something to be found (a ticket).
    Document,
    /// What's being looked for.
    Query,
}

/// Vectors, one per text in order, and what made them.
#[derive(Debug, Clone, PartialEq)]
pub struct Embeddings {
    pub vectors: Vec<Vec<f32>>,
    pub model: String,
    pub tokens: i64,
}

/// The model used when none is set.
pub const fn default_model(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::Openai => Some("text-embedding-3-small"),
        ProviderKind::Voyage => Some("voyage-3.5"),
        _ => None,
    }
}

/// The HTTP request embedding `inputs`.
pub(crate) fn build(
    endpoint: &Endpoint,
    key: &str,
    inputs: &[String],
    input_type: InputType,
) -> Result<Http, AiError> {
    let mut body = json!({ "model": endpoint.model, "input": inputs });
    let bearer = || vec![("authorization", format!("Bearer {key}"))];
    let (url, headers) = match endpoint.kind {
        ProviderKind::Anthropic => {
            return Err(AiError::Invalid("Anthropic has no embeddings API".into()));
        }
        ProviderKind::Openai => (
            format!(
                "{}/v1/embeddings",
                base(endpoint, "https://api.openai.com")?
            ),
            bearer(),
        ),
        ProviderKind::AzureOpenai => {
            let deployment = endpoint.deployment.as_deref().unwrap_or(&endpoint.model);
            (
                format!(
                    "{}/openai/deployments/{deployment}/embeddings?api-version={}",
                    base(endpoint, "")?,
                    crate::provider::AZURE_API_VERSION
                ),
                vec![("api-key", key.to_owned())],
            )
        }
        ProviderKind::OpenaiCompatible => (format!("{}/embeddings", base(endpoint, "")?), bearer()),
        ProviderKind::Voyage => {
            body["input_type"] = json!(match input_type {
                InputType::Document => "document",
                InputType::Query => "query",
            });
            (
                format!(
                    "{}/v1/embeddings",
                    base(endpoint, "https://api.voyageai.com")?
                ),
                bearer(),
            )
        }
    };
    Ok(Http { url, headers, body })
}

/// A successful response's vectors, in the order the texts were sent.
pub(crate) fn read(endpoint: &Endpoint, body: &Json, count: usize) -> Result<Embeddings, AiError> {
    let mut items: Vec<(usize, Vec<f32>)> = Vec::with_capacity(count);
    for (i, item) in body["data"].as_array().into_iter().flatten().enumerate() {
        let index = item["index"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(i);
        let vector: Option<Vec<f32>> = item["embedding"].as_array().map(|values| {
            values
                .iter()
                .filter_map(Json::as_f64)
                // Embeddings are small floats; f32 is what pgvector keeps.
                .map(|v| {
                    #[allow(clippy::cast_possible_truncation)]
                    let v = v as f32;
                    v
                })
                .collect()
        });
        match vector {
            Some(v) if !v.is_empty() => items.push((index, v)),
            _ => {
                return Err(AiError::Invalid(
                    "an embedding wasn't a list of numbers".into(),
                ));
            }
        }
    }
    if items.len() != count {
        return Err(AiError::Invalid(format!(
            "asked for {count} embeddings, got {}",
            items.len()
        )));
    }
    items.sort_by_key(|(i, _)| *i);
    let usage = &body["usage"];
    Ok(Embeddings {
        vectors: items.into_iter().map(|(_, v)| v).collect(),
        model: body["model"].as_str().unwrap_or(&endpoint.model).to_owned(),
        tokens: usage["total_tokens"]
            .as_i64()
            .or_else(|| usage["prompt_tokens"].as_i64())
            .unwrap_or(0),
    })
}

/// Embeds `inputs` (1 to [`MAX_BATCH`]) with the provider and `key`,
/// through `http` (the caller's client, e.g. one enforcing an egress
/// policy).
pub async fn embed(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    key: &str,
    inputs: &[String],
    input_type: InputType,
) -> Result<Embeddings, AiError> {
    if inputs.is_empty() || inputs.len() > MAX_BATCH {
        return Err(AiError::Invalid(format!(
            "1 to {MAX_BATCH} texts at a time"
        )));
    }
    if inputs.iter().any(|t| t.trim().is_empty()) {
        return Err(AiError::Invalid("a text to embed is empty".into()));
    }
    let Http { url, headers, body } = build(endpoint, key, inputs, input_type)?;
    let mut call = http
        .post(&url)
        .timeout(crate::provider::TIMEOUT)
        .json(&body);
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
    read(endpoint, &body, inputs.len())
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
    fn requests_take_each_providers_shape() {
        let texts = vec!["printer jam".to_owned()];
        let openai = build(
            &endpoint(ProviderKind::Openai),
            "k",
            &texts,
            InputType::Query,
        )
        .unwrap();
        assert_eq!(openai.url, "https://example.test/v1/embeddings");
        assert_eq!(
            openai.body,
            json!({ "model": "m", "input": ["printer jam"] })
        );
        assert!(
            openai
                .headers
                .contains(&("authorization", "Bearer k".into()))
        );

        let azure = build(
            &endpoint(ProviderKind::AzureOpenai),
            "k",
            &texts,
            InputType::Query,
        )
        .unwrap();
        assert_eq!(
            azure.url,
            "https://example.test/openai/deployments/d/embeddings?api-version=2024-10-21"
        );
        assert!(azure.headers.contains(&("api-key", "k".into())));

        let compatible = build(
            &endpoint(ProviderKind::OpenaiCompatible),
            "k",
            &texts,
            InputType::Query,
        )
        .unwrap();
        assert_eq!(compatible.url, "https://example.test/embeddings");

        let voyage = build(
            &endpoint(ProviderKind::Voyage),
            "k",
            &texts,
            InputType::Document,
        )
        .unwrap();
        assert_eq!(voyage.url, "https://example.test/v1/embeddings");
        assert_eq!(voyage.body["input_type"], "document");
        let query = build(
            &endpoint(ProviderKind::Voyage),
            "k",
            &texts,
            InputType::Query,
        )
        .unwrap();
        assert_eq!(query.body["input_type"], "query");

        assert!(
            build(
                &endpoint(ProviderKind::Anthropic),
                "k",
                &texts,
                InputType::Query
            )
            .is_err()
        );
    }

    #[test]
    fn vectors_come_back_in_the_order_sent() {
        let body = json!({
            "model": "text-embedding-3-small",
            "data": [
                { "index": 1, "embedding": [0.5, 0.25] },
                { "index": 0, "embedding": [1.0, 0.0] },
            ],
            "usage": { "prompt_tokens": 7, "total_tokens": 7 },
        });
        let e = read(&endpoint(ProviderKind::Openai), &body, 2).unwrap();
        assert_eq!(e.vectors, vec![vec![1.0, 0.0], vec![0.5, 0.25]]);
        assert_eq!((e.model.as_str(), e.tokens), ("text-embedding-3-small", 7));
        // Too few, or not numbers, is an error.
        assert!(read(&endpoint(ProviderKind::Openai), &body, 3).is_err());
        let bad = json!({ "data": [{ "embedding": "nope" }] });
        assert!(read(&endpoint(ProviderKind::Openai), &bad, 1).is_err());
    }
}
