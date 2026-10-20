use serde_json::{Value, json};
use std::fmt::Write;

pub(crate) fn anthropic_stream() -> AnthropicStreamBuilder {
    AnthropicStreamBuilder::default().push(json!({
        "type": "message_start",
        "message": {
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": "claude-3",
            "stop_reason": null,
            "stop_sequence": null,
            "usage": usage(10, 0)
        }
    }))
}

pub(crate) fn usage(input_tokens: u32, output_tokens: u32) -> Value {
    json!({ "input_tokens": input_tokens, "output_tokens": output_tokens })
}

#[derive(Default)]
pub(crate) struct AnthropicStreamBuilder {
    events: Vec<Value>,
}

impl AnthropicStreamBuilder {
    pub(crate) fn text(self, index: u32, chunks: &[&str]) -> Self {
        chunks
            .iter()
            .fold(self.block_start(index, &json!({ "type": "text", "text": "" })), |builder, text| {
                builder.block_delta(index, &json!({ "type": "text_delta", "text": text }))
            })
            .block_stop(index)
    }

    pub(crate) fn thinking(self, index: u32, chunks: &[&str]) -> Self {
        self.thinking_deltas(index, chunks).block_stop(index)
    }

    pub(crate) fn signed_thinking(self, index: u32, chunks: &[&str], signature: &str) -> Self {
        self.thinking_deltas(index, chunks)
            .block_delta(index, &json!({ "type": "signature_delta", "signature": signature }))
            .block_stop(index)
    }

    pub(crate) fn redacted_thinking(self, index: u32, data: &str) -> Self {
        self.block_start(index, &json!({ "type": "redacted_thinking", "data": data })).block_stop(index)
    }

    pub(crate) fn tool_call(self, index: u32, id: &str, name: &str, argument_deltas: &[&str]) -> Self {
        argument_deltas
            .iter()
            .fold(self.tool_start(index, id, name), |builder, delta| builder.tool_delta(index, delta))
            .block_stop(index)
    }

    pub(crate) fn tool_start(self, index: u32, id: &str, name: &str) -> Self {
        self.block_start(index, &json!({ "type": "tool_use", "id": id, "name": name }))
    }

    pub(crate) fn tool_delta(self, index: u32, partial_json: &str) -> Self {
        self.block_delta(index, &json!({ "type": "input_json_delta", "partial_json": partial_json }))
    }

    pub(crate) fn message_delta(self, stop_reason: &str, usage: &Value) -> Self {
        self.push(json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason, "stop_sequence": null },
            "usage": usage
        }))
    }

    pub(crate) fn message_stop(self) -> Self {
        self.push(json!({ "type": "message_stop" }))
    }

    pub(crate) fn error(self, error_type: &str, message: &str) -> Self {
        self.push(json!({ "type": "error", "error": { "type": error_type, "message": message } }))
    }

    pub(crate) fn block_stop(self, index: u32) -> Self {
        self.push(json!({ "type": "content_block_stop", "index": index }))
    }

    /// Each event as its JSON payload.
    pub(crate) fn build(self) -> Vec<String> {
        self.events.iter().map(Value::to_string).collect()
    }

    /// The events framed as the first-party API's server-sent events.
    pub(crate) fn sse(self) -> String {
        self.events.iter().fold(String::new(), |mut sse, event| {
            write!(sse, "event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()).unwrap();
            sse
        })
    }

    fn thinking_deltas(self, index: u32, chunks: &[&str]) -> Self {
        let start = json!({ "type": "thinking", "thinking": "", "signature": "" });
        chunks.iter().fold(self.block_start(index, &start), |builder, thinking| {
            builder.block_delta(index, &json!({ "type": "thinking_delta", "thinking": thinking }))
        })
    }

    fn block_start(self, index: u32, content_block: &Value) -> Self {
        self.push(json!({ "type": "content_block_start", "index": index, "content_block": content_block }))
    }

    fn block_delta(self, index: u32, delta: &Value) -> Self {
        self.push(json!({ "type": "content_block_delta", "index": index, "delta": delta }))
    }

    fn push(mut self, event: Value) -> Self {
        self.events.push(event);
        self
    }
}
