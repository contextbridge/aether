//! Fixture-driven `OpenAI` Chat Completions streaming tests.
//!
//! Loads raw SSE bodies captured from `api.openai.com/v1/chat/completions`,
//! deserializes each `data:` line into `CreateChatCompletionStreamResponse`,
//! and feeds the typed events through `process_completion_stream`.

use async_openai::types::chat::CreateChatCompletionStreamResponse;
use llm::providers::openai::streaming::process_completion_stream;
use llm::{LlmResponse, StopReason};
use tokio_stream::StreamExt;

use crate::providers::common::{assert_minimal_usage, find_usage, parse_sse_data_lines, read_fixture};

async fn parse_fixture(scenario: &str) -> Vec<LlmResponse> {
    let bytes = read_fixture("openai", scenario);
    let lines = parse_sse_data_lines(&bytes);
    let chunks: Vec<CreateChatCompletionStreamResponse> = lines
        .into_iter()
        .filter_map(|line| match serde_json::from_str(&line) {
            Ok(chunk) => Some(chunk),
            Err(e) => {
                eprintln!("openai/{scenario}: skipping unparseable line: {e}\n  line: {line}");
                None
            }
        })
        .collect();

    let stream = tokio_stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>));
    let mut processed = Box::pin(process_completion_stream(stream));
    let mut events = Vec::new();
    while let Some(event) = processed.next().await {
        events.push(event.expect("stream item should not error"));
    }
    events
}

#[tokio::test]
async fn openai_minimal_emits_usage() {
    let events = parse_fixture("01_minimal").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "01_minimal");
}

#[tokio::test]
async fn openai_minimal_ends_with_done() {
    let events = parse_fixture("01_minimal").await;
    let last = events.last().expect("at least one event");
    assert!(
        matches!(last, LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }),
        "last event should be Done(EndTurn), got: {last:?}"
    );
}

#[tokio::test]
async fn openai_tool_call_emits_tool_request_and_usage() {
    let events = parse_fixture("02_tool_call").await;

    let has_tool_complete = events.iter().any(|e| matches!(e, LlmResponse::ToolRequestComplete { .. }));
    assert!(has_tool_complete, "02_tool_call should yield a ToolRequestComplete");

    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "02_tool_call");
}

#[tokio::test]
async fn openai_reasoning_reports_reasoning_tokens() {
    let events = parse_fixture("03_reasoning").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "03_reasoning");
    assert!(
        usage.reasoning_tokens.unwrap_or(0) > 0,
        "03_reasoning should report reasoning_tokens > 0, got {:?}",
        usage.reasoning_tokens
    );
}
