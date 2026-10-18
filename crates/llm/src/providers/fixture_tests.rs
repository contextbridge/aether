use crate::providers::generic::{ANTHROPIC, DEEPSEEK, OPENAI, ProviderConfig};
use crate::providers::test_capture_server::{ProviderFixture, ResponseSpec, hello_context};
use crate::{LlmResponse, StopReason, TokenUsage};

#[tokio::test]
async fn fixtures_emit_usage_and_their_stop_reason() {
    use StopReason::{EndTurn, ToolCalls};
    for (label, config, fixture, stop_reason) in [
        ("openai/01_minimal", &DEEPSEEK, include_str!("../../tests/fixtures/openai/01_minimal.sse"), EndTurn),
        ("openai/02_tool_call", &DEEPSEEK, include_str!("../../tests/fixtures/openai/02_tool_call.sse"), ToolCalls),
        ("openrouter/01_minimal", &DEEPSEEK, include_str!("../../tests/fixtures/openrouter/01_minimal.sse"), EndTurn),
        (
            "openrouter/02_tool_call",
            &DEEPSEEK,
            include_str!("../../tests/fixtures/openrouter/02_tool_call.sse"),
            ToolCalls,
        ),
        ("z_ai/01_minimal", &DEEPSEEK, include_str!("../../tests/fixtures/z_ai/01_minimal.sse"), EndTurn),
        ("z_ai/02_tool_call", &DEEPSEEK, include_str!("../../tests/fixtures/z_ai/02_tool_call.sse"), ToolCalls),
        ("anthropic/01_minimal", &ANTHROPIC, include_str!("../../tests/fixtures/anthropic/01_minimal.sse"), EndTurn),
        (
            "anthropic/02_tool_call",
            &ANTHROPIC,
            include_str!("../../tests/fixtures/anthropic/02_tool_call.sse"),
            ToolCalls,
        ),
        (
            "openai_responses/01_minimal",
            &OPENAI,
            include_str!("../../tests/fixtures/openai_responses/01_minimal.sse"),
            EndTurn,
        ),
    ] {
        let events = replay(config, label, fixture).await;

        expect_usage(&events, label);
        if stop_reason == ToolCalls {
            let completed = events.iter().any(|event| matches!(event, LlmResponse::ToolRequestComplete { .. }));
            assert!(completed, "{label}: {events:?}");
        }
        assert_eq!(events.last(), Some(&LlmResponse::done_with_stop_reason(stop_reason)), "{label}");
    }
}

#[tokio::test]
async fn reasoning_fixtures_report_reasoning_tokens() {
    for (label, config, fixture) in [
        ("openai/03_reasoning", &DEEPSEEK, include_str!("../../tests/fixtures/openai/03_reasoning.sse")),
        (
            "openai_responses/02_reasoning",
            &OPENAI,
            include_str!("../../tests/fixtures/openai_responses/02_reasoning.sse"),
        ),
    ] {
        let usage = expect_usage(&replay(config, label, fixture).await, label);

        assert!(usage.reasoning_tokens.is_some_and(|tokens| !tokens.is_zero()), "{label}: {usage:?}");
    }
}

#[tokio::test]
async fn anthropic_reasoning_fixture_emits_reasoning_chunks() {
    let label = "anthropic/05_reasoning";
    let events = replay(&ANTHROPIC, label, include_str!("../../tests/fixtures/anthropic/05_reasoning.sse")).await;

    assert!(events.iter().any(|event| matches!(event, LlmResponse::Reasoning { .. })), "{label}: {events:?}");
    expect_usage(&events, label);
}

#[tokio::test]
async fn cache_write_fixtures_report_cache_creation() {
    for (label, config, fixture) in [
        ("anthropic/03_cache_write", &ANTHROPIC, include_str!("../../tests/fixtures/anthropic/03_cache_write.sse")),
        (
            "openai_responses/03_mantle_cache_write",
            &OPENAI,
            include_str!("../../tests/fixtures/openai_responses/03_mantle_cache_write.sse"),
        ),
    ] {
        let usage = expect_usage(&replay(config, label, fixture).await, label);

        assert!(usage.cache_creation_tokens.is_some_and(|tokens| !tokens.is_zero()), "{label}: {usage:?}");
    }
}

#[tokio::test]
async fn anthropic_cache_read_fixture_reports_cache_read() {
    let label = "anthropic/04_cache_read";
    let events = replay(&ANTHROPIC, label, include_str!("../../tests/fixtures/anthropic/04_cache_read.sse")).await;

    let usage = expect_usage(&events, label);
    assert!(usage.cache_read_tokens.is_some_and(|tokens| !tokens.is_zero()), "{label}: {usage:?}");
}

/// Chat fixtures replay through `DEEPSEEK` because every Chat Completions provider shares one decoder.
async fn replay(config: &'static ProviderConfig, label: &str, fixture: &'static str) -> Vec<LlmResponse> {
    let mut fixture = ProviderFixture::builder(config).response(ResponseSpec::sse(fixture)).build().await;
    let (_, events) = fixture.run(&hello_context()).await;

    events.into_iter().map(|event| event.unwrap_or_else(|error| panic!("{label}: stream errored: {error}"))).collect()
}

fn expect_usage(events: &[LlmResponse], label: &str) -> TokenUsage {
    let usage = events
        .iter()
        .find_map(|event| match event {
            LlmResponse::Usage { tokens } => Some(*tokens),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{label}: usage event should be present"));
    assert!(!usage.input_tokens.is_zero(), "{label}: input_tokens should be > 0");
    assert!(!usage.output_tokens.is_zero(), "{label}: output_tokens should be > 0");
    usage
}
