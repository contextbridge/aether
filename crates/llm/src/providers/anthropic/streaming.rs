use super::types::{ContentBlockDeltaData, ContentBlockStartData, StreamEvent};
use crate::providers::stream_assembler::{StreamAssembler, assemble};
use crate::{LlmError, LlmResponse, ProviderError, Result, StopReason};
use futures::{Stream, StreamExt, stream};
use tracing::debug;

pub fn process_anthropic_stream(
    lines: impl Stream<Item = Result<String>> + Send,
) -> impl Stream<Item = Result<LlmResponse>> + Send {
    stream::iter([Ok(LlmResponse::Start)]).chain(assemble(lines, |line, turn| decode_line(&line, turn)))
}

fn decode_line(line: &str, turn: &mut StreamAssembler<u32>) -> Result<Vec<LlmResponse>> {
    match serde_json::from_str(line) {
        Ok(event) => decode_event(event, turn),
        Err(e) => {
            debug!("Failed to parse SSE line: {line} - Error: {e}");
            Ok(vec![])
        }
    }
}

fn decode_event(event: StreamEvent, turn: &mut StreamAssembler<u32>) -> Result<Vec<LlmResponse>> {
    let response = match event {
        StreamEvent::ContentBlockStart { data } => match data.content_block {
            ContentBlockStartData::ToolUse { id, name } => Some(turn.start_tool(data.index, id, name)),
            ContentBlockStartData::Text { .. } | ContentBlockStartData::Thinking { .. } => None,
        },
        StreamEvent::ContentBlockDelta { data } => match data.delta {
            ContentBlockDeltaData::TextDelta { text } => {
                (!text.is_empty()).then_some(LlmResponse::Text { chunk: text })
            }
            ContentBlockDeltaData::ThinkingDelta { thinking } => {
                (!thinking.is_empty()).then_some(LlmResponse::Reasoning { chunk: thinking })
            }
            ContentBlockDeltaData::InputJsonDelta { partial_json } => turn.append_tool_args(data.index, partial_json),
        },
        StreamEvent::ContentBlockStop { data } => turn.complete_tool(data.index),
        StreamEvent::MessageDelta { data } => {
            if let Some(stop_reason) = data.delta.stop_reason.as_deref() {
                turn.stop(map_anthropic_stop_reason(stop_reason));
            }
            data.usage.as_ref().map(|usage| LlmResponse::Usage { tokens: usage.into() })
        }
        StreamEvent::MessageStop { .. } => {
            turn.terminate();
            None
        }
        StreamEvent::Error { data } => {
            return Err(map_anthropic_stream_error(&data.error.error_type, &data.error.message));
        }
        StreamEvent::MessageStart { .. } | StreamEvent::Ping => None,
    };

    Ok(response.into_iter().collect())
}

fn map_anthropic_stream_error(error_type: &str, message: &str) -> LlmError {
    let kind = match error_type {
        "rate_limit_error" => crate::ProviderErrorKind::RateLimit,
        "overloaded_error" | "internal_server_error" | "api_error" => crate::ProviderErrorKind::Server,
        _ => crate::ProviderErrorKind::Api,
    };
    ProviderError::new(kind, format!("Anthropic API error: {error_type} - {message}"))
        .with_code(Some(error_type.to_string()))
        .into()
}

