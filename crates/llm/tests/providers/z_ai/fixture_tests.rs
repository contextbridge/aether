//! Fixture-driven Z.ai streaming tests.
//!
//! Loads raw SSE bodies captured from `api.z.ai/api/paas/v4/chat/completions`
//! and feeds them through `process_compatible_stream`. Z.ai's usage shape is
//! the most minimal of the OpenAI-compatible providers — it typically exposes
//! only `prompt_tokens`, `completion_tokens`, and `cached_tokens`. The fixture
//! test guards against regressions where required fields silently become
//! `None` because the parser stops accepting Z.ai's specific shape.

use llm::{LlmResponse, StopReason};

use crate::providers::common::{assert_minimal_usage, find_usage, parse_compatible_fixture};

#[tokio::test]
async fn z_ai_minimal_emits_usage() {
    let events = parse_compatible_fixture("z_ai", "01_minimal").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "01_minimal");
}

#[tokio::test]
async fn z_ai_minimal_ends_with_done() {
    let events = parse_compatible_fixture("z_ai", "01_minimal").await;
    let last = events.last().expect("at least one event");
    assert!(
        matches!(last, LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }),
        "last event should be Done(EndTurn), got: {last:?}"
    );
}

#[tokio::test]
async fn z_ai_tool_call_emits_tool_request_and_usage() {
    let events = parse_compatible_fixture("z_ai", "02_tool_call").await;

    let has_tool_complete = events.iter().any(|e| matches!(e, LlmResponse::ToolRequestComplete { .. }));
    assert!(has_tool_complete, "02_tool_call should yield a ToolRequestComplete");

    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "02_tool_call");
}
