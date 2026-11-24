//! Streamed chat turns: text reaches the caller as the model writes it,
//! and the turn that comes out is the one the non-streaming request gives.
//!
//! Each provider's events are folded back into the body its non-streaming
//! endpoint returns, then read by the same code (`chat::read`), so streaming
//! and not streaming can't disagree about the turn, and what's sent back to
//! the provider later is the same.

use serde_json::{Map, Value as Json, json};

use crate::provider::{Endpoint, ProviderKind};

/// Splits Server-Sent Events out of a byte stream.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    /// Bytes not yet decoded.
    pending: Vec<u8>,
    buffer: String,
}

/// One event: its `event:` name (if any) and its `data:` lines joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl SseParser {
    /// Feeds bytes; returns the events they completed.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.pending.extend_from_slice(bytes);
        // Hold back a trailing "\r" (maybe the first half of a "\r\n" split
        // across chunks) and an incomplete UTF-8 character.
        let mut take = self.pending.len();
        if self.pending.last() == Some(&b'\r') {
            take -= 1;
        }
        let complete = match std::str::from_utf8(&self.pending[..take]) {
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            _ => take,
        };
        let decoded: Vec<u8> = self.pending.drain(..complete).collect();
        // Normalize line endings so "\r\n\r\n" separates events too.
        self.buffer.push_str(
            &String::from_utf8_lossy(&decoded)
                .replace("\r\n", "\n")
                .replace('\r', "\n"),
        );
        let mut events = Vec::new();
        while let Some(end) = self.buffer.find("\n\n") {
            let raw: String = self.buffer.drain(..end + 2).collect();
            let mut event = None;
            let mut data: Vec<&str> = Vec::new();
            for line in raw.lines() {
                if let Some(value) = line.strip_prefix("event:") {
                    event = Some(value.trim().to_owned());
                } else if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.strip_prefix(' ').unwrap_or(value));
                }
            }
            if !data.is_empty() {
                events.push(SseEvent {
                    event,
                    data: data.join("\n"),
                });
            }
        }
        events
    }
}

/// What a stream said, as it goes.
pub(crate) enum Step {
    /// Text the model wrote.
    Text(String),
    /// The provider reported an error in the stream (status, body).
    Error(Json),
    Nothing,
}

/// Folds Anthropic Messages events into a Messages response body.
#[derive(Debug, Default)]
pub(crate) struct AnthropicStream {
    message: Map<String, Json>,
    blocks: Vec<Json>,
    /// Tool inputs arrive as JSON text in pieces.
    partial_json: Vec<String>,
}

impl AnthropicStream {
    pub(crate) fn event(&mut self, data: &str) -> Step {
        let Ok(event) = serde_json::from_str::<Json>(data) else {
            return Step::Nothing;
        };
        let index = event["index"]
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .unwrap_or(0);
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                if let Json::Object(message) = &event["message"] {
                    self.message.clone_from(message);
                }
            }
            "content_block_start" => {
                while self.blocks.len() <= index {
                    self.blocks.push(Json::Null);
                    self.partial_json.push(String::new());
                }
                self.blocks[index] = event["content_block"].clone();
            }
            "content_block_delta" => return self.delta(index, &event["delta"]),
            "content_block_stop" => {
                if let Some(block) = self.blocks.get_mut(index)
                    && matches!(block["type"].as_str(), Some("tool_use" | "server_tool_use"))
                {
                    let text = self.partial_json[index].trim();
                    if !text.is_empty() {
                        block["input"] = serde_json::from_str(text).unwrap_or(json!({}));
                    } else if block["input"].is_null() {
                        block["input"] = json!({});
                    }
                }
            }
            "message_delta" => {
                for (key, value) in event["delta"].as_object().into_iter().flatten() {
                    self.message.insert(key.clone(), value.clone());
                }
                // Output tokens (and any other counts) as they stand at the end.
                if let Json::Object(usage) = &event["usage"] {
                    let total = self.message.entry("usage").or_insert_with(|| json!({}));
                    for (key, value) in usage {
                        total[key] = value.clone();
                    }
                }
            }
            "error" => return Step::Error(event),
            _ => {}
        }
        Step::Nothing
    }

    fn delta(&mut self, index: usize, delta: &Json) -> Step {
        let Some(block) = self.blocks.get_mut(index) else {
            return Step::Nothing;
        };
        match delta["type"].as_str().unwrap_or_default() {
            "text_delta" => {
                let piece = delta["text"].as_str().unwrap_or_default();
                append(block, "text", piece);
                return Step::Text(piece.to_owned());
            }
            "thinking_delta" => append(
                block,
                "thinking",
                delta["thinking"].as_str().unwrap_or_default(),
            ),
            "signature_delta" => {
                block["signature"] = delta["signature"].clone();
            }
            "input_json_delta" => self.partial_json[index]
                .push_str(delta["partial_json"].as_str().unwrap_or_default()),
            "citations_delta" => {
                let citations = block
                    .as_object_mut()
                    .map(|b| b.entry("citations").or_insert_with(|| json!([])));
                if let Some(Json::Array(list)) = citations {
                    list.push(delta["citation"].clone());
                }
            }
            _ => {}
        }
        Step::Nothing
    }

    /// The response body, as the non-streaming request returns it.
    pub(crate) fn finish(mut self) -> Json {
        self.message.insert(
            "content".into(),
            Json::Array(self.blocks.into_iter().filter(|b| !b.is_null()).collect()),
        );
        Json::Object(self.message)
    }
}

