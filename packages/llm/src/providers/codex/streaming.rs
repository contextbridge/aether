use async_openai::types::responses::{OutputItem, ResponseUsage, Status};
use futures::Stream;
use serde::Deserialize;
use tokio_stream::StreamExt;

use crate::providers::tool_call_collector::ToolCallCollector;
use crate::{LlmError, LlmResponse, Result, StopReason};

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum CodexStreamEvent {
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(CodexTextDeltaEvent),
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded(CodexOutputItemEvent),
    #[serde(rename = "response.output_item.done")]
    OutputItemDone(CodexOutputItemEvent),
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta(CodexFunctionCallArgumentsDeltaEvent),
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone(CodexFunctionCallArgumentsDoneEvent),
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta(CodexTextDeltaEvent),
    #[serde(rename = "response.completed")]
    Completed(CodexResponseCompletedEvent),
    #[serde(rename = "error")]
    Error(CodexErrorEvent),
    #[serde(other)]
    Ignored,
}

#[derive(Debug, Deserialize)]
pub struct CodexTextDeltaEvent {
    pub delta: String,
}

#[derive(Debug, Deserialize)]
pub struct CodexOutputItemEvent {
    pub output_index: u32,
    pub item: OutputItem,
}

#[derive(Debug, Deserialize)]
pub struct CodexFunctionCallArgumentsDeltaEvent {
    pub output_index: u32,
    pub delta: String,
}

#[derive(Debug, Deserialize)]
pub struct CodexFunctionCallArgumentsDoneEvent {
    pub output_index: u32,
}

#[derive(Debug, Deserialize)]
pub struct CodexResponseCompletedEvent {
    pub response: CodexResponseCompleted,
}

#[derive(Debug, Deserialize)]
pub struct CodexResponseCompleted {
    #[serde(default)]
    pub usage: Option<ResponseUsage>,
    #[serde(default)]
    pub status: Option<Status>,
}

#[derive(Debug, Deserialize)]
pub struct CodexErrorEvent {
    pub message: String,
}

/// Process a Codex response stream into `LlmResponse` items.
pub fn process_response_stream<T>(stream: T) -> impl Stream<Item = Result<LlmResponse>> + Send
where
    T: Stream<Item = Result<CodexStreamEvent>> + Send + Unpin,
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

        for tc in tool_collector.complete_all() {
            yield Ok(LlmResponse::ToolRequestComplete { tool_call: tc });
        }

        yield Ok(LlmResponse::Done {
            stop_reason: last_stop_reason,
        });
    }
}

fn process_event(
    event: CodexStreamEvent,
    tool_collector: &mut ToolCallCollector<u32>,
    last_stop_reason: &mut Option<StopReason>,
) -> Vec<Result<LlmResponse>> {
    let mut responses = Vec::new();

    match event {
        CodexStreamEvent::OutputTextDelta(e) if !e.delta.is_empty() => {
            responses.push(Ok(LlmResponse::Text { chunk: e.delta }));
        }
        CodexStreamEvent::OutputItemAdded(e) => {
            if let OutputItem::FunctionCall(call) = e.item {
                let tool_responses = tool_collector.handle_delta(e.output_index, call.id, Some(call.name), None);
                responses.extend(tool_responses.into_iter().map(Ok));
            }
        }
        CodexStreamEvent::FunctionCallArgumentsDelta(e) => {
            let tool_responses = tool_collector.handle_delta(e.output_index, None, None, Some(e.delta));
            responses.extend(tool_responses.into_iter().map(Ok));
        }
        CodexStreamEvent::FunctionCallArgumentsDone(e) => {
            if let Some(tc) = tool_collector.complete_one(e.output_index) {
                responses.push(Ok(LlmResponse::ToolRequestComplete { tool_call: tc }));
            }
        }
        CodexStreamEvent::ReasoningSummaryTextDelta(e) if !e.delta.is_empty() => {
            responses.push(Ok(LlmResponse::Reasoning { chunk: e.delta }));
        }
        CodexStreamEvent::OutputItemDone(e) => {
            if let OutputItem::Reasoning(reasoning) = e.item
                && let Some(id) = reasoning.id
                && let Some(encrypted) = reasoning.encrypted_content
            {
                responses.push(Ok(LlmResponse::EncryptedReasoning { id, content: encrypted }));
            }
        }
        CodexStreamEvent::Completed(e) => {
            if let Some(usage) = e.response.usage {
                responses.push(Ok(LlmResponse::Usage { tokens: usage.into() }));
            }
            match e.response.status {
                Some(Status::Completed) => *last_stop_reason = Some(StopReason::EndTurn),
                Some(Status::Incomplete) => *last_stop_reason = Some(StopReason::Length),
                _ => {}
            }
        }
        CodexStreamEvent::Error(e) => {
            responses
                .push(Err(LlmError::ServerError { status: None, message: format!("Codex API error: {}", e.message) }));
        }
        CodexStreamEvent::Ignored
        | CodexStreamEvent::OutputTextDelta(_)
        | CodexStreamEvent::ReasoningSummaryTextDelta(_) => {}
    }

    responses
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsage;
    use async_openai::types::responses::{FunctionToolCall, ReasoningItem};
    use serde_json::json;

    async fn collect_responses(events: Vec<CodexStreamEvent>) -> Vec<LlmResponse> {
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
            CodexStreamEvent::OutputItemAdded(CodexOutputItemEvent {
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
            CodexStreamEvent::FunctionCallArgumentsDone(CodexFunctionCallArgumentsDoneEvent { output_index: 0 }),
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
        let stream =
            make_stream(vec![CodexStreamEvent::Error(CodexErrorEvent { message: "Rate limit exceeded".to_string() })]);
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
        let events: Vec<Result<CodexStreamEvent>> =
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
        let event = CodexStreamEvent::OutputItemDone(CodexOutputItemEvent {
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
                reasoning_tokens: Some(30),
                ..TokenUsage::default()
            })
        );
    }

    #[tokio::test]
    async fn test_completed_without_output_deserializes_usage_and_stop_reason() {
        let event: CodexStreamEvent = serde_json::from_value(json!({
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
        let event =
            CodexStreamEvent::OutputItemDone(CodexOutputItemEvent { output_index: 0, item: reasoning_item(None) });

        let mut tool_collector = ToolCallCollector::<u32>::new();
        let mut stop_reason = None;
        let responses = process_event(event, &mut tool_collector, &mut stop_reason);

        assert!(responses.is_empty());
    }

    fn text_delta(delta: &str) -> CodexStreamEvent {
        CodexStreamEvent::OutputTextDelta(CodexTextDeltaEvent { delta: delta.to_string() })
    }

    fn reasoning_delta(delta: &str) -> CodexStreamEvent {
        CodexStreamEvent::ReasoningSummaryTextDelta(CodexTextDeltaEvent { delta: delta.to_string() })
    }

    fn function_call_delta(delta: &str) -> CodexStreamEvent {
        CodexStreamEvent::FunctionCallArgumentsDelta(CodexFunctionCallArgumentsDeltaEvent {
            output_index: 0,
            delta: delta.to_string(),
        })
    }

    fn completed(status: Status, usage: Option<ResponseUsage>) -> CodexStreamEvent {
        CodexStreamEvent::Completed(CodexResponseCompletedEvent {
            response: CodexResponseCompleted { usage, status: Some(status) },
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

    fn make_stream(events: Vec<CodexStreamEvent>) -> impl Stream<Item = Result<CodexStreamEvent>> + Send + Unpin {
        tokio_stream::iter(events.into_iter().map(Ok).collect::<Vec<_>>())
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
