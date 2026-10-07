use super::types::{ChatCompletionStreamResponse, FinishReason, FunctionCallDelta, ToolCallDelta};
use crate::providers::stream_assembler::{StreamAssembler, assemble};
use crate::{LlmError, LlmResponse, LlmResponseStream, ProviderError, Result, StopReason};
use async_openai::{Client, config::Config};
use futures::{Stream, StreamExt, stream};
use serde::Serialize;
use tracing::{debug, warn};

/// Generic streaming function that accepts any serializable request type.
/// This enables providers to use custom request types while reusing the streaming logic.
pub fn create_custom_stream_generic<T, U>(client: &Client<T>, request: U) -> LlmResponseStream
where
    T: Config + Clone + 'static,
    U: Serialize + Send + 'static,
{
    let client = client.clone();

    Box::pin(async_stream::stream! {
        let stream = match client
            .chat()
            .create_stream_byot::<U, ChatCompletionStreamResponse>(request)
            .await {
            Ok(stream) => stream,
            Err(e) => {
                warn!("create_stream_byot failed: {e}");
                yield Err(LlmError::from(e));
                return;
            }
        };

        // Once the SSE stream has started (HTTP 200), any failure is a fault of
        // the active stream rather than a rejected request, regardless of the
        // error's concrete type. Treat all post-handshake errors as retryable.
        let stream = stream.map(|result| {
            if let Err(ref e) = result {
                warn!("Stream error from API: {e}");
            }
            result.map_err(|e| LlmError::from(ProviderError::stream_interrupted(e.to_string())))
        });

        for await item in process_compatible_stream(stream) {
            yield item;
        }
    })
}

pub fn process_compatible_stream<E: Into<LlmError> + Send>(
    chunks: impl Stream<Item = std::result::Result<ChatCompletionStreamResponse, E>> + Send,
) -> impl Stream<Item = Result<LlmResponse>> + Send {
    let chunks = chunks.map(|chunk| chunk.map_err(Into::into));
    stream::iter([Ok(LlmResponse::Start)]).chain(assemble(chunks, decode_chunk))
}

fn decode_chunk(mut chunk: ChatCompletionStreamResponse, turn: &mut StreamAssembler<i32>) -> Result<Vec<LlmResponse>> {
    turn.allow_eof();

    let mut responses: Vec<_> =
        chunk.usage.map(|usage| LlmResponse::Usage { tokens: usage.into() }).into_iter().collect();
    let Some(choice) = chunk.choices.pop() else {
        return Ok(responses);
    };

    let delta = choice.delta;
    if let Some(reasoning) = delta.reasoning_content.filter(|reasoning| !reasoning.is_empty()) {
        responses.push(LlmResponse::Reasoning { chunk: reasoning });
    }
    if let Some(content) = delta.content.filter(|content| !content.is_empty()) {
        // Text after tool calls means the model has finished them.
        responses.extend(turn.complete_all_tools());
        responses.push(LlmResponse::Text { chunk: content });
    }
    for tool_call in delta.tool_calls.into_iter().flatten() {
        responses.extend(decode_tool_call_delta(tool_call, turn));
    }
    if let Some(finish_reason) = choice.finish_reason {
        debug!("Received finish reason: {finish_reason:?}");
        responses.extend(turn.complete_all_tools());
        turn.stop(map_finish_reason(finish_reason)?);
    }

    Ok(responses)
}

fn decode_tool_call_delta(delta: ToolCallDelta, turn: &mut StreamAssembler<i32>) -> Vec<LlmResponse> {
    let ToolCallDelta { index, id, function, .. } = delta;
    let FunctionCallDelta { name, arguments } = function.unwrap_or_default();

    let start = name.map(|name| turn.start_tool(index, id.unwrap_or_else(|| format!("tool_call_{index}")), name));
    let chunk = arguments.and_then(|chunk| turn.append_tool_args(&index, chunk));
    start.into_iter().chain(chunk).collect()
}

