use aws_sdk_bedrockruntime::error::SdkError;
use aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver;
use aws_sdk_bedrockruntime::types::error::ConverseStreamOutputError;
use aws_sdk_bedrockruntime::types::{
    ContentBlockDelta, ContentBlockStart, ConverseStreamOutput, StopReason as BedrockStopReason,
    TokenUsage as BedrockTokenUsage,
};
use aws_smithy_types::event_stream::RawMessage;
use futures::Stream;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

use crate::{LlmError, LlmResponse, StopReason, TokenUsage, ToolCallRequest};

impl From<&BedrockTokenUsage> for TokenUsage {
    fn from(usage: &BedrockTokenUsage) -> Self {
        TokenUsage {
            input_tokens: u32::try_from(usage.input_tokens).unwrap_or(0),
            output_tokens: u32::try_from(usage.output_tokens).unwrap_or(0),
            cache_read_tokens: usage.cache_read_input_tokens().and_then(|v| u32::try_from(v).ok()),
            cache_creation_tokens: usage.cache_write_input_tokens().and_then(|v| u32::try_from(v).ok()),
            ..TokenUsage::default()
        }
    }
}

struct PendingToolCall {
    id: String,
    name: String,
    args: String,
}

enum StreamEvent {
    Emit(LlmResponse),
    Stop(StopReason),
    Skip,
}

pub fn process_bedrock_stream(
    mut receiver: EventReceiver<ConverseStreamOutput, ConverseStreamOutputError>,
) -> impl Stream<Item = crate::Result<LlmResponse>> + Send {
    async_stream::stream! {
        let message_id = uuid::Uuid::new_v4().to_string();
        yield Ok(LlmResponse::Start { message_id });

        let mut active_tool_calls: HashMap<i32, PendingToolCall> = HashMap::new();
        let mut last_stop_reason: Option<StopReason> = None;

        loop {
            match receiver.recv().await {
                Ok(Some(event)) => {
                    match process_stream_event(&event, &mut active_tool_calls) {
                        StreamEvent::Emit(resp) => yield Ok(resp),
                        StreamEvent::Stop(sr) => last_stop_reason = Some(sr),
                        StreamEvent::Skip => {}
                    }
                }
                Ok(None) => {
                    debug!("Bedrock stream ended (recv returned None)");
                    break;
                }
                Err(e) => {
                    error!("Bedrock stream recv error: {e}");
                    yield Err(LlmError::from(e));
                    break;
                }
            }
        }

        // Emit any remaining tool calls that weren't completed via ContentBlockStop
        for (_index, tc) in active_tool_calls {
            let tool_call = ToolCallRequest {
                id: tc.id,
                name: tc.name,
                arguments: tc.args,
            };
            yield Ok(LlmResponse::ToolRequestComplete { tool_call });
        }

        yield Ok(LlmResponse::Done {
            stop_reason: last_stop_reason,
        });
    }
}

fn process_stream_event(
    event: &ConverseStreamOutput,
    active_tool_calls: &mut HashMap<i32, PendingToolCall>,
) -> StreamEvent {
    match event {
        ConverseStreamOutput::MessageStart(_) => {
            info!("Bedrock message started");
            StreamEvent::Skip
        }
        ConverseStreamOutput::ContentBlockStart(start_event) => {
            handle_content_block_start(start_event, active_tool_calls)
        }
        ConverseStreamOutput::ContentBlockDelta(delta_event) => {
            handle_content_block_delta(delta_event, active_tool_calls)
        }
        ConverseStreamOutput::ContentBlockStop(stop_event) => {
            handle_content_block_stop(stop_event.content_block_index(), active_tool_calls)
        }
        ConverseStreamOutput::MessageStop(stop_event) => {
            let stop_reason = map_bedrock_stop_reason(&stop_event.stop_reason);
            info!("Bedrock message stopped: {stop_reason:?}");
            StreamEvent::Stop(stop_reason)
        }
        ConverseStreamOutput::Metadata(metadata_event) => metadata_event
            .usage()
            .map_or(StreamEvent::Skip, |usage| StreamEvent::Emit(LlmResponse::Usage { tokens: usage.into() })),
        other => {
            warn!("Unhandled Bedrock stream event: {other:?}");
            StreamEvent::Skip
        }
    }
}

fn handle_content_block_start(
    event: &aws_sdk_bedrockruntime::types::ContentBlockStartEvent,
    active_tool_calls: &mut HashMap<i32, PendingToolCall>,
) -> StreamEvent {
    let index = event.content_block_index();

    if let Some(ContentBlockStart::ToolUse(tool_start)) = event.start() {
        let id = tool_start.tool_use_id().to_string();
        let name = tool_start.name().to_string();
        debug!("Bedrock tool use started: {name} ({id})");
        active_tool_calls.insert(index, PendingToolCall { id: id.clone(), name: name.clone(), args: String::new() });
        StreamEvent::Emit(LlmResponse::ToolRequestStart { id, name })
    } else {
        debug!("Content block started at index {index}");
        StreamEvent::Skip
    }
}