fn append(block: &mut Json, field: &str, piece: &str) {
    let mut text = block[field].as_str().unwrap_or_default().to_owned();
    text.push_str(piece);
    block[field] = json!(text);
}

/// Folds OpenAI-style chat completion chunks into a completion body.
#[derive(Debug, Default)]
pub(crate) struct OpenAiStream {
    model: Option<String>,
    content: String,
    /// By index: id, name, arguments (JSON text in pieces).
    tool_calls: Vec<(String, String, String)>,
    finish_reason: Json,
    usage: Json,
}

impl OpenAiStream {
    pub(crate) fn event(&mut self, data: &str) -> Step {
        if data.trim() == "[DONE]" {
            return Step::Nothing;
        }
        let Ok(chunk) = serde_json::from_str::<Json>(data) else {
            return Step::Nothing;
        };
        if chunk.get("error").is_some() {
            return Step::Error(chunk);
        }
        if let Some(model) = chunk["model"].as_str() {
            self.model = Some(model.to_owned());
        }
        if chunk["usage"].is_object() {
            self.usage = chunk["usage"].clone();
        }
        let choice = &chunk["choices"][0];
        if !choice["finish_reason"].is_null() {
            self.finish_reason = choice["finish_reason"].clone();
        }
        let delta = &choice["delta"];
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = call["index"]
                .as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .unwrap_or(self.tool_calls.len());
            while self.tool_calls.len() <= index {
                self.tool_calls.push(Default::default());
            }
            let entry = &mut self.tool_calls[index];
            if let Some(id) = call["id"].as_str() {
                id.clone_into(&mut entry.0);
            }
            if let Some(name) = call["function"]["name"].as_str() {
                entry.1.push_str(name);
            }
            if let Some(arguments) = call["function"]["arguments"].as_str() {
                entry.2.push_str(arguments);
            }
        }
        match delta["content"].as_str() {
            Some(piece) if !piece.is_empty() => {
                self.content.push_str(piece);
                Step::Text(piece.to_owned())
            }
            _ => Step::Nothing,
        }
    }

    /// The completion body, as the non-streaming request returns it.
    pub(crate) fn finish(self, endpoint: &Endpoint) -> Json {
        let mut message = json!({
            "role": "assistant",
            "content": if self.content.is_empty() { Json::Null } else { json!(self.content) },
        });
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = Json::Array(
                self.tool_calls
                    .into_iter()
                    .map(|(id, name, arguments)| {
                        json!({ "id": id, "type": "function",
                                "function": { "name": name, "arguments": arguments } })
                    })
                    .collect(),
            );
        }
        json!({
            "model": self.model.unwrap_or_else(|| endpoint.model.clone()),
            "choices": [{ "finish_reason": self.finish_reason, "message": message }],
            "usage": self.usage,
        })
    }
}

/// Whether to ask OpenAI-style servers for usage in the stream. Other
/// OpenAI-compatible servers may not know the option.
pub(crate) fn wants_stream_usage(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::Openai | ProviderKind::AzureOpenai)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(text: &str) -> Vec<SseEvent> {
        let mut parser = SseParser::default();
        // In awkward pieces, as networks deliver them.
        let bytes = text.as_bytes();
        let mut events = Vec::new();
        for chunk in bytes.chunks(7) {
            events.extend(parser.push(chunk));
        }
        events
    }

    #[test]
    fn sse_events_survive_any_chunking() {
        let events = feed_all(
            "event: ping\ndata: {}\n\nevent: content_block_delta\r\ndata: {\"a\":\r\ndata: 1}\r\n\r\n: comment\n\ndata: \u{e9}t\u{e9} \u{1f600}\n\n",
        );
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: Some("ping".into()),
                    data: "{}".into()
                },
                SseEvent {
                    event: Some("content_block_delta".into()),
                    data: "{\"a\":\n1}".into()
                },
                // Multi-byte characters split across chunks arrive whole.
                SseEvent {
                    event: None,
                    data: "\u{e9}t\u{e9} \u{1f600}".into()
                },
            ]
        );
    }
}
