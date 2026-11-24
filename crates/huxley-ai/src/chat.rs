//! Multi-turn chat with tools: the assistant's loop (design doc §10.3).
//! One conversation shape for every provider. Each assistant turn keeps
//! the provider's own message as it came back, and it is sent back
//! unchanged (Anthropic requires its content, thinking included, to be
//! returned as is).

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::provider::{AiError, Endpoint, Http, ProviderKind, TIMEOUT, failure};

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema of its input.
    pub input_schema: Json,
}

/// A call the model asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Json,
}

/// What a tool call returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    pub is_error: bool,
}

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        text: String,
    },
    Assistant {
        text: String,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        /// The provider's message as it came back, sent back unchanged.
        raw: Json,
    },
    /// Every result of one assistant turn's calls, together.
    ToolResults {
        results: Vec<ToolResult>,
    },
}

/// One assistant turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// The assistant's message (append it to the conversation).
    pub message: Message,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

impl Turn {
    pub fn tool_calls(&self) -> &[ToolCall] {
        match &self.message {
            Message::Assistant { tool_calls, .. } => tool_calls,
            _ => &[],
        }
    }
}

fn anthropic_messages(messages: &[Message]) -> Vec<Json> {
    messages
        .iter()
        .map(|m| match m {
            Message::User { text } => json!({ "role": "user", "content": text }),
            Message::Assistant { raw, .. } => json!({ "role": "assistant", "content": raw }),
            Message::ToolResults { results } => json!({
                "role": "user",
                "content": results.iter().map(|r| json!({
                    "type": "tool_result", "tool_use_id": r.id, "content": r.content, "is_error": r.is_error
                })).collect::<Vec<_>>()
            }),
        })
        .collect()
}

fn openai_messages(system: Option<&str>, messages: &[Message]) -> Vec<Json> {
    let mut out = Vec::new();
    if let Some(system) = system {
        out.push(json!({ "role": "system", "content": system }));
    }
    for m in messages {
        match m {
            Message::User { text } => out.push(json!({ "role": "user", "content": text })),
            Message::Assistant { raw, .. } => out.push(raw.clone()),
            Message::ToolResults { results } => {
                out.extend(results.iter().map(
                    |r| json!({ "role": "tool", "tool_call_id": r.id, "content": r.content }),
                ));
            }
        }
    }
    out
}

/// The URL, headers and body of a chat request.
fn build(
    endpoint: &Endpoint,
    key: &str,
    system: Option<&str>,
    messages: &[Message],
    tools: &[ToolDef],
    max_tokens: u32,
) -> Result<Http, AiError> {
    // The plain request builder gives the URL and headers.
    let probe = crate::provider::Request {
        system: system.map(str::to_owned),
        prompt: "-".into(),
        schema: None,
        max_tokens,
    };
    let http = crate::provider::build(endpoint, key, &probe)?;
    let mut body = http.body;
    if endpoint.kind == ProviderKind::Anthropic {
        {
            body["messages"] = json!(anthropic_messages(messages));
            if !tools.is_empty() {
                body["tools"] = json!(tools
                    .iter()
                    .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.input_schema }))
                    .collect::<Vec<_>>());
            }
        }
    } else {
        {
            body["messages"] = json!(openai_messages(system, messages));
            if !tools.is_empty() {
                body["tools"] = json!(tools
                    .iter()
                    .map(|t| json!({ "type": "function", "function": {
                        "name": t.name, "description": t.description, "parameters": t.input_schema } }))
                    .collect::<Vec<_>>());
            }
        }
    }
    Ok(Http {
        url: http.url,
        headers: http.headers,
        body,
    })
}

