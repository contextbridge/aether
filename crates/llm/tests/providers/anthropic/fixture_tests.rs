//! Fixture-driven Anthropic streaming tests.
//!
//! Loads raw SSE bodies captured from `api.anthropic.com/v1/messages` and
//! feeds them through `process_anthropic_stream`. Asserts structural
//! properties of the parsed `TokenUsage` rather than exact token counts so
//! re-captures don't require re-baselining.

use llm::providers::anthropic::streaming::process_anthropic_stream;
use llm::{LlmResponse, StopReason};

use crate::providers::common::{expect_usage, parse_fixture};

async fn parse(scenario: &str) -> Vec<LlmResponse> {
    parse_fixture("anthropic", scenario, process_anthropic_stream).await
}

#[tokio::test]
async fn anthropic_minimal_emits_usage_and_ends_turn() {
    let events = parse("01_minimal").await;
    expect_usage(&events, "01_minimal");
    assert_eq!(events.last(), Some(&LlmResponse::done_with_stop_reason(StopReason::EndTurn)));
}

#[tokio::test]
async fn anthropic_tool_call_emits_tool_request() {
    let events = parse("02_tool_call").await;
    assert!(events.iter().any(|e| matches!(e, LlmResponse::ToolRequestComplete { .. })), "{events:?}");
    assert_eq!(events.last(), Some(&LlmResponse::done_with_stop_reason(StopReason::ToolCalls)));
    expect_usage(&events, "02_tool_call");
}

#[tokio::test]
async fn anthropic_cache_write_reports_cache_creation() {
    let events = parse("03_cache_write").await;
    let usage = expect_usage(&events, "03_cache_write");
    assert!(usage.cache_creation_tokens.is_some_and(|tokens| !tokens.is_zero()), "{usage:?}");
}

#[tokio::test]
async fn anthropic_cache_read_reports_cache_read() {
    let events = parse("04_cache_read").await;
    let usage = expect_usage(&events, "04_cache_read");
    assert!(usage.cache_read_tokens.is_some_and(|tokens| !tokens.is_zero()), "{usage:?}");
}

#[tokio::test]
async fn anthropic_reasoning_emits_reasoning_chunks() {
    let events = parse("05_reasoning").await;
    assert!(events.iter().any(|e| matches!(e, LlmResponse::Reasoning { .. })), "{events:?}");
    expect_usage(&events, "05_reasoning");
}
