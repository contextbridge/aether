//! Fixture-driven `OpenAI` Chat Completions streaming tests.
//!
//! Loads raw SSE bodies captured from `api.openai.com/v1/chat/completions`,
use llm::{LlmResponse, StopReason};

use crate::providers::common::{assert_minimal_usage, find_usage, parse_compatible_fixture};

#[tokio::test]
async fn openai_minimal_emits_usage() {
    let events = parse_compatible_fixture("openai", "01_minimal").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "01_minimal");
}

#[tokio::test]
async fn openai_minimal_ends_with_done() {
    let events = parse_compatible_fixture("openai", "01_minimal").await;
    let last = events.last().expect("at least one event");
    assert!(
        matches!(last, LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }),
        "last event should be Done(EndTurn), got: {last:?}"
    );
}

#[tokio::test]
async fn openai_tool_call_emits_tool_request_and_usage() {
    let events = parse_compatible_fixture("openai", "02_tool_call").await;

    let has_tool_complete = events.iter().any(|e| matches!(e, LlmResponse::ToolRequestComplete { .. }));
    assert!(has_tool_complete, "02_tool_call should yield a ToolRequestComplete");

    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "02_tool_call");
}

#[tokio::test]
async fn openai_reasoning_reports_reasoning_tokens() {
    let events = parse_compatible_fixture("openai", "03_reasoning").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "03_reasoning");
    assert!(
        usage.reasoning_tokens.is_some_and(|tokens| !tokens.is_zero()),
        "03_reasoning should report reasoning_tokens > 0, got {:?}",
        usage.reasoning_tokens
    );
}
