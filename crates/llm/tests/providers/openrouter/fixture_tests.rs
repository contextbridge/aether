//! Fixture-driven `OpenRouter` streaming tests.
//!
//! Loads raw SSE bodies captured from `openrouter.ai/api/v1/chat/completions`,
//! deserializes each `data:` line into the `OpenAI`-compatible
//! `ChatCompletionStreamResponse`, and feeds the typed events through
//! `process_compatible_stream`. This is the path that exercises `OpenRouter`'s
//! richer `prompt_tokens_details` / `completion_tokens_details` shape.

use llm::{LlmResponse, StopReason};

use crate::providers::common::{assert_minimal_usage, find_usage, parse_compatible_fixture};

#[tokio::test]
async fn openrouter_minimal_emits_usage() {
    let events = parse_compatible_fixture("openrouter", "01_minimal").await;
    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "01_minimal");
}

#[tokio::test]
async fn openrouter_minimal_ends_with_done() {
    let events = parse_compatible_fixture("openrouter", "01_minimal").await;
    let last = events.last().expect("at least one event");
    assert!(
        matches!(last, LlmResponse::Done { stop_reason: Some(StopReason::EndTurn) }),
        "last event should be Done(EndTurn), got: {last:?}"
    );
}

#[tokio::test]
async fn openrouter_tool_call_emits_tool_request_and_usage() {
    let events = parse_compatible_fixture("openrouter", "02_tool_call").await;

    let has_tool_complete = events.iter().any(|e| matches!(e, LlmResponse::ToolRequestComplete { .. }));
    assert!(has_tool_complete, "02_tool_call should yield a ToolRequestComplete");

    let usage = find_usage(&events).expect("usage event should be present");
    assert_minimal_usage(&usage, "02_tool_call");
}
