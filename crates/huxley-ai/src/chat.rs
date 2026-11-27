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
        attachment: None,
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

/// Anthropic content as it may be sent back. After a model declined
/// mid-answer and a fallback model carried on (server-side `fallbacks`; only
/// a streamed response keeps the declined partial), the blocks before the
/// last `fallback` block are sent back only if they're text, the `fallback`
/// blocks themselves (in place), or paired server-tool blocks: the declined
/// model's thinking and tool calls are left out.
fn echoable(content: Vec<Json>) -> Vec<Json> {
    let Some(boundary) = content.iter().rposition(|b| b["type"] == "fallback") else {
        return content;
    };
    let results: std::collections::BTreeSet<String> = content
        .iter()
        .filter_map(|b| b["tool_use_id"].as_str().map(str::to_owned))
        .collect();
    content
        .into_iter()
        .enumerate()
        .filter(|(i, block)| {
            *i >= boundary
                || match block["type"].as_str().unwrap_or_default() {
                    "text" | "fallback" => true,
                    "server_tool_use" => {
                        block["id"].as_str().is_some_and(|id| results.contains(id))
                    }
                    kind => kind.ends_with("_tool_result"),
                }
        })
        .map(|(_, block)| block)
        .collect()
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
        let content = echoable(body["content"].as_array().cloned().unwrap_or_default());
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

/// No bytes for this long ends a streamed turn.
const STREAM_IDLE: std::time::Duration = std::time::Duration::from_secs(120);
/// The longest a streamed turn may take in all.
const STREAM_LIMIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Like [`turn`], but streamed: `on_text` gets the text as the model writes
/// it. The turn is the same as [`turn`] gives.
#[allow(clippy::too_many_arguments)]
pub async fn turn_streaming(
    http: &reqwest::Client,
    endpoint: &Endpoint,
    key: &str,
    system: Option<&str>,
    messages: &[Message],
    tools: &[ToolDef],
    max_tokens: u32,
    on_text: &(dyn Fn(&str) + Send + Sync),
) -> Result<Turn, AiError> {
    use crate::stream::{AnthropicStream, OpenAiStream, SseParser, Step};

    let Http {
        url,
        headers,
        mut body,
    } = build(endpoint, key, system, messages, tools, max_tokens)?;
    body["stream"] = json!(true);
    if crate::stream::wants_stream_usage(endpoint.kind) {
        body["stream_options"] = json!({ "include_usage": true });
    }
    let mut call = http.post(&url).json(&body);
    for (header, value) in headers {
        call = call.header(header, value);
    }
    let mut response = tokio::time::timeout(TIMEOUT, call.send())
        .await
        .map_err(|_| AiError::Network("the provider didn't answer in time".into()))?
        .map_err(|e| AiError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body: Json = response.json().await.unwrap_or(Json::Null);
        return Err(failure(endpoint.kind, status, &body));
    }
    let anthropic = endpoint.kind == ProviderKind::Anthropic;
    let mut parser = SseParser::default();
    let mut claude = AnthropicStream::default();
    let mut openai = OpenAiStream::default();
    let deadline = tokio::time::Instant::now() + STREAM_LIMIT;
    loop {
        let idle = STREAM_IDLE.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        let chunk = tokio::time::timeout(idle, response.chunk())
            .await
            .map_err(|_| AiError::Network("the provider stopped answering".into()))?
            .map_err(|e| AiError::Network(e.to_string()))?;
        let Some(bytes) = chunk else { break };
        for event in parser.push(&bytes) {
            let step = if anthropic {
                claude.event(&event.data)
            } else {
                openai.event(&event.data)
            };
            match step {
                Step::Text(piece) => on_text(&piece),
                Step::Error(error) => return Err(failure(endpoint.kind, 500, &error)),
                Step::Nothing => {}
            }
        }
    }
    let body = if anthropic {
        claude.finish()
    } else {
        openai.finish(endpoint)
    };
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

    /// Feeds Anthropic events (as `data:` lines) through the stream fold.
    fn anthropic_stream(events: &[Json]) -> (Json, String) {
        let mut parser = crate::stream::SseParser::default();
        let mut stream = crate::stream::AnthropicStream::default();
        let mut text = String::new();
        let mut wire = String::new();
        for e in events {
            use std::fmt::Write as _;
            let _ = write!(
                wire,
                "event: {}\ndata: {e}\n\n",
                e["type"].as_str().unwrap()
            );
        }
        for chunk in wire.as_bytes().chunks(11) {
            for event in parser.push(chunk) {
                if let crate::stream::Step::Text(piece) = stream.event(&event.data) {
                    text.push_str(&piece);
                }
            }
        }
        (stream.finish(), text)
    }

    #[test]
    fn a_streamed_anthropic_turn_is_the_turn_the_plain_request_gives() {
        let (body, streamed) = anthropic_stream(&[
            json!({ "type": "message_start", "message": { "model": "claude-opus-5-5", "role": "assistant", "content": [], "usage": { "input_tokens": 40, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig-1" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Let me " } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "look." } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "t9", "name": "get_run", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"id\": \"r" } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "1\"}" } }),
            json!({ "type": "content_block_stop", "index": 2 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 25 } }),
            json!({ "type": "message_stop" }),
        ]);
        assert_eq!(streamed, "Let me look.");
        let plain = json!({ "model": "claude-opus-5-5", "stop_reason": "tool_use",
            "usage": { "input_tokens": 40, "output_tokens": 25 },
            "content": [
                { "type": "thinking", "thinking": "", "signature": "sig-1" },
                { "type": "text", "text": "Let me look." },
                { "type": "tool_use", "id": "t9", "name": "get_run", "input": { "id": "r1" } }
            ] });
        let endpoint = endpoint(ProviderKind::Anthropic);
        assert_eq!(
            read(&endpoint, &body).unwrap(),
            read(&endpoint, &plain).unwrap()
        );
    }

    #[test]
    fn after_a_mid_stream_fallback_only_text_is_kept_from_before_it() {
        let (body, _) = anthropic_stream(&[
            json!({ "type": "message_start", "message": { "model": "claude-opus-5", "content": [], "usage": { "input_tokens": 3 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "declined" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "Partial " } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "old", "name": "x", "input": {} } }),
            json!({ "type": "content_block_stop", "index": 2 }),
            json!({ "type": "content_block_start", "index": 3, "content_block": { "type": "fallback", "from": { "model": "claude-fable-5-2" }, "to": { "model": "claude-opus-5" } } }),
            json!({ "type": "content_block_stop", "index": 3 }),
            json!({ "type": "content_block_start", "index": 4, "content_block": { "type": "thinking", "thinking": "" } }),
            json!({ "type": "content_block_delta", "index": 4, "delta": { "type": "signature_delta", "signature": "kept" } }),
            json!({ "type": "content_block_stop", "index": 4 }),
            json!({ "type": "content_block_start", "index": 5, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 5, "delta": { "type": "text_delta", "text": "answer." } }),
            json!({ "type": "content_block_stop", "index": 5 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 9 } }),
        ]);
        let turn = read(&endpoint(ProviderKind::Anthropic), &body).unwrap();
        let Message::Assistant {
            text,
            tool_calls,
            raw,
        } = &turn.message
        else {
            panic!("an assistant message");
        };
        assert_eq!(text, "Partial answer.");
        assert_eq!(tool_calls, &Vec::<ToolCall>::new());
        let kinds: Vec<&str> = raw
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["text", "fallback", "thinking", "text"]);
        assert_eq!(raw[2]["signature"], "kept");
        assert_eq!(turn.model, "claude-opus-5");
    }

    #[test]
    fn a_streamed_openai_turn_is_the_turn_the_plain_request_gives() {
        let mut parser = crate::stream::SseParser::default();
        let mut stream = crate::stream::OpenAiStream::default();
        let mut streamed = String::new();
        let chunks = [
            json!({ "model": "gpt-x", "choices": [{ "index": 0, "delta": { "role": "assistant", "content": "On " } }] }),
            json!({ "model": "gpt-x", "choices": [{ "index": 0, "delta": { "content": "it." } }] }),
            json!({ "model": "gpt-x", "choices": [{ "index": 0, "delta": { "tool_calls": [{ "index": 0, "id": "c1", "type": "function", "function": { "name": "get_run", "arguments": "{\"id\":" } }] } }] }),
            json!({ "model": "gpt-x", "choices": [{ "index": 0, "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "\"r\"}" } }] } }] }),
            json!({ "model": "gpt-x", "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }] }),
            json!({ "model": "gpt-x", "choices": [], "usage": { "prompt_tokens": 12, "completion_tokens": 4 } }),
        ];
        let mut wire = String::new();
        for c in &chunks {
            use std::fmt::Write as _;
            let _ = write!(wire, "data: {c}\n\n");
        }
        wire.push_str("data: [DONE]\n\n");
        for chunk in wire.as_bytes().chunks(5) {
            for event in parser.push(chunk) {
                if let crate::stream::Step::Text(piece) = stream.event(&event.data) {
                    streamed.push_str(&piece);
                }
            }
        }
        assert_eq!(streamed, "On it.");
        let endpoint = endpoint(ProviderKind::Openai);
        let body = stream.finish(&endpoint);
        let plain = json!({ "model": "gpt-x", "usage": { "prompt_tokens": 12, "completion_tokens": 4 },
            "choices": [{ "finish_reason": "tool_calls", "message": { "role": "assistant", "content": "On it.",
                "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "get_run", "arguments": "{\"id\":\"r\"}" } }] } }] });
        assert_eq!(
            read(&endpoint, &body).unwrap(),
            read(&endpoint, &plain).unwrap()
        );
    }
}