fn map_anthropic_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolCalls,
        "max_tokens" => StopReason::Length,
        _ => StopReason::Unknown(reason.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::llm_response;
    use crate::{ProviderErrorKind, TokenUsage};
    use serde_json::{Value, json};

    #[tokio::test]
    async fn test_process_text_stream() {
        let responses = collect_responses(
            anthropic_stream()
                .text(0, &["Hello", " world"])
                .message_delta("end_turn", &usage(10, 25))
                .message_stop()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().text(&["Hello", " world"]).usage(10, 25).build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn test_process_tool_use_stream() {
        let deltas = [r#"{"query":"#, r#""test"}"#];

        let responses = collect_responses(
            anthropic_stream()
                .tool_call(0, "tool_123", "search", &deltas)
                .message_delta("tool_use", &usage(10, 15))
                .message_stop()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response()
                .tool_call("tool_123", "search", &deltas)
                .usage(10, 15)
                .build_with_stop_reason(StopReason::ToolCalls)
        );
    }

    #[tokio::test]
    async fn test_sequential_tool_calls_reusing_an_index_complete_separately() {
        let responses = collect_responses(
            anthropic_stream()
                .tool_call(0, "tool_123", "search", &[r#"{"query":"test1"}"#])
                .tool_call(0, "tool_456", "calculate", &[r#"{"expression":"2+2"}"#])
                .message_stop()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response()
                .tool_call("tool_123", "search", &[r#"{"query":"test1"}"#])
                .tool_call("tool_456", "calculate", &[r#"{"expression":"2+2"}"#])
                .build()
        );
    }

    #[tokio::test]
    async fn stream_closed_mid_tool_call_does_not_complete_it() {
        let responses = process_lines(
            anthropic_stream().tool_start(0, "tool_a", "bash").tool_delta(0, r#"{"command":"ls"#).build(),
        )
        .await;

        assert!(
            matches!(
                responses.as_slice(),
                [
                    Ok(LlmResponse::Start),
                    Ok(LlmResponse::ToolRequestStart { .. }),
                    Ok(LlmResponse::ToolRequestArg { .. }),
                    Err(error)
                ] if error.provider().map(|provider| provider.kind) == Some(ProviderErrorKind::StreamInterrupted)
            ),
            "{responses:?}"
        );
    }

    #[tokio::test]
    async fn test_process_thinking_stream() {
        let responses = collect_responses(
            anthropic_stream()
                .thinking(0, &["Let me think", " about this"])
                .text(1, &["Here is my answer"])
                .message_delta("end_turn", &usage(10, 50))
                .message_stop()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response()
                .reasoning(&["Let me think", " about this"])
                .text(&["Here is my answer"])
                .usage(10, 50)
                .build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn test_message_delta_forwards_both_cache_read_and_creation() {
        let responses = collect_responses(
            anthropic_stream()
                .text(0, &["ok"])
                .message_delta(
                    "end_turn",
                    &json!({
                        "input_tokens": 100,
                        "output_tokens": 25,
                        "cache_creation_input_tokens": 40,
                        "cache_read_input_tokens": 60
                    }),
                )
                .message_stop()
                .build(),
        )
        .await;

        let usage = responses.iter().find_map(|r| match r {
            LlmResponse::Usage { tokens } => Some(*tokens),
            _ => None,
        });
        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 200.into(),
                output_tokens: 25.into(),
                cache_read_tokens: Some(60.into()),
                cache_creation_tokens: Some(40.into()),
                ..TokenUsage::default()
            }),
            "cached tokens count toward the prompt"
        );
    }

    #[tokio::test]
    async fn error_event_ends_the_stream_with_its_kind() {
        for (error_type, kind) in [
            ("rate_limit_error", ProviderErrorKind::RateLimit),
            ("overloaded_error", ProviderErrorKind::Server),
            ("invalid_request_error", ProviderErrorKind::Api),
        ] {
            let responses = process_lines(anthropic_stream().error(error_type, "boom").message_stop().build()).await;

            assert!(
                matches!(
                    responses.as_slice(),
                    [Ok(LlmResponse::Start), Err(error)] if error.provider().map(|provider| provider.kind) == Some(kind)
                ),
                "{error_type}: {responses:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_anthropic_stream_event_enum_deserialization() {
        use super::super::types::StreamEvent;

        // Test message_start deserialization
        let message_start_json = r#"{"type": "message_start", "message": {"id": "msg_123", "type": "message", "role": "assistant", "content": [], "model": "claude-3", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 0}}}"#;
        let event: StreamEvent = serde_json::from_str(message_start_json).unwrap();
        assert!(matches!(event, StreamEvent::MessageStart { .. }));

        // Test content_block_start deserialization
        let content_block_start_json =
            r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}"#;
        let event: StreamEvent = serde_json::from_str(content_block_start_json).unwrap();
        assert!(matches!(event, StreamEvent::ContentBlockStart { .. }));

        // Test content_block_delta deserialization
        let content_block_delta_json =
            r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}"#;
        let event: StreamEvent = serde_json::from_str(content_block_delta_json).unwrap();
        assert!(matches!(event, StreamEvent::ContentBlockDelta { .. }));

        // Test ping deserialization
        let ping_json = r#"{"type": "ping"}"#;
        let event: StreamEvent = serde_json::from_str(ping_json).unwrap();
        assert!(matches!(event, StreamEvent::Ping));

        // Test error deserialization
        let error_json =
            r#"{"type": "error", "error": {"type": "rate_limit_error", "message": "Rate limit exceeded"}}"#;
        let event: StreamEvent = serde_json::from_str(error_json).unwrap();
        assert!(matches!(event, StreamEvent::Error { .. }));
    }

    async fn collect_responses(lines: Vec<String>) -> Vec<LlmResponse> {
        process_lines(lines).await.into_iter().map(Result::unwrap).collect()
    }

    async fn process_lines(lines: Vec<String>) -> Vec<Result<LlmResponse>> {
        process_anthropic_stream(stream::iter(lines.into_iter().map(Ok))).collect().await
    }

    fn anthropic_stream() -> AnthropicStreamBuilder {
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

    #[derive(Default)]
    struct AnthropicStreamBuilder {
        events: Vec<Value>,
    }

    impl AnthropicStreamBuilder {
        fn text(self, index: u32, chunks: &[&str]) -> Self {
            chunks
                .iter()
                .fold(self.block_start(index, &json!({ "type": "text", "text": "" })), |builder, text| {
                    builder.block_delta(index, &json!({ "type": "text_delta", "text": text }))
                })
                .block_stop(index)
        }

        fn thinking(self, index: u32, chunks: &[&str]) -> Self {
            chunks
                .iter()
                .fold(self.block_start(index, &json!({ "type": "thinking", "thinking": "" })), |builder, thinking| {
                    builder.block_delta(index, &json!({ "type": "thinking_delta", "thinking": thinking }))
                })
                .block_stop(index)
        }

        fn tool_call(self, index: u32, id: &str, name: &str, argument_deltas: &[&str]) -> Self {
            argument_deltas
                .iter()
                .fold(self.tool_start(index, id, name), |builder, delta| builder.tool_delta(index, delta))
                .block_stop(index)
        }

        fn tool_start(self, index: u32, id: &str, name: &str) -> Self {
            self.block_start(index, &json!({ "type": "tool_use", "id": id, "name": name }))
        }

        fn tool_delta(self, index: u32, partial_json: &str) -> Self {
            self.block_delta(index, &json!({ "type": "input_json_delta", "partial_json": partial_json }))
        }

        fn message_delta(self, stop_reason: &str, usage: &Value) -> Self {
            self.push(json!({
                "type": "message_delta",
                "delta": { "stop_reason": stop_reason, "stop_sequence": null },
                "usage": usage
            }))
        }

        fn message_stop(self) -> Self {
            self.push(json!({ "type": "message_stop" }))
        }

        fn error(self, error_type: &str, message: &str) -> Self {
            self.push(json!({ "type": "error", "error": { "type": error_type, "message": message } }))
        }

        fn block_start(self, index: u32, content_block: &Value) -> Self {
            self.push(json!({ "type": "content_block_start", "index": index, "content_block": content_block }))
        }

        fn block_delta(self, index: u32, delta: &Value) -> Self {
            self.push(json!({ "type": "content_block_delta", "index": index, "delta": delta }))
        }

        fn block_stop(self, index: u32) -> Self {
            self.push(json!({ "type": "content_block_stop", "index": index }))
        }

        fn push(mut self, event: Value) -> Self {
            self.events.push(event);
            self
        }

        fn build(self) -> Vec<String> {
            self.events.iter().map(Value::to_string).collect()
        }
    }

    fn usage(input_tokens: u32, output_tokens: u32) -> Value {
        json!({ "input_tokens": input_tokens, "output_tokens": output_tokens })
    }
}
