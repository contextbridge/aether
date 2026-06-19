//! Fixture-driven Z.ai streaming tests.
//!
//! Loads raw SSE bodies captured from `api.z.ai/api/paas/v4/chat/completions`
//! and feeds them through `process_compatible_stream`. Z.ai's usage shape is
//! the most minimal of the OpenAI-compatible providers — it typically exposes
//! only `prompt_tokens`, `completion_tokens`, and `cached_tokens`. The fixture
//! test guards against regressions where required fields silently become
//! `None` because the parser stops accepting Z.ai's specific shape.

use llm::providers::openai_compatible::streaming::process_compatible_stream;
use llm::providers::openai_compatible::types::ChatCompletionStreamResponse;
use llm::{LlmResponse, StopReason};
use tokio_stream::StreamExt;

use crate::providers::common::{assert_minimal_usage, find_usage, parse_sse_data_lines, read_fixture};

async fn parse_fixture(scenario: &str) -> Vec<LlmResponse> {
    let bytes = read_fixture("z_ai", scenario);
    let lines = parse_sse_data_lines(&bytes);
    let chunks: Vec<ChatCompletionStreamResponse> = lines
        .into_iter()
        .filter_map(|line| match serde_json::from_str(&line) {
            Ok(chunk) => Some(chunk),
            Err(e) => {
                eprintln!("z_ai/{scenario}: skipping unparseable line: {e}\n  line: {line}");
                None
            }
        })
        .collect();

    let stream = tokio_stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>));
    let mut processed = Box::pin(process_compatible_stream(stream));
    let mut events = Vec::new();
    while let Some(event) = processed.next().await {
        events.push(event.expect("stream item should not error"));
    }
    events
}

#[tokio::test]
async fn z_ai_minimal_emits_usage() {
    let events = parse_fixture("01_minimal").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "01_minimal");
}

#[tokio::test]
async fn z_ai_minimal_ends_with_done() {
    let events = parse_fixture("01_minimal").await;
    let last = events.last().expect("at least one event");
    assert!(
        matches!(last, LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }),
        "last event should be Done(EndTurn), got: {last:?}"
    );
}

#[tokio::test]
async fn z_ai_tool_call_emits_tool_request_and_usage() {
    let events = parse_fixture("02_tool_call").await;

    let has_tool_complete = events.iter().any(|e| matches!(e, LlmResponse::ToolRequestComplete { .. }));
    assert!(has_tool_complete, "02_tool_call should yield a ToolRequestComplete");

    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "02_tool_call");
}
