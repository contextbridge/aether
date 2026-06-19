use acp_utils::notifications::{ContextUsageParams, SubAgentEvent, SubAgentProgressParams, SubAgentToolRequest};
use agent_client_protocol::schema as acp;
use tui::testing::TestTerminal;

use super::common::*;

#[tokio::test]
async fn test_sub_agent_progress_notification_triggers_render() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let params = SubAgentProgressParams {
        parent_tool_id: "p1".to_string(),
        task_id: "t1".to_string(),
        agent_name: "explorer".to_string(),
        event: SubAgentEvent::ToolCall {
            request: SubAgentToolRequest {
                id: "c1".to_string(),
                name: "grep".to_string(),
                arguments: "{}".to_string(),
            },
        },
    };
    renderer.on_sub_agent_progress(params).unwrap();

    let lines = renderer.writer().get_lines();
    assert!(!lines.is_empty());
}

#[tokio::test]
async fn test_context_usage_notification_updates_nominal_display() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let params = ContextUsageParams {
        usage_ratio: Some(0.75),
        context_limit: Some(200_000),
        input_tokens: 150_000,
        output_tokens: 0,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        reasoning_tokens: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_read_tokens: 0,
        total_cache_creation_tokens: 0,
        total_reasoning_tokens: 0,
    };
    renderer.on_context_usage(params).unwrap();

    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("150k / 200k")),
        "Status line should show nominal context usage.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_context_usage_notification_with_unknown_limit_clears_meter() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let nominal = ContextUsageParams {
        usage_ratio: Some(0.67),
        context_limit: Some(150_000),
        input_tokens: 100_000,
        output_tokens: 0,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        reasoning_tokens: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_read_tokens: 0,
        total_cache_creation_tokens: 0,
        total_reasoning_tokens: 0,
    };
    renderer.on_context_usage(nominal).unwrap();

    let cleared = ContextUsageParams {
        usage_ratio: None,
        context_limit: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        reasoning_tokens: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_read_tokens: 0,
        total_cache_creation_tokens: 0,
        total_reasoning_tokens: 0,
    };
    renderer.on_context_usage(cleared).unwrap();

    let lines = renderer.writer().get_lines();
    assert!(
        !lines.iter().any(|l| l.contains("ctx")),
        "Context segment should not be shown when limit is unknown.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_context_cleared_notification_resets_conversation() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("hello world"),
        ))))
        .unwrap();

    let lines = renderer.writer().get_lines();
    assert!(lines.iter().any(|l| l.contains("hello world")), "Content should be visible before clear");

    renderer.on_context_cleared().unwrap();

    let lines = renderer.writer().get_lines();
    assert!(
        !lines.iter().any(|l| l.contains("hello world")),
        "Content should be cleared after context_cleared.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_on_tick_requests_render_while_completed_entries() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::Plan(acp::Plan::new(vec![acp::PlanEntry::new(
            "1",
            acp::PlanEntryPriority::Medium,
            acp::PlanEntryStatus::Completed,
        )])))
        .unwrap();

    renderer.on_tick().await.unwrap();

    let lines = renderer.writer().get_lines();
    assert!(!lines.is_empty());
}
