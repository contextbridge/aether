use async_openai::types::responses::{OutputItem, ReasoningItem, ResponseUsage, Status};
use serde::{Deserialize, Deserializer, de::Error as _};
use tracing::debug;

use crate::providers::response_stream::StreamAssembler;
use crate::{LlmResponse, ProviderError, ProviderErrorKind, Result, StopReason, TokenUsage};

#[derive(Debug)]
pub struct ResponsesUsage {
    usage: ResponseUsage,
    cache_write_tokens: Option<u32>,
}

impl<'de> Deserialize<'de> for ResponsesUsage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let extension = serde_json::from_value::<ResponsesUsageExtension>(value.clone()).map_err(D::Error::custom)?;
        let usage = serde_json::from_value(value).map_err(D::Error::custom)?;

        Ok(Self { usage, cache_write_tokens: extension.input_tokens_details.cache_write_tokens })
    }
}

impl From<ResponsesUsage> for TokenUsage {
    fn from(usage: ResponsesUsage) -> Self {
        TokenUsage {
            input_tokens: usage.usage.input_tokens.into(),
            output_tokens: usage.usage.output_tokens.into(),
            cache_read_tokens: Some(usage.usage.input_tokens_details.cached_tokens.into()),
            cache_creation_tokens: usage.cache_write_tokens.map(Into::into),
            reasoning_tokens: Some(usage.usage.output_tokens_details.reasoning_tokens.into()),
            ..TokenUsage::default()
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesStreamEvent {
    #[serde(rename = "response.created")]
    Created,
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(ResponsesTextDeltaEvent),
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded(ResponsesOutputItemEvent),
    #[serde(rename = "response.output_item.done")]
    OutputItemDone(ResponsesOutputItemEvent),
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta(ResponsesFunctionCallArgumentsDeltaEvent),
    #[serde(rename = "response.reasoning_summary_text.delta", alias = "response.reasoning_text.delta")]
    ReasoningTextDelta(ResponsesTextDeltaEvent),
    #[serde(rename = "response.completed")]
    Completed(ResponsesCompletedEvent),
    #[serde(rename = "response.incomplete")]
    Incomplete(ResponsesCompletedEvent),
    #[serde(rename = "response.failed")]
    Failed(ResponsesFailedEvent),
    #[serde(rename = "error")]
    Error(ResponsesErrorEvent),
    #[serde(other)]
    Ignored,
}

impl ResponsesStreamEvent {
    /// Whether this event may legitimately arrive before `response.created`:
    /// event types we ignore, and failures the endpoint reports *instead of*
    /// opening a response. Rejecting those would replace the server's own
    /// message with a generic interrupt.
    fn may_precede_creation(&self) -> bool {
        matches!(self, Self::Ignored | Self::Error(_) | Self::Failed(_))
    }
}

#[derive(Debug, Deserialize)]
pub struct ResponsesFailedEvent {
    pub response: ResponsesFailed,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesFailed {
    #[serde(default)]
    pub error: Option<ResponsesErrorEvent>,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesTextDeltaEvent {
    pub delta: String,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesOutputItemEvent {
    pub output_index: u32,
    pub item: OutputItem,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesFunctionCallArgumentsDeltaEvent {
    pub output_index: u32,
    pub delta: String,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesCompletedEvent {
    pub response: ResponsesCompleted,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesCompleted {
    #[serde(default)]
    pub usage: Option<ResponsesUsage>,
    #[serde(default)]
    pub status: Option<Status>,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesErrorEvent {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: String,
}

pub(crate) fn decode_responses() -> impl FnMut(String, &mut StreamAssembler<u32>) -> Result<Vec<LlmResponse>> + Send {
    let mut started = false;
    move |data, turn| {
        let event = serde_json::from_str::<ResponsesStreamEvent>(&data).map_err(|error| {
            debug!(data, %error, "Failed to decode Responses SSE event");
            ProviderError::stream_interrupted(format!("Invalid Responses SSE event: {error}"))
        })?;
        if matches!(event, ResponsesStreamEvent::Created) {
            started = true;
        } else if !started && !event.may_precede_creation() {
            return Err(
                ProviderError::stream_interrupted("Responses stream emitted data before response.created").into()
            );
        }
        decode_event(event, turn)
    }
}

#[derive(Deserialize, Default)]
struct ResponsesUsageExtension {
    #[serde(default)]
    input_tokens_details: ResponsesInputTokenDetailsExtension,
}

#[derive(Deserialize, Default)]
struct ResponsesInputTokenDetailsExtension {
    #[serde(default)]
    cache_write_tokens: Option<u32>,
}

fn map_responses_error(code: Option<String>, message: String, fallback: ProviderErrorKind) -> ProviderError {
    let kind = match code.as_deref() {
        Some("server_error") => ProviderErrorKind::Server,
        Some("rate_limit_exceeded") => ProviderErrorKind::RateLimit,
        _ => fallback,
    };
    ProviderError::new(kind, message).with_code(code)
}

fn decode_event(event: ResponsesStreamEvent, turn: &mut StreamAssembler<u32>) -> Result<Vec<LlmResponse>> {
    let incomplete = matches!(&event, ResponsesStreamEvent::Incomplete(_));

    let responses = match event {
        ResponsesStreamEvent::OutputTextDelta(e) if !e.delta.is_empty() => vec![LlmResponse::Text { chunk: e.delta }],
        ResponsesStreamEvent::OutputItemAdded(e) => match e.item {
            OutputItem::FunctionCall(call) => vec![turn.start_tool(e.output_index, call.call_id, call.name)],
            _ => vec![],
        },
        ResponsesStreamEvent::FunctionCallArgumentsDelta(e) => {
            turn.append_tool_args(&e.output_index, e.delta).into_iter().collect()
        }
        ResponsesStreamEvent::ReasoningTextDelta(e) if !e.delta.is_empty() => {
            vec![LlmResponse::Reasoning { chunk: e.delta }]
        }
        ResponsesStreamEvent::OutputItemDone(e) => match e.item {
            OutputItem::FunctionCall(call) => turn.complete_tool_with(&e.output_index, call.into()),
            OutputItem::Reasoning(ReasoningItem { id: Some(id), encrypted_content: Some(content), .. }) => {
                vec![LlmResponse::EncryptedReasoning { id, content }]
            }
            _ => vec![],
        },
        ResponsesStreamEvent::Completed(e) | ResponsesStreamEvent::Incomplete(e) => {
            match e.response.status {
                Some(Status::Completed) => turn.stop(StopReason::EndTurn),
                Some(Status::Incomplete) => turn.stop(StopReason::Length),
                _ if incomplete => turn.stop(StopReason::Length),
                _ => {}
            }
            turn.finish_now();
            e.response.usage.map(|usage| LlmResponse::Usage { tokens: usage.into() }).into_iter().collect()
        }
        ResponsesStreamEvent::Failed(e) => {
            let error = e.response.error.map_or_else(
                || ProviderError::new(ProviderErrorKind::Api, "Unknown Responses API failure"),
                |e| map_responses_error(e.code, e.message, ProviderErrorKind::Api),
            );
            return Err(error.into());
        }
        ResponsesStreamEvent::Error(e) => {
            let message = format!("Responses API error: {}", e.message);
            return Err(map_responses_error(e.code, message, ProviderErrorKind::Unknown).into());
        }
        ResponsesStreamEvent::Created
        | ResponsesStreamEvent::Ignored
        | ResponsesStreamEvent::OutputTextDelta(_)
        | ResponsesStreamEvent::ReasoningTextDelta(_) => vec![],
    };

    Ok(responses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_connection::DEFAULT_STREAM_IDLE_TIMEOUT;
    use crate::providers::response_stream::{OpenedStream, response_stream};
    use crate::testing::llm_response;
    use crate::{LlmError, LlmResponseStream};
    use futures::{FutureExt, Stream, StreamExt, stream};
    use serde_json::{Value, json};
    use std::future::ready;

    #[tokio::test]
    async fn test_text_stream() {
        let responses =
            collect_responses(responses_stream().created().text(&["Hello", " world"]).completed().build()).await;

        assert_eq!(responses, llm_response().text(&["Hello", " world"]).build_with_stop_reason(StopReason::EndTurn));
    }

    #[tokio::test]
    async fn test_tool_call_stream() {
        let deltas = [r#"{"path":"#, r#""foo.rs"}"#];

        let responses = collect_responses(
            responses_stream().created().tool_call(0, "call_1", "read_file", &deltas).completed().build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().tool_call("call_1", "read_file", &deltas).build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn interleaved_function_calls_complete_independently() {
        let read_deltas = [r#"{"path":"#, r#""a.rs"}"#];
        let bash_deltas = [r#"{"command":"#, r#""ls"}"#];
        let read_arguments = read_deltas.concat();
        let bash_arguments = bash_deltas.concat();

        let responses = collect_responses(
            responses_stream()
                .created()
                .tool_start(0, "call_a", "read_file")
                .tool_start(1, "call_b", "bash")
                .tool_delta(0, read_deltas[0])
                .tool_delta(1, bash_deltas[0])
                .tool_delta(0, read_deltas[1])
                .tool_args_done(0, &read_arguments)
                .tool_done(0, "call_a", "read_file", &read_arguments)
                .tool_delta(1, bash_deltas[1])
                .tool_args_done(1, &bash_arguments)
                .tool_done(1, "call_b", "bash", &bash_arguments)
                .completed()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            vec![
                LlmResponse::Start,
                LlmResponse::tool_request_start("call_a", "read_file"),
                LlmResponse::tool_request_start("call_b", "bash"),
                LlmResponse::tool_request_arg("call_a", read_deltas[0]),
                LlmResponse::tool_request_arg("call_b", bash_deltas[0]),
                LlmResponse::tool_request_arg("call_a", read_deltas[1]),
                LlmResponse::tool_request_complete("call_a", "read_file", &read_arguments),
                LlmResponse::tool_request_arg("call_b", bash_deltas[1]),
                LlmResponse::tool_request_complete("call_b", "bash", &bash_arguments),
                LlmResponse::done_with_stop_reason(StopReason::EndTurn),
            ]
        );
    }

    #[tokio::test]
    async fn function_call_without_done_item_completes_with_the_response() {
        let arguments = r#"{"path":"foo.rs"}"#;

        let responses = collect_responses(
            responses_stream()
                .created()
                .tool_start(0, "call_1", "read_file")
                .tool_delta(0, arguments)
                .completed()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().tool_call("call_1", "read_file", &[arguments]).build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn stream_closed_mid_function_call_does_not_complete_it() {
        let responses = process_events(
            responses_stream().created().tool_start(0, "call_1", "bash").tool_delta(0, r#"{"command":"ls"#).build(),
        )
        .await;

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
    async fn multiple_function_calls_without_argument_deltas_use_completed_item_arguments() {
        let responses = collect_responses(
            responses_stream()
                .created()
                .tool_call_without_deltas(0, "call_a", "read_file", r#"{"filePath":"a.rs"}"#)
                .tool_call_without_deltas(1, "call_b", "bash", r#"{"command":"ls"}"#)
                .completed()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response()
                .tool_call_without_deltas("call_a", "read_file", r#"{"filePath":"a.rs"}"#)
                .tool_call_without_deltas("call_b", "bash", r#"{"command":"ls"}"#)
                .build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn completed_item_without_added_event_starts_the_tool_call() {
        let arguments = r#"{"path":"foo.rs"}"#;

        let responses = collect_responses(
            responses_stream().created().tool_done(0, "call_1", "read_file", arguments).completed().build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response()
                .tool_call_without_deltas("call_1", "read_file", arguments)
                .build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn completed_item_arguments_supersede_streamed_arguments() {
        let arguments = r#"{"path":"foo.rs"}"#;
        let responses = collect_responses(
            responses_stream()
                .created()
                .tool_start(0, "call_1", "read_file")
                .tool_delta(0, "{}")
                .tool_done(0, "call_1", "read_file", arguments)
                .completed()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            vec![
                LlmResponse::Start,
                LlmResponse::tool_request_start("call_1", "read_file"),
                LlmResponse::tool_request_arg("call_1", "{}"),
                LlmResponse::tool_request_complete("call_1", "read_file", arguments),
                LlmResponse::done_with_stop_reason(StopReason::EndTurn),
            ]
        );
    }

    #[tokio::test]
    async fn completed_item_with_empty_arguments_keeps_the_streamed_arguments() {
        let arguments = r#"{"path":"foo.rs"}"#;

        let responses = collect_responses(
            responses_stream()
                .created()
                .tool_start(0, "call_1", "read_file")
                .tool_delta(0, arguments)
                .tool_done(0, "call_1", "read_file", "")
                .completed()
                .build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().tool_call("call_1", "read_file", &[arguments]).build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn terminal_events_complete_without_waiting_for_the_connection_to_close() {
        for (events, stop_reason) in [
            (responses_stream().created().text(&["done"]).completed(), StopReason::EndTurn),
            (responses_stream().created().text(&["done"]).incomplete(), StopReason::Length),
        ] {
            let events = events.build().into_iter().map(|event| Ok(event.to_string()));
            let events = stream::iter(events).chain(stream::pending());
            let responses = process_response_stream(events)
                .collect::<Vec<_>>()
                .now_or_never()
                .expect("a terminal event must finish the response without waiting for another upstream event");

            assert_eq!(
                responses.into_iter().collect::<Result<Vec<_>>>().unwrap(),
                llm_response().text(&["done"]).build_with_stop_reason(stop_reason)
            );
        }
    }

    #[tokio::test]
    async fn events_after_the_terminal_event_are_not_read() {
        let responses =
            collect_responses(responses_stream().created().text(&["done"]).completed().text(&["late"]).build()).await;

        assert_eq!(responses, llm_response().text(&["done"]).build_with_stop_reason(StopReason::EndTurn));
    }

    #[tokio::test]
    async fn error_event_after_creation_stays_retryable() {
        let responses = process_events(responses_stream().created().error(None, "Rate limit exceeded").build()).await;

        assert!(matches!(responses[0], Ok(LlmResponse::Start)));
        let err = responses[1].as_ref().expect_err("expected error event to surface as Err");
        assert_eq!(err.provider().map(|provider| provider.kind), Some(ProviderErrorKind::Unknown), "got {err:?}");
        assert!(err.is_retryable(), "an uncoded top-level error event must stay retryable so the agent can recover");
    }

    #[tokio::test]
    async fn error_events_map_codes_to_retryable_kinds() {
        for (code, kind) in [
            (None, ProviderErrorKind::Unknown),
            (Some("bogus"), ProviderErrorKind::Unknown),
            (Some("rate_limit_exceeded"), ProviderErrorKind::RateLimit),
            (Some("server_error"), ProviderErrorKind::Server),
        ] {
            let responses = process_events(responses_stream().error(code, "slow down").build()).await;

            let err = responses[1].as_ref().expect_err("expected the error event to surface as Err");
            assert_eq!(err.provider().map(|provider| provider.kind), Some(kind), "{code:?}: {err:?}");
            assert_eq!(err.provider().and_then(|provider| provider.code.as_deref()), code);
            assert!(err.is_retryable(), "{code:?}: {err:?}");
            assert!(err.to_string().contains("slow down"), "server message was dropped: {err}");
        }
    }

    #[tokio::test]
    async fn failed_events_map_codes_to_kinds() {
        for (code, kind, retryable) in [
            (Some("server_error"), ProviderErrorKind::Server, true),
            (Some("rate_limit_exceeded"), ProviderErrorKind::RateLimit, true),
            (Some("invalid_prompt"), ProviderErrorKind::Api, false),
            (Some("bogus"), ProviderErrorKind::Api, false),
            (None, ProviderErrorKind::Api, false),
        ] {
            let responses = process_events(responses_stream().failed(code, "model overloaded").build()).await;

            let err = responses[1].as_ref().expect_err("expected the failure to surface as Err");
            assert_eq!(err.provider().map(|provider| provider.kind), Some(kind), "{code:?}: {err:?}");
            assert_eq!(err.provider().and_then(|provider| provider.code.as_deref()), code);
            assert_eq!(err.is_retryable(), retryable, "{code:?}: {err:?}");
            assert!(err.to_string().contains("model overloaded"), "server message was dropped: {err}");
        }
    }

    #[tokio::test]
    async fn test_reasoning_delta() {
        let deltas = ["Thinking about", " the problem"];

        let responses = collect_responses(responses_stream().created().reasoning(&deltas).completed().build()).await;

        assert_eq!(responses, llm_response().reasoning(&deltas).build_with_stop_reason(StopReason::EndTurn));
    }

    #[tokio::test]
    async fn test_incomplete_status_gives_length_stop_reason() {
        let responses = collect_responses(responses_stream().created().incomplete().build()).await;

        assert_eq!(responses, llm_response().build_with_stop_reason(StopReason::Length));
    }

    #[tokio::test]
    async fn test_stream_error_propagation_is_retryable() {
        let events: Vec<Result<String>> = vec![Err(ProviderError::stream_interrupted("connection lost").into())];

        let responses: Vec<_> = process_response_stream(tokio_stream::iter(events)).collect().await;

        assert!(matches!(responses[0], Ok(LlmResponse::Start)));
        let err = responses[1].as_ref().expect_err("expected upstream Err to surface as Err");
        assert_eq!(
            err.provider().map(|provider| provider.kind),
            Some(ProviderErrorKind::StreamInterrupted),
            "got {err:?}"
        );
        assert_eq!(responses.len(), 2);
        assert!(err.is_retryable(), "mid-stream interrupts must be retryable");
    }

    #[tokio::test]
    async fn data_before_creation_is_interrupted() {
        let responses = process_events(responses_stream().text(&["leaked"]).build()).await;

        assert_eq!(
            responses[1].as_ref().err().and_then(LlmError::provider).map(|provider| provider.kind),
            Some(ProviderErrorKind::StreamInterrupted),
            "{responses:?}"
        );
    }

    #[tokio::test]
    async fn stream_without_terminal_event_is_interrupted() {
        let responses = process_events(responses_stream().created().text(&["partial"]).build()).await;

        assert!(matches!(responses[0], Ok(LlmResponse::Start)));
        assert!(matches!(responses[1], Ok(LlmResponse::Text { .. })));
        assert_eq!(
            responses[2].as_ref().err().and_then(LlmError::provider).map(|provider| provider.kind),
            Some(ProviderErrorKind::StreamInterrupted)
        );
        assert!(!responses.iter().any(|response| matches!(response, Ok(LlmResponse::Done { .. }))));
    }

    #[tokio::test]
    async fn captured_responses_fixture_uses_the_shared_processor() {
        let responses = process_fixture(include_str!("../../../tests/fixtures/openai_responses/01_minimal.sse")).await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let usage = find_usage(&responses).expect("fixture should report usage");
        assert!(!usage.input_tokens.is_zero(), "input_tokens should be > 0: {usage:?}");
        assert!(!usage.output_tokens.is_zero(), "output_tokens should be > 0: {usage:?}");
        assert!(matches!(responses.last(), Some(Ok(LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }))));
    }

    #[tokio::test]
    async fn captured_reasoning_fixture_preserves_reasoning_usage() {
        let responses =
            process_fixture(include_str!("../../../tests/fixtures/openai_responses/02_reasoning.sse")).await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let usage = find_usage(&responses).expect("fixture should report usage");
        assert!(!usage.input_tokens.is_zero(), "input_tokens should be > 0: {usage:?}");
        assert!(!usage.output_tokens.is_zero(), "output_tokens should be > 0: {usage:?}");
        assert!(usage.reasoning_tokens.is_some_and(|tokens| !tokens.is_zero()), "{usage:?}");
    }

    #[tokio::test]
    async fn captured_mantle_fixture_preserves_cache_write_usage() {
        let responses =
            process_fixture(include_str!("../../../tests/fixtures/openai_responses/03_mantle_cache_write.sse")).await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let usage = find_usage(&responses).expect("fixture should report usage");
        assert_eq!(usage.cache_creation_tokens.map(crate::Tokens::get), Some(1024));
    }

    #[tokio::test]
    async fn test_encrypted_reasoning_from_output_item_done() {
        let responses = collect_responses(
            responses_stream().created().reasoning_item(0, Some("enc-blob-data")).completed().build(),
        )
        .await;

        assert_eq!(
            responses,
            llm_response().encrypted_reasoning("r_1", "enc-blob-data").build_with_stop_reason(StopReason::EndTurn)
        );
    }

    #[tokio::test]
    async fn test_output_item_done_without_encrypted_content_is_ignored() {
        let responses =
            collect_responses(responses_stream().created().reasoning_item(0, None).completed().build()).await;

        assert_eq!(responses, llm_response().build_with_stop_reason(StopReason::EndTurn));
    }

    #[tokio::test]
    async fn test_usage_forwards_reasoning_and_cache_read() {
        let responses =
            process_events(responses_stream().created().completed_with_usage(&usage_json(120, 80, 50, 30)).build())
                .await;

        assert_eq!(
            find_usage(&responses),
            Some(TokenUsage {
                input_tokens: 120.into(),
                output_tokens: 80.into(),
                cache_read_tokens: Some(50.into()),
                reasoning_tokens: Some(30.into()),
                ..TokenUsage::default()
            })
        );
    }

    #[tokio::test]
    async fn test_completed_without_output_deserializes_usage_and_stop_reason() {
        let responses = collect_responses(
            responses_stream()
                .created()
                .push(json!({
                    "type": "response.completed",
                    "sequence_number": 1,
                    "response": {
                        "id": "resp_1",
                        "object": "response",
                        "created_at": 1_000_u64,
                        "status": "completed",
                        "background": false,
                        "completed_at": 2_000_u64,
                        "error": null,
                        "model": "test-model",
                        "usage": usage_json(100, 20, 0, 10)
                    }
                }))
                .build(),
        )
        .await;

        assert!(matches!(
            responses.iter().find(|response| matches!(response, LlmResponse::Usage { .. })),
            Some(LlmResponse::Usage { tokens })
                if tokens.input_tokens.get() == 100
                    && tokens.output_tokens.get() == 20
                    && tokens.reasoning_tokens.map(crate::Tokens::get) == Some(10)
        ));
        assert!(matches!(responses.last().unwrap(), LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }));
    }

    async fn collect_responses(events: Vec<Value>) -> Vec<LlmResponse> {
        process_events(events).await.into_iter().map(Result::unwrap).collect()
    }

    async fn process_events(events: Vec<Value>) -> Vec<Result<LlmResponse>> {
        process_data(events.into_iter().map(|event| event.to_string()).collect()).await
    }

    async fn process_fixture(sse: &str) -> Vec<Result<LlmResponse>> {
        let data = sse.lines().filter_map(|line| line.strip_prefix("data: ")).filter(|data| *data != "[DONE]");
        process_data(data.map(str::to_string).collect()).await
    }

    async fn process_data(data: Vec<String>) -> Vec<Result<LlmResponse>> {
        process_response_stream(tokio_stream::iter(data.into_iter().map(Ok))).collect().await
    }

    fn process_response_stream(data: impl Stream<Item = Result<String>> + Send + 'static) -> LlmResponseStream {
        response_stream(ready(Ok(OpenedStream::new(data))), decode_responses(), DEFAULT_STREAM_IDLE_TIMEOUT)
    }

    fn find_usage(responses: &[Result<LlmResponse>]) -> Option<TokenUsage> {
        responses.iter().find_map(|response| match response {
            Ok(LlmResponse::Usage { tokens }) => Some(*tokens),
            _ => None,
        })
    }

    fn responses_stream() -> ResponsesStreamBuilder {
        ResponsesStreamBuilder::default()
    }

    #[derive(Default)]
    struct ResponsesStreamBuilder {
        events: Vec<Value>,
    }

    impl ResponsesStreamBuilder {
        fn created(self) -> Self {
            self.push(json!({ "type": "response.created" }))
        }

        fn text(self, deltas: &[&str]) -> Self {
            deltas.iter().fold(self, |builder, delta| {
                builder.push(json!({ "type": "response.output_text.delta", "delta": delta }))
            })
        }

        fn reasoning(self, deltas: &[&str]) -> Self {
            deltas.iter().fold(self, |builder, delta| {
                builder.push(json!({ "type": "response.reasoning_summary_text.delta", "delta": delta }))
            })
        }

        fn reasoning_item(self, output_index: u32, encrypted_content: Option<&str>) -> Self {
            self.push(json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": { "type": "reasoning", "id": "r_1", "summary": [], "encrypted_content": encrypted_content }
            }))
        }

        fn tool_call(self, output_index: u32, call_id: &str, name: &str, argument_deltas: &[&str]) -> Self {
            let arguments = argument_deltas.concat();
            argument_deltas
                .iter()
                .fold(self.tool_start(output_index, call_id, name), |builder, delta| {
                    builder.tool_delta(output_index, delta)
                })
                .tool_args_done(output_index, &arguments)
                .tool_done(output_index, call_id, name, &arguments)
        }

        fn tool_call_without_deltas(self, output_index: u32, call_id: &str, name: &str, arguments: &str) -> Self {
            self.tool_start(output_index, call_id, name).tool_args_done(output_index, arguments).tool_done(
                output_index,
                call_id,
                name,
                arguments,
            )
        }

        fn tool_start(self, output_index: u32, call_id: &str, name: &str) -> Self {
            self.push(json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": function_call_item(call_id, name, "", "in_progress")
            }))
        }

        fn tool_delta(self, output_index: u32, delta: &str) -> Self {
            self.push(json!({
                "type": "response.function_call_arguments.delta",
                "output_index": output_index,
                "delta": delta
            }))
        }

        fn tool_args_done(self, output_index: u32, arguments: &str) -> Self {
            self.push(json!({
                "type": "response.function_call_arguments.done",
                "output_index": output_index,
                "arguments": arguments
            }))
        }

        fn tool_done(self, output_index: u32, call_id: &str, name: &str, arguments: &str) -> Self {
            self.push(json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": function_call_item(call_id, name, arguments, "completed")
            }))
        }

        fn error(self, code: Option<&str>, message: &str) -> Self {
            self.push(json!({ "type": "error", "code": code, "message": message }))
        }

        fn failed(self, code: Option<&str>, message: &str) -> Self {
            self.push(json!({
                "type": "response.failed",
                "response": { "error": { "code": code, "message": message } }
            }))
        }

        fn completed(self) -> Self {
            self.push(json!({ "type": "response.completed", "response": { "status": "completed" } }))
        }

        fn completed_with_usage(self, usage: &Value) -> Self {
            self.push(json!({ "type": "response.completed", "response": { "status": "completed", "usage": usage } }))
        }

        fn incomplete(self) -> Self {
            self.push(json!({ "type": "response.incomplete", "response": { "status": "incomplete" } }))
        }

        fn push(mut self, event: Value) -> Self {
            self.events.push(event);
            self
        }

        fn build(self) -> Vec<Value> {
            self.events
        }
    }

    fn function_call_item(call_id: &str, name: &str, arguments: &str, status: &str) -> Value {
        json!({
            "type": "function_call",
            "id": format!("fc_{call_id}"),
            "call_id": call_id,
            "name": name,
            "arguments": arguments,
            "status": status
        })
    }

    fn usage_json(input_tokens: u32, output_tokens: u32, cached_tokens: u32, reasoning_tokens: u32) -> Value {
        json!({
            "input_tokens": input_tokens,
            "input_tokens_details": { "cached_tokens": cached_tokens },
            "output_tokens": output_tokens,
            "output_tokens_details": { "reasoning_tokens": reasoning_tokens },
            "total_tokens": input_tokens + output_tokens
        })
    }
}
