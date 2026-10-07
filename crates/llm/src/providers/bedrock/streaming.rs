use aws_sdk_bedrockruntime::error::SdkError;
use aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver;
use aws_sdk_bedrockruntime::types::error::ConverseStreamOutputError;
use aws_sdk_bedrockruntime::types::{
    ContentBlockDelta, ContentBlockStart, ConverseStreamOutput, ReasoningContentBlockDelta,
    StopReason as BedrockStopReason, TokenUsage as BedrockTokenUsage,
};
use aws_smithy_types::event_stream::RawMessage;
use futures::{Stream, StreamExt, stream};
use tracing::{error, warn};

use crate::providers::stream_assembler::{StreamAssembler, assemble};
use crate::{LlmError, LlmResponse, ProviderError, StopReason, TokenUsage, Tokens};

pub fn process_bedrock_stream(
    events: impl Stream<Item = crate::Result<ConverseStreamOutput>> + Send,
) -> impl Stream<Item = crate::Result<LlmResponse>> + Send {
    stream::iter([Ok(LlmResponse::Start)]).chain(assemble(events, |event, turn| Ok(decode_event(event, turn))))
}

pub(crate) fn converse_events(
    receiver: EventReceiver<ConverseStreamOutput, ConverseStreamOutputError>,
) -> impl Stream<Item = crate::Result<ConverseStreamOutput>> + Send {
    stream::unfold(receiver, |mut receiver| async move {
        let event = receiver.recv().await.map_err(|e| {
            error!("Bedrock stream recv error: {e}");
            LlmError::from(e)
        });
        event.transpose().map(|event| (event, receiver))
    })
}

impl From<&BedrockTokenUsage> for TokenUsage {
    fn from(usage: &BedrockTokenUsage) -> Self {
        let cache_read = usage.cache_read_input_tokens().and_then(|v| u32::try_from(v).ok()).map(Tokens::from);
        let cache_creation = usage.cache_write_input_tokens().and_then(|v| u32::try_from(v).ok()).map(Tokens::from);
        // Bedrock's input_tokens excludes cached tokens; TokenUsage counts the whole prompt.
        let cached = cache_read.unwrap_or_default() + cache_creation.unwrap_or_default();
        TokenUsage {
            input_tokens: Tokens::from(u32::try_from(usage.input_tokens).unwrap_or(0)) + cached,
            output_tokens: u32::try_from(usage.output_tokens).unwrap_or(0).into(),
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            ..TokenUsage::default()
        }
    }
}

impl From<SdkError<ConverseStreamOutputError, RawMessage>> for LlmError {
    fn from(e: SdkError<ConverseStreamOutputError, RawMessage>) -> Self {
        let message = format!("Bedrock stream error: {e}");
        let provider = match e {
            SdkError::ServiceError(svc) => {
                let inner = svc.err();
                if inner.is_throttling_exception() {
                    ProviderError::rate_limit(message)
                } else if inner.is_service_unavailable_exception()
                    || inner.is_internal_server_exception()
                    || inner.is_model_stream_error_exception()
                {
                    ProviderError::stream_interrupted(message)
                } else {
                    ProviderError::api(message)
                }
            }
            _ => ProviderError::stream_interrupted(message),
        };
        Self::from(provider)
    }
}

