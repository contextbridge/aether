use super::types::{ContentBlockDeltaData, ContentBlockStartData, StreamEvent};
use crate::{LlmError, LlmResponse, Result, StopReason, ToolCallRequest};
use async_stream;
use futures::Stream;
use std::collections::HashMap;
use tokio_stream::StreamExt;
use tracing::{debug, warn};

pub fn process_anthropic_stream<T: Stream<Item = Result<String>> + Send + Sync + Unpin>(
    stream: T,
) -> impl Stream<Item = Result<LlmResponse>> + Send {
    async_stream::stream! {
        let message_id = uuid::Uuid::new_v4().to_string();
        yield Ok(LlmResponse::Start { message_id });

        let mut active_tool_calls: HashMap<String, (String, String)> = HashMap::new();
        let mut index_to_id: HashMap<u32, String> = HashMap::new();
        let mut stream = Box::pin(stream);
        let mut last_stop_reason: Option<StopReason> = None;

        while let Some(result) = stream.next().await {
            match result {
                Ok(line) => {
                    let event: StreamEvent = match serde_json::from_str(&line) {
                        Ok(event) => event,
                        Err(e) => {
                            debug!("Failed to parse SSE line: {} - Error: {}", line, e);
                            continue;
                        }
                    };

                    match process_stream_event(event, &mut active_tool_calls, &mut index_to_id) {
                        Ok((response, stop_reason)) => {
                            if let Some(stop_reason) = stop_reason {
                                last_stop_reason = Some(stop_reason);
                            }
                            if let Some(response) = response {
                                yield Ok(response);
                            }
                        }
                        Err(e) => {
                            yield Err(e);
                            break;
                        }
                    }
                }
                Err(e) => {
                    yield Err(e);
                    break;
                }
            }
        }

        for (id, (name, arguments)) in active_tool_calls {
            let tool_call = ToolCallRequest { id, name, arguments };
            yield Ok(LlmResponse::ToolRequestComplete { tool_call });
        }

        yield Ok(LlmResponse::Done {
            stop_reason: last_stop_reason,
        });
    }
}

fn process_stream_event(
    event: StreamEvent,
    active_tool_calls: &mut HashMap<String, (String, String)>,
    index_to_id: &mut HashMap<u32, String>,
) -> Result<(Option<LlmResponse>, Option<StopReason>)> {
    use StreamEvent::{
        ContentBlockDelta, ContentBlockStart, ContentBlockStop, Error, MessageDelta, MessageStart, MessageStop, Ping,
    };
    match event {
        MessageStart { .. } => Ok(handle_message_start()),
        ContentBlockStart { data } => Ok(handle_content_block_start(data, active_tool_calls, index_to_id)),
        ContentBlockDelta { data } => Ok(handle_content_block_delta(data, active_tool_calls, index_to_id)),
        ContentBlockStop { data } => Ok(handle_content_block_stop(&data, active_tool_calls, index_to_id)),
        MessageDelta { data } => Ok(handle_message_delta(&data)),
        MessageStop { .. } => Ok(handle_message_stop()),
        Error { data } => Err(map_anthropic_stream_error(&data.error.error_type, &data.error.message)),
        Ping => Ok(handle_ping()),
    }
}

fn map_anthropic_stream_error(error_type: &str, message: &str) -> LlmError {
    let formatted = format!("Anthropic API error: {error_type} - {message}");
    match error_type {
        "rate_limit_error" => LlmError::RateLimited(formatted),
        "overloaded_error" | "internal_server_error" | "api_error" => {
            LlmError::ServerError { status: None, message: formatted }
        }
        _ => LlmError::ApiError(formatted),
    }
}

fn map_anthropic_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolCalls,
        "max_tokens" => StopReason::Length,
        _ => StopReason::Unknown(reason.to_string()),
    }
}

type EventResult = (Option<LlmResponse>, Option<StopReason>);

fn handle_message_start() -> EventResult {
    debug!("Message started");
    (None, None)
}

fn handle_content_block_start(
    start_data: super::types::ContentBlockStart,
    active_tool_calls: &mut HashMap<String, (String, String)>,
    index_to_id: &mut HashMap<u32, String>,
) -> EventResult {
    match start_data.content_block {
        ContentBlockStartData::Text { .. } => {
            debug!("Text block started at index {}", start_data.index);
            (None, None)
        }
        ContentBlockStartData::Thinking { .. } => {
            debug!("Thinking block started at index {}", start_data.index);
            (None, None)
        }
        ContentBlockStartData::ToolUse { id, name } => {
            debug!("Tool use started: {} ({})", name, id);
            index_to_id.insert(start_data.index, id.clone());
            active_tool_calls.insert(id.clone(), (name.clone(), String::new()));
            (Some(LlmResponse::ToolRequestStart { id, name }), None)
        }
    }
}

