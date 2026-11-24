//! The `ai.prompt`, `ai.classify` and `ai.extract` steps (design doc
//! §10.1): they call the run's organization's AI provider (its own or the
//! nearest above), and every call is logged against the run. Input that
//! carries secrets is never sent to a provider.

use huxley_ai::{AiError, Purpose, Request};
use serde_json::{Map, Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

/// Most labels a classification may have.
const MAX_LABELS: usize = 200;

fn failure(e: &AiError) -> Json {
    let mut failure = error("ai", e.to_string());
    if let AiError::Provider {
        retryable, status, ..
    } = e
    {
        failure["retryable"] = json!(retryable);
        failure["status"] = json!(status);
    }
    failure
}

fn text_of<'a>(input: &'a Json, field: &str) -> Result<&'a str, Json> {
    input[field]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| error("input", format!("{field} is required")))
}

/// `schema` with `additionalProperties: false` on every object (providers'
/// strict modes require it), and objects' properties all required.
fn strict(schema: &Json) -> Json {
    match schema {
        Json::Object(map) => {
            let mut out: Map<String, Json> =
                map.iter().map(|(k, v)| (k.clone(), strict(v))).collect();
            if out.get("type") == Some(&json!("object")) {
                out.entry("additionalProperties").or_insert(json!(false));
                if let Some(Json::Object(properties)) = out.get("properties") {
                    let names: Vec<Json> = properties.keys().map(|k| json!(k)).collect();
                    out.entry("required").or_insert(Json::Array(names));
                }
            }
            Json::Object(out)
        }
        Json::Array(items) => Json::Array(items.iter().map(strict).collect()),
        other => other.clone(),
    }
}

fn max_tokens(input: &Json) -> Result<u32, Json> {
    match &input["max_tokens"] {
        Json::Null => Ok(0),
        value => value
            .as_u64()
            .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=64_000).contains(n))
            .ok_or_else(|| error("input", "max_tokens is 1 to 64000")),
    }
}

impl Engine {
    /// Runs an AI step (`kind` is its type) for run `run_id` of `org_id`.
    pub(crate) async fn ai_step(
        &self,
        kind: &str,
        org_id: Uuid,
        run_id: Uuid,
        input: &Json,
        tainted: bool,
    ) -> Result<Json, Json> {
        // Secrets (and values derived from them) don't go to a third party.
        if tainted {
            return Err(error(
                "input",
                "AI steps can't be sent secrets or values derived from them",
            ));
        }
        let model = input["model"].as_str();
        let request = match kind {
            "ai.prompt" => Request {
                system: input["system"].as_str().map(str::to_owned),
                prompt: text_of(input, "prompt")?.to_owned(),
                schema: None,
                max_tokens: max_tokens(input)?,
            },
            "ai.classify" => {
                let labels: Vec<String> = input["labels"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                if labels.is_empty() || labels.len() > MAX_LABELS {
                    return Err(error(
                        "input",
                        format!("labels is a list of 1 to {MAX_LABELS} strings"),
                    ));
                }
                let mut system =
                    String::from("Classify the text. Answer with exactly one of the given labels.");
                if let Some(instructions) = input["instructions"].as_str() {
                    system.push_str("\n\n");
                    system.push_str(instructions);
                }
                Request {
                    system: Some(system),
                    prompt: text_of(input, "text")?.to_owned(),
                    schema: Some(json!({
                        "type": "object",
                        "properties": { "label": { "type": "string", "enum": labels } },
                        "required": ["label"],
                        "additionalProperties": false
                    })),
                    max_tokens: 0,
                }
            }
            _ => {
                let schema = &input["schema"];
                if schema["type"] != "object" {
                    return Err(error("input", "schema is a JSON Schema of an object"));
                }
                let mut system = String::from(
                    "Extract the requested data from the text. Use null for anything it doesn't say.",
                );
                if let Some(instructions) = input["instructions"].as_str() {
                    system.push_str("\n\n");
                    system.push_str(instructions);
                }
                Request {
                    system: Some(system),
                    prompt: text_of(input, "text")?.to_owned(),
                    schema: Some(strict(schema)),
                    max_tokens: 0,
                }
            }
        };
        let purpose = Purpose {
            feature: "workflow",
            run_id: Some(run_id),
        };
        let done = self
            .ai()
            .complete(org_id, purpose, model, &request)
            .await
            .map_err(|e| failure(&e))?;
        Ok(match kind {
            "ai.prompt" => json!({ "text": done.text, "model": done.model }),
            "ai.classify" => {
                json!({ "label": done.data.unwrap_or_default()["label"], "model": done.model })
            }
            _ => done.data.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_are_made_strict() {
        let schema = json!({ "type": "object", "properties": {
            "name": { "type": "string" },
            "address": { "type": "object", "properties": { "city": { "type": "string" } } } } });
        let strict = strict(&schema);
        assert_eq!(strict["additionalProperties"], false);
        // Every property is required (in whatever order the map keeps).
        let mut required: Vec<&str> = strict["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Json::as_str)
            .collect();
        required.sort_unstable();
        assert_eq!(required, ["address", "name"]);
        assert_eq!(
            strict["properties"]["address"]["additionalProperties"],
            false
        );
        // What's given stays.
        let given = json!({ "type": "object", "properties": { "a": {} }, "required": [] });
        assert_eq!(super::strict(&given)["required"], json!([]));
    }
}