fn handle_content_block_delta(
    event: &aws_sdk_bedrockruntime::types::ContentBlockDeltaEvent,
    active_tool_calls: &mut HashMap<i32, PendingToolCall>,
) -> StreamEvent {
    let index = event.content_block_index();

    let Some(delta) = event.delta() else {
        return StreamEvent::Skip;
    };

    match delta {
        ContentBlockDelta::Text(text) if !text.is_empty() => {
            StreamEvent::Emit(LlmResponse::Text { chunk: text.clone() })
        }
        ContentBlockDelta::ToolUse(tool_delta) => {
            let input = tool_delta.input();
            if input.is_empty() {
                return StreamEvent::Skip;
            }

            if let Some(tc) = active_tool_calls.get_mut(&index) {
                tc.args.push_str(input);
                StreamEvent::Emit(LlmResponse::ToolRequestArg { id: tc.id.clone(), chunk: input.to_string() })
            } else {
                warn!("Received tool input delta for unknown content block index: {index}");
                StreamEvent::Skip
            }
        }
        ContentBlockDelta::ReasoningContent(reasoning) => {
            if let Ok(text) = reasoning.as_text()
                && !text.is_empty()
            {
                return StreamEvent::Emit(LlmResponse::Reasoning { chunk: text.clone() });
            }
            StreamEvent::Skip
        }
        _ => {
            debug!("Unhandled content block delta type");
            StreamEvent::Skip
        }
    }
}

fn handle_content_block_stop(index: i32, active_tool_calls: &mut HashMap<i32, PendingToolCall>) -> StreamEvent {
    if let Some(tc) = active_tool_calls.remove(&index) {
        let tool_call = ToolCallRequest { id: tc.id, name: tc.name, arguments: tc.args };
        StreamEvent::Emit(LlmResponse::ToolRequestComplete { tool_call })
    } else {
        debug!("Content block stopped at index {index}");
        StreamEvent::Skip
    }
}

impl From<SdkError<ConverseStreamOutputError, RawMessage>> for LlmError {
    fn from(e: SdkError<ConverseStreamOutputError, RawMessage>) -> Self {
        let message = format!("Bedrock stream error: {e}");
        match e {
            SdkError::ServiceError(svc) => {
                let inner = svc.err();
                if inner.is_throttling_exception() {
                    LlmError::RateLimited(message)
                } else if inner.is_service_unavailable_exception()
                    || inner.is_internal_server_exception()
                    || inner.is_model_stream_error_exception()
                {
                    LlmError::StreamInterrupted(message)
                } else {
                    LlmError::ApiError(message)
                }
            }
            _ => LlmError::StreamInterrupted(message),
        }
    }
}

