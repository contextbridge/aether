use super::types::{ContentBlock, ContentBlockDeltaData, ContentBlockStartData, StreamEvent, Usage};
use crate::providers::response_stream::StreamAssembler;
use crate::{LlmError, LlmResponse, ProviderError, Result, StopReason};
use std::collections::HashMap;
use tracing::debug;

pub(crate) fn decode_messages<T: AsRef<[u8]>>()
-> impl FnMut(T, &mut StreamAssembler<u32>) -> Result<Vec<LlmResponse>> + Send {
    let mut decoder = MessagesDecoder::default();
    move |json, turn| decoder.decode(json.as_ref(), turn)
}

#[derive(Default)]
struct MessagesDecoder {
    message_id: String,
    usage: Usage,
    thinking: HashMap<u32, ContentBlock>,
}

impl MessagesDecoder {
    fn decode(&mut self, json: &[u8], turn: &mut StreamAssembler<u32>) -> Result<Vec<LlmResponse>> {
        match serde_json::from_slice(json) {
            Ok(event) => self.decode_event(event, turn),
            Err(e) => {
                debug!("Failed to parse Messages stream event: {} - Error: {e}", String::from_utf8_lossy(json));
                Ok(vec![])
            }
        }
    }

    fn decode_event(&mut self, event: StreamEvent, turn: &mut StreamAssembler<u32>) -> Result<Vec<LlmResponse>> {
        let response = match event {
            StreamEvent::MessageStart(start) => {
                self.message_id = start.message.id;
                self.usage = start.message.usage.unwrap_or_default();
                None
            }
            StreamEvent::ContentBlockStart(start) => match start.content_block {
                ContentBlockStartData::ToolUse { id, name } => Some(turn.start_tool(start.index, id, name)),
                ContentBlockStartData::Thinking => {
                    let block = ContentBlock::Thinking { thinking: String::new(), signature: String::new() };
                    self.thinking.insert(start.index, block);
                    None
                }
                ContentBlockStartData::RedactedThinking { data } => {
                    self.thinking.insert(start.index, ContentBlock::RedactedThinking { data });
                    None
                }
                ContentBlockStartData::Other => None,
            },
            StreamEvent::ContentBlockDelta(data) => match data.delta {
                ContentBlockDeltaData::TextDelta { text } => {
                    (!text.is_empty()).then_some(LlmResponse::Text { chunk: text })
                }
                ContentBlockDeltaData::ThinkingDelta { thinking: chunk } => {
                    if let Some(ContentBlock::Thinking { thinking, .. }) = self.thinking.get_mut(&data.index) {
                        thinking.push_str(&chunk);
                    }
                    (!chunk.is_empty()).then_some(LlmResponse::Reasoning { chunk })
                }
                ContentBlockDeltaData::SignatureDelta { signature: chunk } => {
                    if let Some(ContentBlock::Thinking { signature, .. }) = self.thinking.get_mut(&data.index) {
                        signature.push_str(&chunk);
                    }
                    None
                }
                ContentBlockDeltaData::InputJsonDelta { partial_json } => {
                    turn.append_tool_args(&data.index, partial_json)
                }
            },
            StreamEvent::ContentBlockStop(data) => match self.thinking.remove(&data.index) {
                Some(block) => self.replayable(&block)?,
                None => turn.complete_tool(&data.index),
            },
            StreamEvent::MessageDelta(data) => {
                if let Some(stop_reason) = data.delta.stop_reason.as_deref() {
                    turn.stop(map_anthropic_stop_reason(stop_reason));
                }
                data.usage.map(|usage| {
                    self.usage = self.usage.merge(&usage);
                    LlmResponse::Usage { tokens: (&self.usage).into() }
                })
            }
            StreamEvent::MessageStop => {
                turn.allow_eof();
                None
            }
            StreamEvent::Error(data) => {
                return Err(map_anthropic_stream_error(&data.error.error_type, &data.error.message));
            }
            StreamEvent::Ping => None,
        };

        Ok(response.into_iter().collect())
    }

    fn replayable(&self, block: &ContentBlock) -> Result<Option<LlmResponse>> {
        if matches!(block, ContentBlock::Thinking { signature, .. } if signature.is_empty()) {
            return Ok(None);
        }
        let content = serde_json::to_string(block)?;
        Ok(Some(LlmResponse::EncryptedReasoning { id: self.message_id.clone(), content }))
    }
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
    use crate::provider_connection::DEFAULT_STREAM_IDLE_TIMEOUT;
    use crate::providers::anthropic::test_stream::{anthropic_stream, usage};
    use crate::providers::response_stream::{OpenedStream, response_stream};
    use crate::testing::llm_response;
    use crate::{ProviderErrorKind, TokenUsage};
    use futures::{StreamExt, stream};
    use serde_json::json;
    use std::future::ready;

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
    async fn unknown_tool_call_arguments_and_stops_are_ignored() {
        let responses =
            collect_responses(anthropic_stream().tool_delta(7, "{}").block_stop(7).message_stop().build()).await;

        assert_eq!(responses, llm_response().build());
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

        let message_start_json = r#"{"type": "message_start", "message": {"id": "msg_123", "type": "message", "role": "assistant", "content": [], "model": "claude-3", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 0}}}"#;
        let event: StreamEvent = serde_json::from_str(message_start_json).unwrap();
        assert!(matches!(event, StreamEvent::MessageStart(start) if start.message.id == "msg_123"));

        let content_block_start_json =
            r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}"#;
        let event: StreamEvent = serde_json::from_str(content_block_start_json).unwrap();
        assert!(matches!(event, StreamEvent::ContentBlockStart(_)));

        let content_block_delta_json =
            r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}"#;
        let event: StreamEvent = serde_json::from_str(content_block_delta_json).unwrap();
        assert!(matches!(event, StreamEvent::ContentBlockDelta(_)));

        let ping_json = r#"{"type": "ping"}"#;
        let event: StreamEvent = serde_json::from_str(ping_json).unwrap();
        assert!(matches!(event, StreamEvent::Ping));

        let error_json =
            r#"{"type": "error", "error": {"type": "rate_limit_error", "message": "Rate limit exceeded"}}"#;
        let event: StreamEvent = serde_json::from_str(error_json).unwrap();
        assert!(matches!(event, StreamEvent::Error(_)));
    }

    async fn collect_responses(lines: Vec<String>) -> Vec<LlmResponse> {
        process_lines(lines).await.into_iter().map(Result::unwrap).collect()
    }

    async fn process_lines(lines: Vec<String>) -> Vec<Result<LlmResponse>> {
        let opened = ready(Ok(OpenedStream::new(stream::iter(lines.into_iter().map(Ok)))));
        response_stream(opened, decode_messages::<String>(), DEFAULT_STREAM_IDLE_TIMEOUT).collect().await
    }
}