fn handle_content_block_delta(
    delta_data: super::types::ContentBlockDelta,
    active_tool_calls: &mut HashMap<String, (String, String)>,
    index_to_id: &HashMap<u32, String>,
) -> EventResult {
    match delta_data.delta {
        ContentBlockDeltaData::TextDelta { text } => {
            if text.is_empty() {
                (None, None)
            } else {
                (Some(LlmResponse::Text { chunk: text }), None)
            }
        }
        ContentBlockDeltaData::ThinkingDelta { thinking } => {
            if thinking.is_empty() {
                (None, None)
            } else {
                (Some(LlmResponse::Reasoning { chunk: thinking }), None)
            }
        }
        ContentBlockDeltaData::InputJsonDelta { partial_json } => {
            if let Some(id) = index_to_id.get(&delta_data.index) {
                if let Some((_name, arguments)) = active_tool_calls.get_mut(id) {
                    arguments.push_str(&partial_json);
                    (Some(LlmResponse::ToolRequestArg { id: id.clone(), chunk: partial_json }), None)
                } else {
                    warn!("Received tool input delta for unknown tool call id: {}", id);
                    (None, None)
                }
            } else {
                warn!("Received tool input delta for unknown tool call index: {}", delta_data.index);
                (None, None)
            }
        }
    }
}

fn handle_content_block_stop(
    stop_data: &super::types::ContentBlockStop,
    active_tool_calls: &mut HashMap<String, (String, String)>,
    index_to_id: &mut HashMap<u32, String>,
) -> EventResult {
    if let Some(id) = index_to_id.remove(&stop_data.index) {
        if let Some((name, arguments)) = active_tool_calls.remove(&id) {
            let tool_call = ToolCallRequest { id, name, arguments };
            (Some(LlmResponse::ToolRequestComplete { tool_call }), None)
        } else {
            debug!("Content block stopped but tool call not found for id: {}", id);
            (None, None)
        }
    } else {
        debug!("Content block stopped at index {}", stop_data.index);
        (None, None)
    }
}

fn handle_message_delta(message_delta: &super::types::MessageDelta) -> EventResult {
    debug!("Message delta received");
    let stop_reason = message_delta.delta.stop_reason.as_deref().map(map_anthropic_stop_reason);

    let response = message_delta.usage.as_ref().map(|usage| LlmResponse::Usage { tokens: usage.into() });
    (response, stop_reason)
}

fn handle_message_stop() -> EventResult {
    debug!("Message stopped");
    (None, None)
}