fn map_bedrock_stop_reason(reason: &BedrockStopReason) -> StopReason {
    match reason {
        BedrockStopReason::EndTurn | BedrockStopReason::StopSequence => StopReason::EndTurn,
        BedrockStopReason::ToolUse => StopReason::ToolCalls,
        BedrockStopReason::MaxTokens | BedrockStopReason::ModelContextWindowExceeded => StopReason::Length,
        BedrockStopReason::ContentFiltered | BedrockStopReason::GuardrailIntervened => StopReason::ContentFilter,
        other => StopReason::Unknown(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_stop_reason_end_turn() {
        assert_eq!(map_bedrock_stop_reason(&BedrockStopReason::EndTurn), StopReason::EndTurn);
    }

    #[test]
    fn test_map_stop_reason_stop_sequence() {
        assert_eq!(map_bedrock_stop_reason(&BedrockStopReason::StopSequence), StopReason::EndTurn);
    }

    #[test]
    fn test_map_stop_reason_tool_use() {
        assert_eq!(map_bedrock_stop_reason(&BedrockStopReason::ToolUse), StopReason::ToolCalls);
    }

    #[test]
    fn test_map_stop_reason_max_tokens() {
        assert_eq!(map_bedrock_stop_reason(&BedrockStopReason::MaxTokens), StopReason::Length);
    }

    #[test]
    fn test_map_stop_reason_context_window_exceeded() {
        assert_eq!(map_bedrock_stop_reason(&BedrockStopReason::ModelContextWindowExceeded), StopReason::Length);
    }

    #[test]
    fn test_map_stop_reason_content_filtered() {
        assert_eq!(map_bedrock_stop_reason(&BedrockStopReason::ContentFiltered), StopReason::ContentFilter);
    }

    #[test]
    fn test_map_stop_reason_guardrail() {
        assert_eq!(map_bedrock_stop_reason(&BedrockStopReason::GuardrailIntervened), StopReason::ContentFilter);
    }

    #[test]
    fn test_handle_content_block_start_tool_use() {
        let mut active = HashMap::new();
        let tool_start = aws_sdk_bedrockruntime::types::ToolUseBlockStart::builder()
            .tool_use_id("tool_123")
            .name("search")
            .build()
            .unwrap();

        let event = aws_sdk_bedrockruntime::types::ContentBlockStartEvent::builder()
            .content_block_index(0)
            .start(ContentBlockStart::ToolUse(tool_start))
            .build()
            .unwrap();

        let result = handle_content_block_start(&event, &mut active);
        assert!(
            matches!(&result, StreamEvent::Emit(LlmResponse::ToolRequestStart { id, name }) if id == "tool_123" && name == "search")
        );
        assert!(active.contains_key(&0));
    }

    #[test]
    fn test_handle_content_block_delta_text() {
        let mut active = HashMap::new();
        let delta = aws_sdk_bedrockruntime::types::ContentBlockDeltaEvent::builder()
            .content_block_index(0)
            .delta(ContentBlockDelta::Text("Hello".to_string()))
            .build()
            .unwrap();

        let result = handle_content_block_delta(&delta, &mut active);
        assert!(matches!(&result, StreamEvent::Emit(LlmResponse::Text { chunk }) if chunk == "Hello"));
    }

    #[test]
    fn test_handle_content_block_delta_tool_input() {
        let mut active = HashMap::new();
        active
            .insert(0, PendingToolCall { id: "tool_123".to_string(), name: "search".to_string(), args: String::new() });

        let tool_delta =
            aws_sdk_bedrockruntime::types::ToolUseBlockDelta::builder().input(r#"{"query":"test"}"#).build().unwrap();

        let delta = aws_sdk_bedrockruntime::types::ContentBlockDeltaEvent::builder()
            .content_block_index(0)
            .delta(ContentBlockDelta::ToolUse(tool_delta))
            .build()
            .unwrap();

        let result = handle_content_block_delta(&delta, &mut active);
        assert!(
            matches!(&result, StreamEvent::Emit(LlmResponse::ToolRequestArg { id, chunk }) if id == "tool_123" && chunk == r#"{"query":"test"}"#)
        );

        // Verify accumulated args
        assert_eq!(active.get(&0).unwrap().args, r#"{"query":"test"}"#);
    }

    #[test]
    fn test_handle_content_block_stop_completes_tool() {
        let mut active = HashMap::new();
        active.insert(
            0,
            PendingToolCall {
                id: "tool_123".to_string(),
                name: "search".to_string(),
                args: r#"{"query":"test"}"#.to_string(),
            },
        );

        let result = handle_content_block_stop(0, &mut active);
        assert!(matches!(&result, StreamEvent::Emit(LlmResponse::ToolRequestComplete { tool_call })
            if tool_call.id == "tool_123"
            && tool_call.name == "search"
            && tool_call.arguments == r#"{"query":"test"}"#
        ));
        assert!(active.is_empty());
    }

    #[test]
    fn test_handle_content_block_stop_no_tool() {
        let mut active = HashMap::new();
        let result = handle_content_block_stop(0, &mut active);
        assert!(matches!(result, StreamEvent::Skip));
    }

    #[test]
    fn test_metadata_event_emits_cache_read_and_creation() {
        let usage = aws_sdk_bedrockruntime::types::TokenUsage::builder()
            .input_tokens(100)
            .output_tokens(50)
            .total_tokens(150)
            .cache_read_input_tokens(40)
            .cache_write_input_tokens(20)
            .build()
            .unwrap();

        let metadata = aws_sdk_bedrockruntime::types::ConverseStreamMetadataEvent::builder().usage(usage).build();

        let event = ConverseStreamOutput::Metadata(metadata);
        let mut active = HashMap::new();
        let result = process_stream_event(&event, &mut active);

        match result {
            StreamEvent::Emit(LlmResponse::Usage { tokens: sample }) => {
                assert_eq!(sample.input_tokens, 100);
                assert_eq!(sample.output_tokens, 50);
                assert_eq!(sample.cache_read_tokens, Some(40));
                assert_eq!(sample.cache_creation_tokens, Some(20));
            }
            _ => panic!("expected Emit(Usage{{..}})"),
        }
    }

    #[test]
    fn test_metadata_event_without_cache_fields() {
        let usage = aws_sdk_bedrockruntime::types::TokenUsage::builder()
            .input_tokens(10)
            .output_tokens(5)
            .total_tokens(15)
            .build()
            .unwrap();

        let metadata = aws_sdk_bedrockruntime::types::ConverseStreamMetadataEvent::builder().usage(usage).build();

        let event = ConverseStreamOutput::Metadata(metadata);
        let mut active = HashMap::new();
        let result = process_stream_event(&event, &mut active);

        match result {
            StreamEvent::Emit(LlmResponse::Usage { tokens: sample }) => {
                assert_eq!(sample.cache_read_tokens, None);
                assert_eq!(sample.cache_creation_tokens, None);
            }
            _ => panic!("expected Emit(Usage{{..}})"),
        }
    }

    #[test]
    fn test_handle_content_block_delta_empty_text() {
        let mut active = HashMap::new();
        let delta = aws_sdk_bedrockruntime::types::ContentBlockDeltaEvent::builder()
            .content_block_index(0)
            .delta(ContentBlockDelta::Text(String::new()))
            .build()
            .unwrap();

        let result = handle_content_block_delta(&delta, &mut active);
        assert!(matches!(result, StreamEvent::Skip));
    }
}