fn map_finish_reason(reason: FinishReason) -> Result<StopReason> {
    match reason {
        FinishReason::Stop => Ok(StopReason::EndTurn),
        FinishReason::Length | FinishReason::ModelContextWindowExceeded => Ok(StopReason::Length),
        FinishReason::ToolCalls => Ok(StopReason::ToolCalls),
        FinishReason::ContentFilter => Ok(StopReason::ContentFilter),
        FinishReason::FunctionCall => Ok(StopReason::FunctionCall),
        FinishReason::Error | FinishReason::NetworkError => {
            Err(ProviderError::server(format!("Provider reported {reason:?} finish reason")).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_compatible::types::{
        ChatCompletionStreamChoice, ChatCompletionStreamResponseDelta, CompletionTokensDetails, PromptTokensDetails,
        Usage,
    };
    use crate::testing::llm_response;
    use crate::{ProviderErrorKind, TokenUsage};

    #[tokio::test]
    async fn error_finish_reasons_yield_retryable_server_errors() {
        for reason in [FinishReason::Error, FinishReason::NetworkError] {
            let responses = process_chunks(chat_stream().finish(reason).build()).await;

            let err = responses.last().unwrap().as_ref().expect_err("an error finish reason must surface as Err");
            assert_eq!(err.provider().map(|provider| provider.kind), Some(ProviderErrorKind::Server), "{err:?}");
            assert!(err.is_retryable(), "{reason:?} must be retryable so the agent recovers");
            assert!(!responses.iter().any(|r| matches!(r, Ok(LlmResponse::Done { .. }))), "no Done after an error");
        }
    }

    #[tokio::test]
    async fn empty_stream_is_interrupted() {
        let responses = process_chunks(vec![]).await;

        assert!(
            matches!(
                responses.as_slice(),
                [Ok(LlmResponse::Start), Err(error)]
                    if error.provider().map(|provider| provider.kind) == Some(ProviderErrorKind::StreamInterrupted)
                        && error.is_retryable()
            ),
            "{responses:?}"
        );
    }

    #[tokio::test]
    async fn stream_error_mid_tool_call_does_not_complete_it() {
        let chunks = chat_stream().tool_start(0, "call_1", "bash").tool_args(0, r#"{"command":"#).build();
        let items = chunks.into_iter().map(Ok).chain([Err(ProviderError::stream_interrupted("connection lost"))]);

        let responses: Vec<_> = process_compatible_stream(stream::iter(items)).collect().await;

        assert!(
            matches!(
                responses.as_slice(),
                [
                    Ok(LlmResponse::Start),
                    Ok(LlmResponse::ToolRequestStart { .. }),
                    Ok(LlmResponse::ToolRequestArg { .. }),
                    Err(_)
                ]
            ),
            "{responses:?}"
        );
    }

    #[tokio::test]
    async fn test_reasoning_chunks() {
        let responses = collect_responses(chat_stream().reasoning(&["thinking"]).build()).await;

        assert_eq!(responses, llm_response().reasoning(&["thinking"]).build());
    }

    #[tokio::test]
    async fn test_tool_call_stream() {
        let responses = collect_responses(
            chat_stream().tool_start(0, "call_1", "tool").tool_args(0, "{}").finish(FinishReason::ToolCalls).build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().tool_call("call_1", "tool", &["{}"]).build_with_stop_reason(StopReason::ToolCalls)
        );
    }

    #[tokio::test]
    async fn tool_call_delta_without_id_gets_index_based_id() {
        let responses = collect_responses(
            chat_stream().tool_delta(3, None, Some("tool"), Some("{}")).finish(FinishReason::ToolCalls).build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().tool_call("tool_call_3", "tool", &["{}"]).build_with_stop_reason(StopReason::ToolCalls)
        );
    }

    #[tokio::test]
    async fn parallel_tool_calls_complete_in_index_order() {
        let responses = collect_responses(
            chat_stream()
                .tool_start(0, "call_1", "function_a")
                .tool_start(1, "call_2", "function_b")
                .tool_args(0, r#"{"param":"#)
                .tool_args(1, r#"{"value":"#)
                .tool_args(0, r#""test"}"#)
                .tool_args(1, "42}")
                .finish(FinishReason::ToolCalls)
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            vec![
                LlmResponse::Start,
                LlmResponse::tool_request_start("call_1", "function_a"),
                LlmResponse::tool_request_start("call_2", "function_b"),
                LlmResponse::tool_request_arg("call_1", r#"{"param":"#),
                LlmResponse::tool_request_arg("call_2", r#"{"value":"#),
                LlmResponse::tool_request_arg("call_1", r#""test"}"#),
                LlmResponse::tool_request_arg("call_2", "42}"),
                LlmResponse::tool_request_complete("call_1", "function_a", r#"{"param":"test"}"#),
                LlmResponse::tool_request_complete("call_2", "function_b", r#"{"value":42}"#),
                LlmResponse::done_with_stop_reason(StopReason::ToolCalls),
            ]
        );
    }

    #[tokio::test]
    async fn text_after_tool_call_completes_it_first() {
        let responses = collect_responses(
            chat_stream()
                .tool_start(0, "call_1", "test_func")
                .tool_args(0, "{}")
                .text(&["Here is the result"])
                .finish(FinishReason::Stop)
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response()
                .tool_call("call_1", "test_func", &["{}"])
                .text(&["Here is the result"])
                .build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn test_zai_shape_only_populates_cache_read() {
        let tokens = collect_first_usage(
            chat_stream()
                .usage(Usage {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    total_tokens: 150,
                    prompt_tokens_details: Some(PromptTokensDetails { cached_tokens: Some(30), ..Default::default() }),
                    completion_tokens_details: None,
                })
                .build(),
        )
        .await
        .expect("usage event");

        assert_eq!(
            tokens,
            TokenUsage {
                input_tokens: 100.into(),
                output_tokens: 50.into(),
                cache_read_tokens: Some(30.into()),
                ..TokenUsage::default()
            }
        );
    }

    #[tokio::test]
    async fn test_openrouter_shape_populates_all_fields() {
        let tokens = collect_first_usage(
            chat_stream()
                .usage(Usage {
                    prompt_tokens: 1000,
                    completion_tokens: 500,
                    total_tokens: 1500,
                    prompt_tokens_details: Some(PromptTokensDetails {
                        cached_tokens: Some(100),
                        cache_write_tokens: Some(50),
                        audio_tokens: Some(10),
                        video_tokens: Some(5),
                    }),
                    completion_tokens_details: Some(CompletionTokensDetails {
                        reasoning_tokens: Some(300),
                        audio_tokens: Some(8),
                        accepted_prediction_tokens: Some(2),
                        rejected_prediction_tokens: Some(1),
                    }),
                })
                .build(),
        )
        .await
        .expect("usage event");

        assert_eq!(
            tokens,
            TokenUsage {
                input_tokens: 1000.into(),
                output_tokens: 500.into(),
                cache_read_tokens: Some(100.into()),
                cache_creation_tokens: Some(50.into()),
                input_audio_tokens: Some(10.into()),
                input_video_tokens: Some(5.into()),
                reasoning_tokens: Some(300.into()),
                output_audio_tokens: Some(8.into()),
                accepted_prediction_tokens: Some(2.into()),
                rejected_prediction_tokens: Some(1.into()),
            }
        );
    }

    #[tokio::test]
    async fn test_openai_shape_populates_input_audio_and_completion_details() {
        let tokens = collect_first_usage(
            chat_stream()
                .usage(Usage {
                    prompt_tokens: 200,
                    completion_tokens: 100,
                    total_tokens: 300,
                    prompt_tokens_details: Some(PromptTokensDetails {
                        cached_tokens: Some(80),
                        audio_tokens: Some(12),
                        ..Default::default()
                    }),
                    completion_tokens_details: Some(CompletionTokensDetails {
                        reasoning_tokens: Some(40),
                        accepted_prediction_tokens: Some(5),
                        ..Default::default()
                    }),
                })
                .build(),
        )
        .await
        .expect("usage event");

        assert_eq!(tokens.cache_read_tokens.map(crate::Tokens::get), Some(80));
        assert_eq!(tokens.cache_creation_tokens, None);
        assert_eq!(tokens.input_audio_tokens.map(crate::Tokens::get), Some(12));
        assert_eq!(tokens.reasoning_tokens.map(crate::Tokens::get), Some(40));
        assert_eq!(tokens.accepted_prediction_tokens.map(crate::Tokens::get), Some(5));
    }

    #[tokio::test]
    async fn test_context_window_exceeded_maps_to_length() {
        let chunk: ChatCompletionStreamResponse = serde_json::from_str(
            r#"{
                "id": "chunk_1",
                "created": 1,
                "model": "glm-5",
                "choices": [{
                    "index": 0,
                    "finish_reason": "model_context_window_exceeded",
                    "delta": {
                        "role": "assistant",
                        "content": ""
                    }
                }]
            }"#,
        )
        .expect("response should deserialize");

        let responses = collect_responses(chat_stream().push(chunk).build()).await;

        assert_eq!(responses, llm_response().build_with_stop_reason(StopReason::Length));
    }

    async fn collect_responses(chunks: Vec<ChatCompletionStreamResponse>) -> Vec<LlmResponse> {
        process_chunks(chunks).await.into_iter().map(Result::unwrap).collect()
    }

    async fn process_chunks(chunks: Vec<ChatCompletionStreamResponse>) -> Vec<Result<LlmResponse>> {
        process_compatible_stream(stream::iter(chunks.into_iter().map(Ok::<_, LlmError>))).collect().await
    }

    async fn collect_first_usage(chunks: Vec<ChatCompletionStreamResponse>) -> Option<TokenUsage> {
        collect_responses(chunks).await.into_iter().find_map(|response| match response {
            LlmResponse::Usage { tokens } => Some(tokens),
            _ => None,
        })
    }

    fn chat_stream() -> ChatStreamBuilder {
        ChatStreamBuilder::default()
    }

    #[derive(Default)]
    struct ChatStreamBuilder {
        chunks: Vec<ChatCompletionStreamResponse>,
    }

    impl ChatStreamBuilder {
        fn text(self, chunks: &[&str]) -> Self {
            chunks.iter().fold(self, |builder, text| {
                builder.delta(ChatCompletionStreamResponseDelta {
                    content: Some((*text).to_string()),
                    ..Default::default()
                })
            })
        }

        fn reasoning(self, chunks: &[&str]) -> Self {
            chunks.iter().fold(self, |builder, reasoning| {
                builder.delta(ChatCompletionStreamResponseDelta {
                    reasoning_content: Some((*reasoning).to_string()),
                    ..Default::default()
                })
            })
        }

        fn tool_start(self, index: i32, id: &str, name: &str) -> Self {
            self.tool_delta(index, Some(id), Some(name), None)
        }

        fn tool_args(self, index: i32, arguments: &str) -> Self {
            self.tool_delta(index, None, None, Some(arguments))
        }

        fn tool_delta(self, index: i32, id: Option<&str>, name: Option<&str>, arguments: Option<&str>) -> Self {
            self.delta(ChatCompletionStreamResponseDelta {
                tool_calls: Some(vec![ToolCallDelta {
                    index,
                    id: id.map(ToString::to_string),
                    tool_type: Some("function".to_string()),
                    function: Some(FunctionCallDelta {
                        name: name.map(ToString::to_string),
                        arguments: arguments.map(ToString::to_string),
                    }),
                }]),
                ..Default::default()
            })
        }

        fn finish(self, reason: FinishReason) -> Self {
            self.push(chunk(vec![choice(ChatCompletionStreamResponseDelta::default(), Some(reason))], None))
        }

        fn usage(self, usage: Usage) -> Self {
            self.push(chunk(vec![], Some(usage)))
        }

        fn delta(self, delta: ChatCompletionStreamResponseDelta) -> Self {
            self.push(chunk(vec![choice(delta, None)], None))
        }

        fn push(mut self, chunk: ChatCompletionStreamResponse) -> Self {
            self.chunks.push(chunk);
            self
        }

        fn build(self) -> Vec<ChatCompletionStreamResponse> {
            self.chunks
        }
    }

    fn chunk(choices: Vec<ChatCompletionStreamChoice>, usage: Option<Usage>) -> ChatCompletionStreamResponse {
        ChatCompletionStreamResponse {
            id: "chunk".to_string(),
            choices,
            created: 1,
            model: "test".to_string(),
            system_fingerprint: None,
            object: "chat.completion.chunk".to_string(),
            usage,
        }
    }

    fn choice(
        delta: ChatCompletionStreamResponseDelta,
        finish_reason: Option<FinishReason>,
    ) -> ChatCompletionStreamChoice {
        ChatCompletionStreamChoice { index: 0, delta, finish_reason, logprobs: None }
    }
}
