//! Fixture-driven `OpenAI`-compatible Chat Completions streaming tests.
//!
//! Each provider's captured SSE body goes through `process_compatible_stream`.
//! `OpenRouter` exercises the richest `*_tokens_details` shape and Z.ai the most
//! minimal one (only `prompt_tokens`, `completion_tokens` and `cached_tokens`),
//! so a parser change that stops accepting either shape fails here.

use llm::providers::openai_compatible::process_compatible_stream;
use llm::{LlmResponse, StopReason};

use crate::providers::common::{expect_usage, parse_fixture};

const PROVIDERS: [&str; 3] = ["openai", "openrouter", "z_ai"];

#[tokio::test]
async fn minimal_fixtures_emit_usage_and_end_turn() {
    for provider in PROVIDERS {
        let events = parse_fixture(provider, "01_minimal", process_compatible_stream).await;
        expect_usage(&events, provider);
        assert_eq!(events.last(), Some(&LlmResponse::done_with_stop_reason(StopReason::EndTurn)), "{provider}");
    }
}

#[tokio::test]
async fn tool_call_fixtures_emit_tool_request_and_usage() {
    for provider in PROVIDERS {
        let events = parse_fixture(provider, "02_tool_call", process_compatible_stream).await;
        assert!(events.iter().any(|e| matches!(e, LlmResponse::ToolRequestComplete { .. })), "{provider}: {events:?}");
        assert_eq!(events.last(), Some(&LlmResponse::done_with_stop_reason(StopReason::ToolCalls)), "{provider}");
        expect_usage(&events, provider);
    }
}

#[tokio::test]
async fn openai_reasoning_reports_reasoning_tokens() {
    let events = parse_fixture("openai", "03_reasoning", process_compatible_stream).await;
    let usage = expect_usage(&events, "openai");
    assert!(usage.reasoning_tokens.is_some_and(|tokens| !tokens.is_zero()), "{usage:?}");
}
