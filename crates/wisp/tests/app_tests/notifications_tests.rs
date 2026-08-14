use acp_utils::notifications::{
    ContextUsage, ContextUsageParams, SubAgentEvent, SubAgentProgressParams, SubAgentToolRequest,
};
use agent_client_protocol::schema::v1 as acp;

use super::common::*;

#[tokio::test]
async fn test_sub_agent_progress_notification_triggers_render() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

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
    renderer.on_sub_agent_progress(params)?;

    let lines = renderer.writer().get_lines();
    assert!(!lines.is_empty());
    Ok(())
}

#[tokio::test]
async fn test_context_usage_notification_updates_nominal_display() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    let params = ContextUsageParams {
        usage: ContextUsage {
            usage_ratio: Some(0.75),
            context_limit: Some(200_000),
            input_tokens: 150_000,
            ..ContextUsage::default()
        },
    };
    renderer.on_context_usage(params)?;

    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("150k / 200k")),
        "Status line should show nominal context usage.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_context_usage_notification_with_unknown_limit_clears_meter() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    let nominal = ContextUsageParams {
        usage: ContextUsage {
            usage_ratio: Some(0.67),
            context_limit: Some(150_000),
            input_tokens: 100_000,
            ..ContextUsage::default()
        },
    };
    renderer.on_context_usage(nominal)?;

    let cleared = ContextUsageParams { usage: ContextUsage::default() };
    renderer.on_context_usage(cleared)?;

    let lines = renderer.writer().get_lines();
    assert!(
        !lines.iter().any(|l| l.contains("ctx")),
        "Context segment should not be shown when limit is unknown.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_context_cleared_notification_resets_conversation() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    renderer.on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("hello world")),
    )))?;

    let lines = renderer.writer().get_lines();
    assert!(lines.iter().any(|l| l.contains("hello world")), "Content should be visible before clear");

    renderer.on_context_cleared()?;

    let lines = renderer.writer().get_lines();
    assert!(
        !lines.iter().any(|l| l.contains("hello world")),
        "Content should be cleared after context_cleared.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_on_tick_requests_render_while_completed_entries() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    renderer.on_session_update(acp::SessionUpdate::Plan(acp::Plan::new(vec![acp::PlanEntry::new(
        "1",
        acp::PlanEntryPriority::Medium,
        acp::PlanEntryStatus::Completed,
    )])))?;

    renderer.on_tick().await?;

    let lines = renderer.writer().get_lines();
    assert!(!lines.is_empty());
    Ok(())
}