/// The assistant turn in a successful response.
fn read(endpoint: &Endpoint, body: &Json) -> Result<Turn, AiError> {
    if endpoint.kind == ProviderKind::Anthropic {
        if body["stop_reason"] == "refusal" {
            let why = body["stop_details"]["explanation"]
                .as_str()
                .unwrap_or("no reason given");
            return Err(AiError::Refused(why.to_owned()));
        }
        let content = body["content"].as_array().cloned().unwrap_or_default();
        let text: String = content
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect();
        let tool_calls = content
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| ToolCall {
                id: b["id"].as_str().unwrap_or_default().to_owned(),
                name: b["name"].as_str().unwrap_or_default().to_owned(),
                input: b["input"].clone(),
            })
            .collect();
        return Ok(Turn {
            message: Message::Assistant {
                text,
                tool_calls,
                raw: Json::Array(content),
            },
            model: body["model"].as_str().unwrap_or(&endpoint.model).to_owned(),
            input_tokens: body["usage"]["input_tokens"].as_i64().unwrap_or(0),
            output_tokens: body["usage"]["output_tokens"].as_i64().unwrap_or(0),
        });
    }
    let choice = &body["choices"][0];
    if choice["finish_reason"] == "content_filter" {
        return Err(AiError::Refused("the provider's content filter".into()));
    }
    let message = &choice["message"];
    let tool_calls = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| ToolCall {
            id: c["id"].as_str().unwrap_or_default().to_owned(),
            name: c["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            // Arguments arrive as JSON text.
            input: c["function"]["arguments"]
                .as_str()
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or(json!({})),
        })
        .collect();
    let mut raw = message.clone();
    raw["role"] = json!("assistant");
    Ok(Turn {
        message: Message::Assistant {
            text: message["content"].as_str().unwrap_or_default().to_owned(),
            tool_calls,
            raw,
        },
        model: body["model"].as_str().unwrap_or(&endpoint.model).to_owned(),
        input_tokens: body["usage"]["prompt_tokens"].as_i64().unwrap_or(0),
        output_tokens: body["usage"]["completion_tokens"].as_i64().unwrap_or(0),
    })
}

/// Asks the provider for the next assistant turn of `messages`.
pub async fn turn(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    key: &str,
    system: Option<&str>,
    messages: &[Message],
    tools: &[ToolDef],
    max_tokens: u32,
) -> Result<Turn, AiError> {
    let Http { url, headers, body } = build(endpoint, key, system, messages, tools, max_tokens)?;
    let mut call = http.post(&url).timeout(TIMEOUT).json(&body);
    for (header, value) in headers {
        call = call.header(header, value);
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
    read(endpoint, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(kind: ProviderKind) -> Endpoint {
        Endpoint {
            kind,
            model: "m".into(),
            base_url: Some("https://x.test".into()),
            deployment: None,
        }
    }

    fn conversation() -> Vec<Message> {
        vec![
            Message::User {
                text: "List my workflows".into(),
            },
            Message::Assistant {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "t1".into(),
                    name: "list_workflows".into(),
                    input: json!({}),
                }],
                raw: json!([{ "type": "thinking", "thinking": "", "signature": "sig" },
                             { "type": "tool_use", "id": "t1", "name": "list_workflows", "input": {} }]),
            },
            Message::ToolResults {
                results: vec![ToolResult {
                    id: "t1".into(),
                    content: "[]".into(),
                    is_error: false,
                }],
            },
        ]
    }

    #[test]
    fn anthropic_turns_are_sent_back_as_they_came() {
        let tools = vec![ToolDef {
            name: "list_workflows".into(),
            description: "d".into(),
            input_schema: json!({ "type": "object" }),
        }];
        let Http { body, .. } = build(
            &endpoint(ProviderKind::Anthropic),
            "k",
            Some("sys"),
            &conversation(),
            &tools,
            0,
        )
        .unwrap();
        // The thinking block goes back unchanged, before the tool use.
        assert_eq!(body["messages"][1]["content"][0]["signature"], "sig");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "t1");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["system"], "sys");

        let response = json!({ "model": "claude-opus-5-5", "stop_reason": "tool_use", "usage": { "input_tokens": 5, "output_tokens": 2 },
            "content": [{ "type": "text", "text": "Checking." }, { "type": "tool_use", "id": "t2", "name": "get_run", "input": { "id": "r" } }] });
        let turn = read(&endpoint(ProviderKind::Anthropic), &response).unwrap();
        assert_eq!(turn.tool_calls()[0].name, "get_run");
        assert!(matches!(&turn.message, Message::Assistant { text, .. } if text == "Checking."));
    }

    #[test]
    fn openai_tool_calls_map_both_ways() {
        let mut messages = conversation();
        messages[1] = Message::Assistant {
            text: String::new(),
            tool_calls: vec![],
            raw: json!({ "role": "assistant", "tool_calls": [{ "id": "t1", "type": "function",
                         "function": { "name": "list_workflows", "arguments": "{}" } }] }),
        };
        let Http { body, .. } = build(
            &endpoint(ProviderKind::Openai),
            "k",
            Some("sys"),
            &messages,
            &[],
            0,
        )
        .unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][2]["tool_calls"][0]["id"], "t1");
        assert_eq!(
            body["messages"][3],
            json!({ "role": "tool", "tool_call_id": "t1", "content": "[]" })
        );

        let response = json!({ "model": "gpt", "choices": [{ "finish_reason": "tool_calls", "message": { "content": null,
            "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "get_run", "arguments": "{\"id\":\"r\"}" } }] } }] });
        let turn = read(&endpoint(ProviderKind::Openai), &response).unwrap();
        assert_eq!(turn.tool_calls()[0].input, json!({ "id": "r" }));
    }
}
