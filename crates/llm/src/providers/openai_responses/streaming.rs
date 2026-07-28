use async_openai::types::responses::{OutputItem, ResponseUsage, Status};
use futures::Stream;
use serde::{Deserialize, Deserializer, de::Error as _};
use tokio_stream::StreamExt;

use crate::providers::tool_call_collector::ToolCallCollector;
use crate::{LlmError, LlmResponse, Result, StopReason, TokenUsage};

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
            input_tokens: usage.usage.input_tokens,
            output_tokens: usage.usage.output_tokens,
            cache_read_tokens: Some(usage.usage.input_tokens_details.cached_tokens),
            cache_creation_tokens: usage.cache_write_tokens,
            cache_reporting_exclusive: Some(false),
            reasoning_tokens: Some(usage.usage.output_tokens_details.reasoning_tokens),
            ..TokenUsage::default()
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ResponsesStreamEvent {
    #[serde(rename = "response.created")]
    Created(ResponsesCreatedEvent),
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(ResponsesTextDeltaEvent),
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded(ResponsesOutputItemEvent),
    #[serde(rename = "response.output_item.done")]
    OutputItemDone(ResponsesOutputItemEvent),
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta(ResponsesFunctionCallArgumentsDeltaEvent),
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone(ResponsesFunctionCallArgumentsDoneEvent),
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta(ResponsesTextDeltaEvent),
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
pub struct ResponsesCreatedEvent {
    pub response: ResponsesCreated,
}

#[derive(Debug, Deserialize)]
pub struct ResponsesCreated {
    pub id: String,
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
pub struct ResponsesFunctionCallArgumentsDoneEvent {
    pub output_index: u32,
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
    pub message: String,
}

/// Process an `OpenAI` Responses event stream into `LlmResponse` items.
pub fn process_response_stream<T>(stream: T) -> impl Stream<Item = Result<LlmResponse>> + Send
where
    T: Stream<Item = Result<ResponsesStreamEvent>> + Send + Unpin,
{
    async_stream::stream! {
        let mut tool_collector = ToolCallCollector::<u32>::new();
        let mut stream = Box::pin(stream);
        let mut last_stop_reason: Option<StopReason> = None;
        let mut started = false;
        let mut terminal = false;
        let mut failed = false;

        while let Some(result) = stream.next().await {
            let event = match result {
                Ok(event) => event,
                Err(e) => {
                    yield Err(LlmError::StreamInterrupted(e.to_string()));
                    failed = true;
                    break;
                }
            };

            if matches!(event, ResponsesStreamEvent::Created(_)) {
                started = true;
            } else if !started && !event.may_precede_creation() {
                yield Err(LlmError::StreamInterrupted(
                    "Responses stream emitted data before response.created".to_string(),
                ));
                failed = true;
                break;
            }

            terminal = matches!(event, ResponsesStreamEvent::Completed(_) | ResponsesStreamEvent::Incomplete(_));
            let responses = process_event(event, &mut tool_collector, &mut last_stop_reason);
            let event_failed = responses.iter().any(Result::is_err);
            for response in responses {
                yield response;
            }
            if event_failed || terminal {
                failed = event_failed;
                break;
            }
        }

        if !failed {
            for tc in tool_collector.complete_all() {
                yield Ok(LlmResponse::ToolRequestComplete { tool_call: tc });
            }

            if terminal {
                yield Ok(LlmResponse::Done { stop_reason: last_stop_reason });
            } else {
                yield Err(LlmError::StreamInterrupted(
                    "Responses stream ended before a terminal response event".to_string(),
                ));
            }
        }
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

fn process_event(
    event: ResponsesStreamEvent,
    tool_collector: &mut ToolCallCollector<u32>,
    last_stop_reason: &mut Option<StopReason>,
) -> Vec<Result<LlmResponse>> {
    let mut responses = Vec::new();
    let incomplete = matches!(&event, ResponsesStreamEvent::Incomplete(_));

    match event {
        ResponsesStreamEvent::Created(e) => {
            responses.push(Ok(LlmResponse::Start { message_id: e.response.id }));
        }
        ResponsesStreamEvent::OutputTextDelta(e) if !e.delta.is_empty() => {
            responses.push(Ok(LlmResponse::Text { chunk: e.delta }));
        }
        ResponsesStreamEvent::OutputItemAdded(e) => {
            if let OutputItem::FunctionCall(call) = e.item {
                let tool_responses = tool_collector.handle_delta(e.output_index, call.id, Some(call.name), None);
                responses.extend(tool_responses.into_iter().map(Ok));
            }
        }
        ResponsesStreamEvent::FunctionCallArgumentsDelta(e) => {
            let tool_responses = tool_collector.handle_delta(e.output_index, None, None, Some(e.delta));
            responses.extend(tool_responses.into_iter().map(Ok));
        }
        ResponsesStreamEvent::FunctionCallArgumentsDone(e) => {
            if let Some(tc) = tool_collector.complete_one(e.output_index) {
                responses.push(Ok(LlmResponse::ToolRequestComplete { tool_call: tc }));
            }
        }
        ResponsesStreamEvent::ReasoningSummaryTextDelta(e) if !e.delta.is_empty() => {
            responses.push(Ok(LlmResponse::Reasoning { chunk: e.delta }));
        }
        ResponsesStreamEvent::OutputItemDone(e) => {
            if let OutputItem::Reasoning(reasoning) = e.item
                && let Some(id) = reasoning.id
                && let Some(encrypted) = reasoning.encrypted_content
            {
                responses.push(Ok(LlmResponse::EncryptedReasoning { id, content: encrypted }));
            }
        }
        ResponsesStreamEvent::Completed(e) | ResponsesStreamEvent::Incomplete(e) => {
            if let Some(usage) = e.response.usage {
                responses.push(Ok(LlmResponse::Usage { tokens: usage.into() }));
            }
            match e.response.status {
                Some(Status::Completed) => *last_stop_reason = Some(StopReason::EndTurn),
                Some(Status::Incomplete) => *last_stop_reason = Some(StopReason::Length),
                _ if incomplete => {
                    *last_stop_reason = Some(StopReason::Length);
                }
                _ => {}
            }
        }
        ResponsesStreamEvent::Failed(e) => {
            let message = e.response.error.map_or_else(|| "Unknown Responses API failure".to_string(), |e| e.message);
            responses.push(Err(LlmError::ApiError(message)));
        }
        ResponsesStreamEvent::Error(e) => {
            responses.push(Err(LlmError::ServerError {
                status: None,
                message: format!("Responses API error: {}", e.message),
            }));
        }
        ResponsesStreamEvent::Ignored
        | ResponsesStreamEvent::OutputTextDelta(_)
        | ResponsesStreamEvent::ReasoningSummaryTextDelta(_) => {}
    }

    responses
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsage;
    use async_openai::types::responses::{FunctionToolCall, ReasoningItem};
    use serde_json::json;

    async fn collect_responses(events: Vec<ResponsesStreamEvent>) -> Vec<LlmResponse> {
        let stream = make_stream(events);
        let mut response_stream = Box::pin(process_response_stream(stream));
        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }
        responses
    }

    #[tokio::test]
    async fn test_text_stream() {
        let responses = collect_responses(vec![
            text_delta("Hello"),
            text_delta(" world"),
            completed(Status::Completed, Some(make_usage(10, 5))),
        ])
        .await;

        assert!(matches!(responses[0], LlmResponse::Start { .. }));
        assert!(matches!(responses[1], LlmResponse::Text { ref chunk } if chunk == "Hello"));
        assert!(matches!(responses[2], LlmResponse::Text { ref chunk } if chunk == " world"));
        assert!(matches!(
            responses[3],
            LlmResponse::Usage { tokens: TokenUsage { input_tokens: 10, output_tokens: 5, .. } }
        ));
        assert!(matches!(responses[4], LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }));
    }

    #[tokio::test]
    async fn test_tool_call_stream() {
        let responses = collect_responses(vec![
            ResponsesStreamEvent::OutputItemAdded(ResponsesOutputItemEvent {
                output_index: 0,
                item: OutputItem::FunctionCall(FunctionToolCall {
                    id: Some("fc_1".to_string()),
                    call_id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: String::new(),
                    status: None,
                    namespace: None,
                }),
            }),
            function_call_delta(r#"{"path":"#),
            function_call_delta(r#""foo.rs"}"#),
            ResponsesStreamEvent::FunctionCallArgumentsDone(ResponsesFunctionCallArgumentsDoneEvent {
                output_index: 0,
            }),
            completed(Status::Completed, Some(make_usage(20, 10))),
        ])
        .await;

        assert!(matches!(responses[0], LlmResponse::Start { .. }));
        assert!(
            matches!(&responses[1], LlmResponse::ToolRequestStart { id, name } if id == "fc_1" && name == "read_file")
        );
        assert!(matches!(responses[2], LlmResponse::ToolRequestArg { .. }));
        assert!(matches!(responses[3], LlmResponse::ToolRequestArg { .. }));

        let tc = responses.iter().find(|r| matches!(r, LlmResponse::ToolRequestComplete { .. }));
        assert!(tc.is_some());
        if let LlmResponse::ToolRequestComplete { tool_call } = tc.unwrap() {
            assert_eq!(tool_call.id, "fc_1");
            assert_eq!(tool_call.name, "read_file");
            assert_eq!(tool_call.arguments, r#"{"path":"foo.rs"}"#);
        }
    }

    #[tokio::test]
    async fn test_error_event_is_retryable_server_error() {
        let stream = make_stream(vec![ResponsesStreamEvent::Error(ResponsesErrorEvent {
            message: "Rate limit exceeded".to_string(),
        })]);
        let mut response_stream = Box::pin(process_response_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result);
        }

        assert!(responses[0].is_ok());
        let err = responses[1].as_ref().expect_err("expected error event to surface as Err");
        assert!(matches!(err, LlmError::ServerError { status: None, .. }), "got {err:?}");
        assert!(err.is_retryable(), "ResponseError must be retryable so the agent can recover");
    }

    #[tokio::test]
    async fn test_reasoning_delta() {
        let responses = collect_responses(vec![
            reasoning_delta("Thinking about"),
            reasoning_delta(" the problem"),
            completed(Status::Completed, None),
        ])
        .await;

        assert!(matches!(responses[1], LlmResponse::Reasoning { ref chunk } if chunk == "Thinking about"));
        assert!(matches!(responses[2], LlmResponse::Reasoning { ref chunk } if chunk == " the problem"));
    }

    #[tokio::test]
    async fn test_incomplete_status_gives_length_stop_reason() {
        let responses = collect_responses(vec![completed(Status::Incomplete, None)]).await;

        assert!(matches!(responses.last().unwrap(), LlmResponse::Done { stop_reason: Some(StopReason::Length) }));
    }

    #[tokio::test]
    async fn test_stream_error_propagation_is_retryable() {
        let events: Vec<Result<ResponsesStreamEvent>> =
            vec![Err(LlmError::StreamInterrupted("connection lost".to_string()))];

        let stream = tokio_stream::iter(events);
        let mut response_stream = Box::pin(process_response_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result);
        }

        let err = responses[0].as_ref().expect_err("expected upstream Err to surface as Err");
        assert!(matches!(err, LlmError::StreamInterrupted(_)), "got {err:?}");
        assert_eq!(responses.len(), 1);
        assert!(err.is_retryable(), "mid-stream interrupts must be retryable");
    }

    #[tokio::test]
    async fn error_event_before_creation_keeps_the_servers_message() {
        let events =
            vec![Ok(ResponsesStreamEvent::Error(ResponsesErrorEvent { message: "Rate limit exceeded".to_string() }))];
        let responses = process_response_stream(tokio_stream::iter(events)).collect::<Vec<_>>().await;

        let err = responses[0].as_ref().expect_err("expected the error event to surface as Err");
        assert!(matches!(err, LlmError::ServerError { .. }), "got {err:?}");
        assert!(err.to_string().contains("Rate limit exceeded"), "server message was dropped: {err}");
    }

    #[tokio::test]
    async fn failure_event_before_creation_keeps_the_servers_message() {
        let events = vec![Ok(ResponsesStreamEvent::Failed(ResponsesFailedEvent {
            response: ResponsesFailed { error: Some(ResponsesErrorEvent { message: "model overloaded".to_string() }) },
        }))];
        let responses = process_response_stream(tokio_stream::iter(events)).collect::<Vec<_>>().await;

        let err = responses[0].as_ref().expect_err("expected the failure event to surface as Err");
        assert!(matches!(err, LlmError::ApiError(_)), "got {err:?}");
        assert!(err.to_string().contains("model overloaded"), "server message was dropped: {err}");
    }

    #[tokio::test]
    async fn data_before_creation_is_interrupted() {
        let events = vec![Ok(text_delta("leaked"))];
        let responses = process_response_stream(tokio_stream::iter(events)).collect::<Vec<_>>().await;

        assert!(matches!(responses[0], Err(LlmError::StreamInterrupted(_))), "{responses:?}");
    }

    #[tokio::test]
    async fn stream_without_terminal_event_is_interrupted() {
        let stream = make_stream(vec![text_delta("partial")]);
        let responses = process_response_stream(stream).collect::<Vec<_>>().await;

        assert!(matches!(responses[0], Ok(LlmResponse::Start { .. })));
        assert!(matches!(responses[1], Ok(LlmResponse::Text { .. })));
        assert!(matches!(responses[2], Err(LlmError::StreamInterrupted(_))));
        assert!(!responses.iter().any(|response| matches!(response, Ok(LlmResponse::Done { .. }))));
    }

    #[tokio::test]
    async fn captured_responses_fixture_uses_the_shared_processor() {
        let responses = process_fixture(include_str!("../../../tests/fixtures/openai_responses/01_minimal.sse")).await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let usage = fixture_usage(&responses).expect("fixture should report usage");
        assert!(usage.input_tokens > 0, "input_tokens should be > 0: {usage:?}");
        assert!(usage.output_tokens > 0, "output_tokens should be > 0: {usage:?}");
        assert!(matches!(responses.last(), Some(Ok(LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }))));
    }

    #[tokio::test]
    async fn captured_reasoning_fixture_preserves_reasoning_usage() {
        let responses =
            process_fixture(include_str!("../../../tests/fixtures/openai_responses/02_reasoning.sse")).await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let usage = fixture_usage(&responses).expect("fixture should report usage");
        assert!(usage.input_tokens > 0, "input_tokens should be > 0: {usage:?}");
        assert!(usage.output_tokens > 0, "output_tokens should be > 0: {usage:?}");
        assert!(usage.reasoning_tokens.is_some_and(|tokens| tokens > 0), "{usage:?}");
    }

    #[tokio::test]
    async fn captured_mantle_fixture_preserves_cache_write_usage() {
        let responses =
            process_fixture(include_str!("../../../tests/fixtures/openai_responses/03_mantle_cache_write.sse")).await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let usage = fixture_usage(&responses).expect("fixture should report usage");
        assert_eq!(usage.cache_creation_tokens, Some(1024));
    }

    /// Decode a captured SSE body and run it through the shared processor.
    async fn process_fixture(sse: &str) -> Vec<Result<LlmResponse>> {
        let events = sse
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|data| *data != "[DONE]")
            .map(|data| serde_json::from_str::<ResponsesStreamEvent>(data).map_err(LlmError::from));
        process_response_stream(tokio_stream::iter(events)).collect::<Vec<_>>().await
    }

    fn fixture_usage(responses: &[Result<LlmResponse>]) -> Option<TokenUsage> {
        responses.iter().find_map(|response| match response {
            Ok(LlmResponse::Usage { tokens }) => Some(*tokens),
            _ => None,
        })
    }

    #[test]
    fn test_encrypted_reasoning_from_output_item_done() {
        let event = ResponsesStreamEvent::OutputItemDone(ResponsesOutputItemEvent {
            output_index: 0,
            item: reasoning_item(Some("enc-blob-data")),
        });

        let mut tool_collector = ToolCallCollector::<u32>::new();
        let mut stop_reason = None;
        let responses = process_event(event, &mut tool_collector, &mut stop_reason);

        assert_eq!(responses.len(), 1);
        assert!(
            matches!(&responses[0], Ok(LlmResponse::EncryptedReasoning { content, .. }) if content == "enc-blob-data")
        );
    }

    #[tokio::test]
    async fn test_usage_forwards_reasoning_and_cache_read() {
        let responses =
            collect_responses(vec![completed(Status::Completed, Some(make_usage_full(120, 80, 50, 30)))]).await;

        let usage = responses.iter().find_map(|r| match r {
            LlmResponse::Usage { tokens } => Some(*tokens),
            _ => None,
        });

        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 120,
                output_tokens: 80,
                cache_read_tokens: Some(50),
                cache_reporting_exclusive: Some(false),
                reasoning_tokens: Some(30),
                ..TokenUsage::default()
            })
        );
    }

    #[tokio::test]
    async fn test_completed_without_output_deserializes_usage_and_stop_reason() {
        let event: ResponsesStreamEvent = serde_json::from_value(json!({
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
                "usage": make_usage_json(100, 20, 0, 10)
            }
        }))
        .unwrap();
        let responses = collect_responses(vec![event]).await;

        assert!(matches!(
            responses.iter().find(|response| matches!(response, LlmResponse::Usage { .. })),
            Some(LlmResponse::Usage {
                tokens: TokenUsage { input_tokens: 100, output_tokens: 20, reasoning_tokens: Some(10), .. }
            })
        ));
        assert!(matches!(responses.last().unwrap(), LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }));
    }

    #[test]
    fn test_output_item_done_without_encrypted_content_is_ignored() {
        let event = ResponsesStreamEvent::OutputItemDone(ResponsesOutputItemEvent {
            output_index: 0,
            item: reasoning_item(None),
        });

        let mut tool_collector = ToolCallCollector::<u32>::new();
        let mut stop_reason = None;
        let responses = process_event(event, &mut tool_collector, &mut stop_reason);

        assert!(responses.is_empty());
    }

    fn text_delta(delta: &str) -> ResponsesStreamEvent {
        ResponsesStreamEvent::OutputTextDelta(ResponsesTextDeltaEvent { delta: delta.to_string() })
    }

    fn reasoning_delta(delta: &str) -> ResponsesStreamEvent {
        ResponsesStreamEvent::ReasoningSummaryTextDelta(ResponsesTextDeltaEvent { delta: delta.to_string() })
    }

    fn function_call_delta(delta: &str) -> ResponsesStreamEvent {
        ResponsesStreamEvent::FunctionCallArgumentsDelta(ResponsesFunctionCallArgumentsDeltaEvent {
            output_index: 0,
            delta: delta.to_string(),
        })
    }

    fn completed(status: Status, usage: Option<ResponsesUsage>) -> ResponsesStreamEvent {
        ResponsesStreamEvent::Completed(ResponsesCompletedEvent {
            response: ResponsesCompleted { usage, status: Some(status) },
        })
    }

    fn reasoning_item(encrypted_content: Option<&str>) -> OutputItem {
        OutputItem::Reasoning(ReasoningItem {
            id: Some("r_1".to_string()),
            summary: vec![],
            encrypted_content: encrypted_content.map(ToString::to_string),
            content: None,
            status: None,
        })
    }

    fn make_stream(
        events: Vec<ResponsesStreamEvent>,
    ) -> impl Stream<Item = Result<ResponsesStreamEvent>> + Send + Unpin {
        tokio_stream::iter(
            std::iter::once(Ok(ResponsesStreamEvent::Created(ResponsesCreatedEvent {
                response: ResponsesCreated { id: "resp_test".to_string() },
            })))
            .chain(events.into_iter().map(Ok))
            .collect::<Vec<_>>(),
        )
    }

    fn make_usage(input_tokens: u32, output_tokens: u32) -> ResponsesUsage {
        make_usage_full(input_tokens, output_tokens, 0, 0)
    }

    fn make_usage_full(
        input_tokens: u32,
        output_tokens: u32,
        cached_tokens: u32,
        reasoning_tokens: u32,
    ) -> ResponsesUsage {
        serde_json::from_value(make_usage_json(input_tokens, output_tokens, cached_tokens, reasoning_tokens)).unwrap()
    }

    fn make_usage_json(
        input_tokens: u32,
        output_tokens: u32,
        cached_tokens: u32,
        reasoning_tokens: u32,
    ) -> serde_json::Value {
        json!({
            "input_tokens": input_tokens,
            "input_tokens_details": { "cached_tokens": cached_tokens },
            "output_tokens": output_tokens,
            "output_tokens_details": { "reasoning_tokens": reasoning_tokens },
            "total_tokens": input_tokens + output_tokens
        })
    }
}
