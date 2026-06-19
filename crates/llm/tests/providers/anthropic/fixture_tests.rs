//! Fixture-driven Anthropic streaming tests.
//!
//! Loads raw SSE bodies captured from `api.anthropic.com/v1/messages` and
//! feeds them through `process_anthropic_stream`. Asserts structural
//! properties of the parsed `TokenUsage` rather than exact token counts so
//! re-captures don't require re-baselining.

use llm::providers::anthropic::streaming::process_anthropic_stream;
use llm::{LlmResponse, StopReason};
use tokio_stream::StreamExt;

use crate::providers::common::{assert_minimal_usage, find_usage, parse_sse_data_lines, read_fixture};

async fn parse_fixture(scenario: &str) -> Vec<LlmResponse> {
    let bytes = read_fixture("anthropic", scenario);
    let lines = parse_sse_data_lines(&bytes);
    let stream = tokio_stream::iter(lines.into_iter().map(Ok));
    let mut processed = Box::pin(process_anthropic_stream(stream));
    let mut events = Vec::new();
    while let Some(event) = processed.next().await {
        events.push(event.expect("stream item should not error"));
    }
    events
}

#[tokio::test]
async fn anthropic_minimal_emits_usage() {
    let events = parse_fixture("01_minimal").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "01_minimal");
}

#[tokio::test]
async fn anthropic_minimal_ends_with_done() {
    let events = parse_fixture("01_minimal").await;
    let last = events.last().expect("at least one event");
    assert!(
        matches!(last, LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }),
        "last event should be Done(EndTurn), got: {last:?}"
    );
}

#[tokio::test]
async fn anthropic_tool_call_emits_tool_request() {
    let events = parse_fixture("02_tool_call").await;
    let has_tool_complete = events.iter().any(|e| matches!(e, LlmResponse::ToolRequestComplete { .. }));
    assert!(has_tool_complete, "tool_call fixture should yield a ToolRequestComplete");

    let last = events.last().expect("at least one event");
    assert!(
        matches!(last, LlmResponse::Done { stop_reason: Some(StopReason::ToolCalls) }),
        "tool_call fixture should end with Done(ToolCalls), got: {last:?}"
    );

    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "02_tool_call");
}

#[tokio::test]
async fn anthropic_cache_write_reports_cache_creation() {
    let events = parse_fixture("03_cache_write").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "03_cache_write");
    assert!(
        usage.cache_creation_tokens.unwrap_or(0) > 0,
        "03_cache_write should report cache_creation_tokens > 0, got {:?}",
        usage.cache_creation_tokens
    );
}

#[tokio::test]
async fn anthropic_cache_read_reports_cache_read() {
    let events = parse_fixture("04_cache_read").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "04_cache_read");
    assert!(
        usage.cache_read_tokens.unwrap_or(0) > 0,
        "04_cache_read should report cache_read_tokens > 0, got {:?}",
        usage.cache_read_tokens
    );
}

#[tokio::test]
async fn anthropic_reasoning_emits_reasoning_chunks() {
    let events = parse_fixture("05_reasoning").await;
    let has_reasoning = events.iter().any(|e| matches!(e, LlmResponse::Reasoning { .. }));
    assert!(has_reasoning, "reasoning fixture should yield Reasoning events");

    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "05_reasoning");
}