fn handle_ping() -> EventResult {
    debug!("Received ping event");
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsage;
    use tokio_stream;

    #[tokio::test]
    async fn test_process_text_stream() {
        let lines = vec![
            "{\"type\": \"message_start\", \"message\": {\"id\": \"msg_123\", \"type\": \"message\", \"role\": \"assistant\", \"content\": [], \"model\": \"claude-3\", \"stop_reason\": null, \"stop_sequence\": null, \"usage\": {\"input_tokens\": 10, \"output_tokens\": 0}}}".to_string(),
            "{\"type\": \"content_block_start\", \"index\": 0, \"content_block\": {\"type\": \"text\", \"text\": \"\"}}".to_string(),
            "{\"type\": \"content_block_delta\", \"index\": 0, \"delta\": {\"type\": \"text_delta\", \"text\": \"Hello\"}}".to_string(),
            "{\"type\": \"content_block_delta\", \"index\": 0, \"delta\": {\"type\": \"text_delta\", \"text\": \" world\"}}".to_string(),
            "{\"type\": \"content_block_stop\", \"index\": 0}".to_string(),
            "{\"type\": \"message_delta\", \"delta\": {\"stop_reason\": \"end_turn\", \"stop_sequence\": null}, \"usage\": {\"input_tokens\": 10, \"output_tokens\": 25}}".to_string(),
            "{\"type\": \"message_stop\"}".to_string(),
        ];

        let stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut response_stream = Box::pin(process_anthropic_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

        assert!(matches!(responses[0], LlmResponse::Start { .. }));
        assert!(matches!(responses[1], LlmResponse::Text { ref chunk } if chunk == "Hello"));
        assert!(matches!(responses[2], LlmResponse::Text { ref chunk } if chunk == " world"));
        assert!(matches!(
            responses[3],
            LlmResponse::Usage { tokens: TokenUsage { input_tokens: 10, output_tokens: 25, .. } }
        ));
        assert!(matches!(responses[4], LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }));
    }

    #[tokio::test]
    async fn test_process_tool_use_stream() {
        let lines = vec![
            "{\"type\": \"message_start\", \"message\": {\"id\": \"msg_123\", \"type\": \"message\", \"role\": \"assistant\", \"content\": [], \"model\": \"claude-3\", \"stop_reason\": null, \"stop_sequence\": null, \"usage\": {\"input_tokens\": 10, \"output_tokens\": 0}}}".to_string(),
            "{\"type\": \"content_block_start\", \"index\": 0, \"content_block\": {\"type\": \"tool_use\", \"id\": \"tool_123\", \"name\": \"search\"}}".to_string(),
            "{\"type\": \"content_block_delta\", \"index\": 0, \"delta\": {\"type\": \"input_json_delta\", \"partial_json\": \"{\\\"query\\\":\\\"test\\\"}\"}".to_string(),
            "{\"type\": \"content_block_stop\", \"index\": 0}".to_string(),
            "{\"type\": \"message_delta\", \"delta\": {\"stop_reason\": \"tool_use\", \"stop_sequence\": null}, \"usage\": {\"input_tokens\": 10, \"output_tokens\": 15}}".to_string(),
            "{\"type\": \"message_stop\"}".to_string(),
        ];

        let stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut response_stream = Box::pin(process_anthropic_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

        assert!(matches!(responses[0], LlmResponse::Start { .. }));
        assert!(
            matches!(responses[1], LlmResponse::ToolRequestStart { ref id, ref name } if id == "tool_123" && name == "search")
        );
        assert!(
            matches!(responses[2], LlmResponse::ToolRequestComplete { ref tool_call } if tool_call.id == "tool_123" && tool_call.name == "search")
        );
        assert!(matches!(
            responses[3],
            LlmResponse::Usage { tokens: TokenUsage { input_tokens: 10, output_tokens: 15, .. } }
        ));
        assert!(matches!(responses[4], LlmResponse::Done { stop_reason: Some(StopReason::ToolCalls) }));
    }

    #[tokio::test]
    async fn test_multiple_tool_calls_with_same_index() {
        // This test demonstrates the issue with index-based tracking
        // If multiple tool calls happen to have the same index (which shouldn't happen
        // but could theoretically), the current implementation would overwrite them
        let lines = vec![
            r#"{"type": "message_start", "message": {"id": "msg_123", "type": "message", "role": "assistant", "content": [], "model": "claude-3", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 0}}}"#.to_string(),
            r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "tool_123", "name": "search"}}"#.to_string(),
            r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"query\":\"test1\"}"}}"#.to_string(),
            r#"{"type": "content_block_stop", "index": 0}"#.to_string(),
            // Another tool call with different ID but same index (simulating potential edge case)
            r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "tool_456", "name": "calculate"}}"#.to_string(),
            r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"expression\":\"2+2\"}"}}"#.to_string(),
            r#"{"type": "content_block_stop", "index": 0}"#.to_string(),
            r#"{"type": "message_stop"}"#.to_string(),
        ];

        let stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut response_stream = Box::pin(process_anthropic_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

        // We should get both tool calls, but with index-based tracking we might lose one
        let tool_starts: Vec<_> =
            responses.iter().filter(|r| matches!(r, LlmResponse::ToolRequestStart { .. })).collect();
        let tool_completes: Vec<_> =
            responses.iter().filter(|r| matches!(r, LlmResponse::ToolRequestComplete { .. })).collect();

        // With ID-based tracking, we should get both tool calls
        assert_eq!(tool_starts.len(), 2);
        assert_eq!(tool_completes.len(), 2);
    }

    #[tokio::test]
    async fn test_process_thinking_stream() {
        let lines = vec![
            r#"{"type": "message_start", "message": {"id": "msg_123", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-4-6", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 0}}}"#.to_string(),
            r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}"#.to_string(),
            r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Let me think"}}"#.to_string(),
            r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": " about this"}}"#.to_string(),
            r#"{"type": "content_block_stop", "index": 0}"#.to_string(),
            r#"{"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}"#.to_string(),
            r#"{"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Here is my answer"}}"#.to_string(),
            r#"{"type": "content_block_stop", "index": 1}"#.to_string(),
            r#"{"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"input_tokens": 10, "output_tokens": 50}}"#.to_string(),
            r#"{"type": "message_stop"}"#.to_string(),
        ];

        let stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut response_stream = Box::pin(process_anthropic_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

        assert!(matches!(responses[0], LlmResponse::Start { .. }));
        assert!(matches!(responses[1], LlmResponse::Reasoning { ref chunk } if chunk == "Let me think"));
        assert!(matches!(responses[2], LlmResponse::Reasoning { ref chunk } if chunk == " about this"));
        assert!(matches!(responses[3], LlmResponse::Text { ref chunk } if chunk == "Here is my answer"));
        assert!(matches!(
            responses[4],
            LlmResponse::Usage { tokens: TokenUsage { input_tokens: 10, output_tokens: 50, .. } }
        ));
        assert!(matches!(responses[5], LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }));
    }

    #[tokio::test]
    async fn test_message_delta_forwards_both_cache_read_and_creation() {
        let lines = vec![
            r#"{"type": "message_start", "message": {"id": "msg_xyz", "type": "message", "role": "assistant", "content": [], "model": "claude-3", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 0}}}"#.to_string(),
            r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}"#.to_string(),
            r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "ok"}}"#.to_string(),
            r#"{"type": "content_block_stop", "index": 0}"#.to_string(),
            r#"{"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"input_tokens": 100, "output_tokens": 25, "cache_creation_input_tokens": 40, "cache_read_input_tokens": 60}}"#.to_string(),
            r#"{"type": "message_stop"}"#.to_string(),
        ];

        let stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut response_stream = Box::pin(process_anthropic_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

        let usage = responses.iter().find_map(|r| match r {
            LlmResponse::Usage { tokens } => Some(*tokens),
            _ => None,
        });

        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 100,
                output_tokens: 25,
                cache_read_tokens: Some(60),
                cache_creation_tokens: Some(40),
                ..TokenUsage::default()
            })
        );
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
}