fn decode_event(event: ConverseStreamOutput, turn: &mut StreamAssembler<i32>) -> Vec<LlmResponse> {
    let response = match event {
        ConverseStreamOutput::ContentBlockStart(event) => match event.start {
            Some(ContentBlockStart::ToolUse(tool)) => {
                Some(turn.start_tool(event.content_block_index, tool.tool_use_id, tool.name))
            }
            _ => None,
        },
        ConverseStreamOutput::ContentBlockDelta(event) => match event.delta {
            Some(ContentBlockDelta::Text(text)) if !text.is_empty() => Some(LlmResponse::Text { chunk: text }),
            Some(ContentBlockDelta::ToolUse(delta)) => turn.append_tool_args(event.content_block_index, delta.input),
            Some(ContentBlockDelta::ReasoningContent(ReasoningContentBlockDelta::Text(text))) if !text.is_empty() => {
                Some(LlmResponse::Reasoning { chunk: text })
            }
            _ => None,
        },
        ConverseStreamOutput::ContentBlockStop(event) => turn.complete_tool(event.content_block_index),
        ConverseStreamOutput::MessageStop(event) => {
            turn.stop(map_bedrock_stop_reason(&event.stop_reason));
            turn.terminate();
            None
        }
        ConverseStreamOutput::Metadata(event) => {
            event.usage.as_ref().map(|usage| LlmResponse::Usage { tokens: usage.into() })
        }
        ConverseStreamOutput::MessageStart(_) => None,
        other => {
            warn!("Unhandled Bedrock stream event: {other:?}");
            None
        }
    };

    response.into_iter().collect()
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
    use crate::ProviderErrorKind;
    use crate::testing::llm_response;
    use aws_sdk_bedrockruntime::types::{
        ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent, ConversationRole,
        ConverseStreamMetadataEvent, MessageStartEvent, MessageStopEvent, ToolUseBlockDelta, ToolUseBlockStart,
    };

    #[tokio::test]
    async fn test_text_stream() {
        let responses = collect_responses(
            bedrock_stream().text(0, &["Hello", "", " world"]).message_stop(BedrockStopReason::EndTurn).build(),
        )
        .await;

        assert_eq!(responses, llm_response().text(&["Hello", " world"]).build_with_stop_reason(StopReason::EndTurn));
    }

    #[tokio::test]
    async fn test_reasoning_stream() {
        let responses = collect_responses(
            bedrock_stream()
                .reasoning(0, &["thinking"])
                .text(1, &["answer"])
                .message_stop(BedrockStopReason::EndTurn)
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().reasoning(&["thinking"]).text(&["answer"]).build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn test_tool_call_stream() {
        let deltas = [r#"{"query":"#, r#""test"}"#];

        let responses = collect_responses(
            bedrock_stream()
                .tool_call(0, "tool_123", "search", &deltas)
                .message_stop(BedrockStopReason::ToolUse)
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().tool_call("tool_123", "search", &deltas).build_with_stop_reason(StopReason::ToolCalls)
        );
    }

    #[tokio::test]
    async fn stream_closed_mid_tool_call_does_not_complete_it() {
        let responses =
            process_events(bedrock_stream().tool_start(0, "tool_123", "search").tool_delta(0, r#"{"query":"#).build())
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
    async fn test_metadata_after_message_stop_reports_cache_usage() {
        let usage = BedrockTokenUsage::builder()
            .input_tokens(100)
            .output_tokens(50)
            .total_tokens(150)
            .cache_read_input_tokens(40)
            .cache_write_input_tokens(20)
            .build()
            .unwrap();

        let responses =
            collect_responses(bedrock_stream().message_stop(BedrockStopReason::EndTurn).metadata(usage).build()).await;

        let usage = responses.iter().find_map(|response| match response {
            LlmResponse::Usage { tokens } => Some(*tokens),
            _ => None,
        });
        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 160.into(),
                output_tokens: 50.into(),
                cache_read_tokens: Some(40.into()),
                cache_creation_tokens: Some(20.into()),
                ..TokenUsage::default()
            }),
            "cached tokens count toward the prompt"
        );
        assert_eq!(responses.last(), Some(&LlmResponse::done_with_stop_reason(StopReason::EndTurn)));
    }

    #[tokio::test]
    async fn test_metadata_without_cache_fields() {
        let usage = BedrockTokenUsage::builder().input_tokens(10).output_tokens(5).total_tokens(15).build().unwrap();

        let responses =
            collect_responses(bedrock_stream().message_stop(BedrockStopReason::EndTurn).metadata(usage).build()).await;

        assert_eq!(responses, llm_response().usage(10, 5).build_with_stop_reason(StopReason::EndTurn));
    }

    #[tokio::test]
    async fn test_stop_reasons_map_to_llm_stop_reasons() {
        for (bedrock_stop_reason, stop_reason) in [
            (BedrockStopReason::EndTurn, StopReason::EndTurn),
            (BedrockStopReason::StopSequence, StopReason::EndTurn),
            (BedrockStopReason::ToolUse, StopReason::ToolCalls),
            (BedrockStopReason::MaxTokens, StopReason::Length),
            (BedrockStopReason::ModelContextWindowExceeded, StopReason::Length),
            (BedrockStopReason::ContentFiltered, StopReason::ContentFilter),
            (BedrockStopReason::GuardrailIntervened, StopReason::ContentFilter),
        ] {
            let responses = collect_responses(bedrock_stream().message_stop(bedrock_stop_reason).build()).await;

            assert_eq!(responses, llm_response().build_with_stop_reason(stop_reason));
        }
    }

    async fn collect_responses(events: Vec<ConverseStreamOutput>) -> Vec<LlmResponse> {
        process_events(events).await.into_iter().map(Result::unwrap).collect()
    }

    async fn process_events(events: Vec<ConverseStreamOutput>) -> Vec<crate::Result<LlmResponse>> {
        process_bedrock_stream(stream::iter(events.into_iter().map(Ok))).collect().await
    }

    fn bedrock_stream() -> BedrockStreamBuilder {
        BedrockStreamBuilder::default().push(ConverseStreamOutput::MessageStart(
            MessageStartEvent::builder().role(ConversationRole::Assistant).build().unwrap(),
        ))
    }

    #[derive(Default)]
    struct BedrockStreamBuilder {
        events: Vec<ConverseStreamOutput>,
    }

    impl BedrockStreamBuilder {
        fn text(self, index: i32, chunks: &[&str]) -> Self {
            chunks
                .iter()
                .fold(self, |builder, chunk| builder.delta(index, ContentBlockDelta::Text((*chunk).to_string())))
                .block_stop(index)
        }

        fn reasoning(self, index: i32, chunks: &[&str]) -> Self {
            chunks
                .iter()
                .fold(self, |builder, chunk| {
                    builder.delta(
                        index,
                        ContentBlockDelta::ReasoningContent(ReasoningContentBlockDelta::Text((*chunk).to_string())),
                    )
                })
                .block_stop(index)
        }

        fn tool_call(self, index: i32, id: &str, name: &str, argument_deltas: &[&str]) -> Self {
            argument_deltas
                .iter()
                .fold(self.tool_start(index, id, name), |builder, delta| builder.tool_delta(index, delta))
                .block_stop(index)
        }

        fn tool_start(self, index: i32, id: &str, name: &str) -> Self {
            let tool = ToolUseBlockStart::builder().tool_use_id(id).name(name).build().unwrap();
            self.push(ConverseStreamOutput::ContentBlockStart(
                ContentBlockStartEvent::builder()
                    .content_block_index(index)
                    .start(ContentBlockStart::ToolUse(tool))
                    .build()
                    .unwrap(),
            ))
        }

        fn tool_delta(self, index: i32, input: &str) -> Self {
            self.delta(index, ContentBlockDelta::ToolUse(ToolUseBlockDelta::builder().input(input).build().unwrap()))
        }

        fn message_stop(self, stop_reason: BedrockStopReason) -> Self {
            self.push(ConverseStreamOutput::MessageStop(
                MessageStopEvent::builder().stop_reason(stop_reason).build().unwrap(),
            ))
        }

        fn metadata(self, usage: BedrockTokenUsage) -> Self {
            self.push(ConverseStreamOutput::Metadata(ConverseStreamMetadataEvent::builder().usage(usage).build()))
        }

        fn delta(self, index: i32, delta: ContentBlockDelta) -> Self {
            self.push(ConverseStreamOutput::ContentBlockDelta(
                ContentBlockDeltaEvent::builder().content_block_index(index).delta(delta).build().unwrap(),
            ))
        }

        fn block_stop(self, index: i32) -> Self {
            self.push(ConverseStreamOutput::ContentBlockStop(
                ContentBlockStopEvent::builder().content_block_index(index).build().unwrap(),
            ))
        }

        fn push(mut self, event: ConverseStreamOutput) -> Self {
            self.events.push(event);
            self
        }

        fn build(self) -> Vec<ConverseStreamOutput> {
            self.events
        }
    }
}
