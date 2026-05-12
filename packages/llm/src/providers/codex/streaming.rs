use async_openai::types::responses::{OutputItem, ResponseStreamEvent, Status};

use crate::providers::tool_call_collector::ToolCallCollector;
use crate::{LlmError, LlmResponse, Result, StopReason};
use futures::Stream;
use tokio_stream::StreamExt;

/// Process a typed `ResponseStreamEvent` stream into `LlmResponse` items.
pub fn process_response_stream<T>(stream: T) -> impl Stream<Item = Result<LlmResponse>> + Send
where
    T: Stream<Item = Result<ResponseStreamEvent>> + Send + Unpin,
{
    async_stream::stream! {
        let message_id = uuid::Uuid::new_v4().to_string();
        yield Ok(LlmResponse::Start { message_id });

        let mut tool_collector = ToolCallCollector::<u32>::new();
        let mut stream = Box::pin(stream);
        let mut last_stop_reason: Option<StopReason> = None;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    for response in process_event(event, &mut tool_collector, &mut last_stop_reason) {
                        yield response;
                    }
                }
                Err(e) => {
                    yield Err(LlmError::StreamInterrupted(e.to_string()));
                    break;
                }
            }
        }

        // Complete any pending tool calls
        for tc in tool_collector.complete_all() {
            yield Ok(LlmResponse::ToolRequestComplete { tool_call: tc });
        }

        yield Ok(LlmResponse::Done {
            stop_reason: last_stop_reason,
        });
    }
}

