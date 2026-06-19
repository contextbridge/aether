use async_openai::types::chat::{
    CompletionUsage, CreateChatCompletionStreamResponse, FinishReason as OpenAiFinishReason,
};
use async_stream;
use tokio_stream::{Stream, StreamExt};
use tracing::debug;

use crate::providers::tool_call_collector::ToolCallCollector;
use crate::{LlmError, LlmResponse, Result, StopReason, TokenUsage};

impl From<CompletionUsage> for TokenUsage {
    fn from(usage: CompletionUsage) -> Self {
        let prompt = usage.prompt_tokens_details.unwrap_or_default();
        let completion = usage.completion_tokens_details.unwrap_or_default();
        TokenUsage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cache_read_tokens: prompt.cached_tokens,
            input_audio_tokens: prompt.audio_tokens,
            reasoning_tokens: completion.reasoning_tokens,
            output_audio_tokens: completion.audio_tokens,
            accepted_prediction_tokens: completion.accepted_prediction_tokens,
            rejected_prediction_tokens: completion.rejected_prediction_tokens,
            ..TokenUsage::default()
        }
    }
}

/// Common stream processing logic that handles tool call state tracking and event emission.
/// Works with standard `async_openai` `CreateChatCompletionStreamResponse` types.
pub fn process_completion_stream<E: Into<LlmError> + Send>(
    mut stream: impl Stream<Item = std::result::Result<CreateChatCompletionStreamResponse, E>> + Send + Unpin,
) -> impl Stream<Item = Result<LlmResponse>> + Send {
    async_stream::stream! {
        let message_id = uuid::Uuid::new_v4().to_string();
        yield Ok(LlmResponse::Start { message_id });

        let mut collector = ToolCallCollector::<u32>::new();
        let mut last_stop_reason: Option<StopReason> = None;

        while let Some(result) = stream.next().await {
            match result {
                Ok(mut response) => {
                    // Emit usage information if available
                    // This must be checked on every chunk since usage may come
                    // in a separate final chunk after finish_reason
                    if let Some(usage) = response.usage {
                        yield Ok(LlmResponse::Usage { tokens: usage.into() });
                    }

                    if let Some(choice) = response.choices.pop() {
                        let delta = choice.delta;

                        if let Some(content) = delta.content
                            && !content.is_empty() {
                                // If we have pending tool calls and now we're getting content,
                                // complete all tool calls first
                                for tool_call in collector.complete_all() {
                                    yield Ok(LlmResponse::ToolRequestComplete { tool_call });
                                }
                                yield Ok(LlmResponse::Text { chunk: content });
                            }

                        if let Some(tool_calls) = delta.tool_calls {
                            for tc in tool_calls {
                                let (id, name, args) = match tc.function {
                                    Some(f) => (tc.id, f.name, f.arguments),
                                    None => (tc.id, None, None),
                                };
                                for response in collector.handle_delta(tc.index, id, name, args) {
                                    yield Ok(response);
                                }
                            }
                        }

                        if let Some(finish_reason) = choice.finish_reason {
                            let finish_reason_str = format!("{finish_reason:?}");
                            debug!("Received finish reason: {finish_reason_str}");
                            last_stop_reason = Some(map_openai_finish_reason(finish_reason));

                            for tool_call in collector.complete_all() {
                                yield Ok(LlmResponse::ToolRequestComplete { tool_call });
                            }
                            // Don't break yet - continue to capture usage from subsequent chunks
                            // OpenRouter sends usage in the last SSE message after finish_reason
                            // See: https://openrouter.ai/docs/guides/usage-accounting
                        }
                    } else {
                        // No choices in this chunk - could be:
                        // 1. Final usage-only chunk after finish_reason (OpenRouter)
                        // 2. Stream is done (some providers)
                        // We already extracted usage above if present
                        debug!("No choices in response, ending stream");
                        for tool_call in collector.complete_all() {
                            yield Ok(LlmResponse::ToolRequestComplete { tool_call });
                        }
                        break;
                    }
                }
                Err(e) => {
                    yield Err(e.into());
                    break;
                }
            }
        }

        yield Ok(LlmResponse::Done {
            stop_reason: last_stop_reason,
        });
    }
}

fn map_openai_finish_reason(reason: OpenAiFinishReason) -> StopReason {
    match reason {
        OpenAiFinishReason::Stop => StopReason::EndTurn,
        OpenAiFinishReason::Length => StopReason::Length,
        OpenAiFinishReason::ToolCalls => StopReason::ToolCalls,
        OpenAiFinishReason::ContentFilter => StopReason::ContentFilter,
        OpenAiFinishReason::FunctionCall => StopReason::FunctionCall,
    }
}