fn process_event(
    event: ResponseStreamEvent,
    tool_collector: &mut ToolCallCollector<u32>,
    last_stop_reason: &mut Option<StopReason>,
) -> Vec<Result<LlmResponse>> {
    let mut responses = Vec::new();

    match event {
        ResponseStreamEvent::ResponseOutputTextDelta(e) if !e.delta.is_empty() => {
            responses.push(Ok(LlmResponse::Text { chunk: e.delta }));
        }
        ResponseStreamEvent::ResponseOutputItemAdded(e) => {
            if let OutputItem::FunctionCall(call) = e.item {
                let tool_responses = tool_collector.handle_delta(e.output_index, call.id, Some(call.name), None);
                responses.extend(tool_responses.into_iter().map(Ok));
            }
        }
        ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(e) => {
            let tool_responses = tool_collector.handle_delta(e.output_index, None, None, Some(e.delta));
            responses.extend(tool_responses.into_iter().map(Ok));
        }
        ResponseStreamEvent::ResponseFunctionCallArgumentsDone(e) => {
            if let Some(tc) = tool_collector.complete_one(e.output_index) {
                responses.push(Ok(LlmResponse::ToolRequestComplete { tool_call: tc }));
            }
        }
        ResponseStreamEvent::ResponseReasoningSummaryTextDelta(e) if !e.delta.is_empty() => {
            responses.push(Ok(LlmResponse::Reasoning { chunk: e.delta }));
        }
        ResponseStreamEvent::ResponseOutputItemDone(e) => {
            if let OutputItem::Reasoning(reasoning) = e.item
                && let Some(id) = reasoning.id
                && let Some(encrypted) = reasoning.encrypted_content
            {
                responses.push(Ok(LlmResponse::EncryptedReasoning { id, content: encrypted }));
            }
        }
        ResponseStreamEvent::ResponseCompleted(e) => {
            if let Some(usage) = e.response.usage {
                responses.push(Ok(LlmResponse::Usage { tokens: usage.into() }));
            }
            match e.response.status {
                Status::Completed => *last_stop_reason = Some(StopReason::EndTurn),
                Status::Incomplete => *last_stop_reason = Some(StopReason::Length),
                _ => {}
            }
        }
        ResponseStreamEvent::ResponseError(e) => {
            responses
                .push(Err(LlmError::ServerError { status: None, message: format!("Codex API error: {}", e.message) }));
        }
        // Events we don't need to act on
        _ => {}
    }

    responses
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsage;
    use async_openai::types::responses::{
        FunctionToolCall, ReasoningItem, Response, ResponseCompletedEvent, ResponseErrorEvent,
        ResponseFunctionCallArgumentsDeltaEvent, ResponseFunctionCallArgumentsDoneEvent, ResponseOutputItemAddedEvent,
        ResponseOutputItemDoneEvent, ResponseReasoningSummaryTextDeltaEvent, ResponseTextDeltaEvent, ResponseUsage,
    };
    /// Build a minimal `Response` with given status and optional usage via JSON deserialization.
    fn make_response(status: &Status, usage: Option<ResponseUsage>) -> Response {
        let status_str = serde_json::to_value(status).unwrap();
        let mut json = serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "status": status_str,
            "output": [],
            "model": "test",
            "created_at": 0
        });
        if let Some(u) = usage {
            json["usage"] = serde_json::to_value(u).unwrap();
        }
        serde_json::from_value(json).unwrap()
    }

    fn make_usage(input_tokens: u32, output_tokens: u32) -> ResponseUsage {
        make_usage_full(input_tokens, output_tokens, 0, 0)
    }

    fn make_usage_full(
        input_tokens: u32,
        output_tokens: u32,
        cached_tokens: u32,
        reasoning_tokens: u32,
    ) -> ResponseUsage {
        serde_json::from_value(serde_json::json!({
            "input_tokens": input_tokens,
            "input_tokens_details": { "cached_tokens": cached_tokens },
            "output_tokens": output_tokens,
            "output_tokens_details": { "reasoning_tokens": reasoning_tokens },
            "total_tokens": input_tokens + output_tokens
        }))
        .unwrap()
    }

    fn make_stream(events: Vec<ResponseStreamEvent>) -> impl Stream<Item = Result<ResponseStreamEvent>> + Send + Unpin {
        tokio_stream::iter(events.into_iter().map(Ok).collect::<Vec<_>>())
    }

    #[tokio::test]
    async fn test_text_stream() {
        let events = vec![
            ResponseStreamEvent::ResponseOutputTextDelta(ResponseTextDeltaEvent {
                output_index: 0,
                content_index: 0,
                delta: "Hello".to_string(),
                sequence_number: 1,
                item_id: "msg_1".to_string(),
                logprobs: None,
            }),
            ResponseStreamEvent::ResponseOutputTextDelta(ResponseTextDeltaEvent {
                output_index: 0,
                content_index: 0,
                delta: " world".to_string(),
                sequence_number: 2,
                item_id: "msg_1".to_string(),
                logprobs: None,
            }),
            ResponseStreamEvent::ResponseCompleted(ResponseCompletedEvent {
                sequence_number: 3,
                response: make_response(&Status::Completed, Some(make_usage(10, 5))),
            }),
        ];

        let stream = make_stream(events);
        let mut response_stream = Box::pin(process_response_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

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
        let events = vec![
            ResponseStreamEvent::ResponseOutputItemAdded(ResponseOutputItemAddedEvent {
                sequence_number: 1,
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
            ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(ResponseFunctionCallArgumentsDeltaEvent {
                sequence_number: 2,
                item_id: "fc_1".to_string(),
                output_index: 0,
                delta: r#"{"path":"#.to_string(),
            }),
            ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(ResponseFunctionCallArgumentsDeltaEvent {
                sequence_number: 3,
                item_id: "fc_1".to_string(),
                output_index: 0,
                delta: r#""foo.rs"}"#.to_string(),
            }),
            ResponseStreamEvent::ResponseFunctionCallArgumentsDone(ResponseFunctionCallArgumentsDoneEvent {
                sequence_number: 4,
                item_id: "fc_1".to_string(),
                output_index: 0,
                arguments: r#"{"path":"foo.rs"}"#.to_string(),
                name: None,
            }),
            ResponseStreamEvent::ResponseCompleted(ResponseCompletedEvent {
                sequence_number: 5,
                response: make_response(&Status::Completed, Some(make_usage(20, 10))),
            }),
        ];

        let stream = make_stream(events);
        let mut response_stream = Box::pin(process_response_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

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
        let events = vec![ResponseStreamEvent::ResponseError(ResponseErrorEvent {
            sequence_number: 1,
            code: None,
            message: "Rate limit exceeded".to_string(),
            param: None,
        })];

        let stream = make_stream(events);
        let mut response_stream = Box::pin(process_response_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result);
        }

        assert!(responses[0].is_ok());
        let err = responses[1].as_ref().expect_err("expected ResponseError to surface as Err");
        assert!(matches!(err, LlmError::ServerError { status: None, .. }), "got {err:?}");
        assert!(err.is_retryable(), "ResponseError must be retryable so the agent can recover");
    }

    #[tokio::test]
    async fn test_reasoning_delta() {
        let events = vec![
            ResponseStreamEvent::ResponseReasoningSummaryTextDelta(ResponseReasoningSummaryTextDeltaEvent {
                sequence_number: 1,
                item_id: "r_1".to_string(),
                output_index: 0,
                summary_index: 0,
                delta: "Thinking about".to_string(),
            }),
            ResponseStreamEvent::ResponseReasoningSummaryTextDelta(ResponseReasoningSummaryTextDeltaEvent {
                sequence_number: 2,
                item_id: "r_1".to_string(),
                output_index: 0,
                summary_index: 0,
                delta: " the problem".to_string(),
            }),
            ResponseStreamEvent::ResponseCompleted(ResponseCompletedEvent {
                sequence_number: 3,
                response: make_response(&Status::Completed, None),
            }),
        ];

        let stream = make_stream(events);
        let mut response_stream = Box::pin(process_response_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

        assert!(matches!(responses[1], LlmResponse::Reasoning { ref chunk } if chunk == "Thinking about"));
        assert!(matches!(responses[2], LlmResponse::Reasoning { ref chunk } if chunk == " the problem"));
    }

    #[tokio::test]
    async fn test_incomplete_status_gives_length_stop_reason() {
        let events = vec![ResponseStreamEvent::ResponseCompleted(ResponseCompletedEvent {
            sequence_number: 1,
            response: make_response(&Status::Incomplete, None),
        })];

        let stream = make_stream(events);
        let mut response_stream = Box::pin(process_response_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result.unwrap());
        }

        assert!(matches!(responses.last().unwrap(), LlmResponse::Done { stop_reason: Some(StopReason::Length) }));
    }

    #[tokio::test]
    async fn test_stream_error_propagation_is_retryable() {
        let events: Vec<Result<ResponseStreamEvent>> =
            vec![Err(LlmError::StreamInterrupted("connection lost".to_string()))];

        let stream = tokio_stream::iter(events);
        let mut response_stream = Box::pin(process_response_stream(stream));

        let mut responses = Vec::new();
        while let Some(result) = response_stream.next().await {
            responses.push(result);
        }

        assert!(responses[0].is_ok());
        let err = responses[1].as_ref().expect_err("expected upstream Err to surface as Err");
        assert!(matches!(err, LlmError::StreamInterrupted(_)), "got {err:?}");
        assert!(err.is_retryable(), "mid-stream interrupts must be retryable");
    }

    #[test]
    fn test_encrypted_reasoning_from_output_item_done() {
        let event = ResponseStreamEvent::ResponseOutputItemDone(ResponseOutputItemDoneEvent {
            sequence_number: 1,
            output_index: 0,
            item: OutputItem::Reasoning(ReasoningItem {
                id: Some("r_1".to_string()),
                summary: vec![],
                encrypted_content: Some("enc-blob-data".to_string()),
                content: None,
                status: None,
            }),
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
        let events = vec![ResponseStreamEvent::ResponseCompleted(ResponseCompletedEvent {
            sequence_number: 1,
            response: make_response(&Status::Completed, Some(make_usage_full(120, 80, 50, 30))),
        })];

        let stream = make_stream(events);
        let mut response_stream = Box::pin(process_response_stream(stream));

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
                input_tokens: 120,
                output_tokens: 80,
                cache_read_tokens: Some(50),
                reasoning_tokens: Some(30),
                ..TokenUsage::default()
            })
        );
    }

    #[test]
    fn test_output_item_done_without_encrypted_content_is_ignored() {
        let event = ResponseStreamEvent::ResponseOutputItemDone(ResponseOutputItemDoneEvent {
            sequence_number: 1,
            output_index: 0,
            item: OutputItem::Reasoning(ReasoningItem {
                id: Some("r_2".to_string()),
                summary: vec![],
                encrypted_content: None,
                content: None,
                status: None,
            }),
        });

        let mut tool_collector = ToolCallCollector::<u32>::new();
        let mut stop_reason = None;
        let responses = process_event(event, &mut tool_collector, &mut stop_reason);

        assert!(responses.is_empty());
    }
}
