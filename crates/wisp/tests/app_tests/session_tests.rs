use acp_utils::client::PromptCommand;
use acp_utils::notifications::{SessionDisplayMeta, SessionPreviewResponse, SessionPreviewRole, SessionPreviewTurn};
use agent_client_protocol::schema as acp;
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedReceiver;
use tui::testing::assert_buffer_eq;
use tui::{KeyCode, KeyEvent, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_resume_picker_shows_search_box_and_filters() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    open_resume_picker(&mut renderer).await?;
    renderer.on_sessions_listed(vec![
        acp::SessionInfo::new("session-login", PathBuf::from("/repo/auth")).title("Fix login redirect".to_string()),
        acp::SessionInfo::new("session-billing", PathBuf::from("/repo/billing")).title("Billing cleanup".to_string()),
    ])?;

    assert_buffer_contains(renderer.writer(), "🔍 Search");
    assert_buffer_contains(renderer.writer(), "type to search title or path");
    assert_buffer_not_contains(renderer.writer(), "Resume a previous session");
    type_string(&mut renderer, "login").await?;
    assert_buffer_contains(renderer.writer(), "Fix login redirect");
    assert_buffer_not_contains(renderer.writer(), "Billing cleanup");
    Ok(())
}

#[tokio::test]
async fn test_resume_picker_space_appends_to_query() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    open_resume_picker(&mut renderer).await?;
    renderer.on_sessions_listed(vec![
        session_info("session-login", "/repo/auth", "Fix login redirect"),
        session_info("session-billing", "/repo/billing", "Billing cleanup"),
    ])?;

    type_string(&mut renderer, "Fix ").await?;

    assert_buffer_contains(renderer.writer(), "Fix login redirect");
    assert_buffer_not_contains(renderer.writer(), "Billing cleanup");
    Ok(())
}

#[tokio::test]
async fn test_resume_picker_empty_and_filtered_empty_states() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    open_resume_picker(&mut renderer).await?;
    renderer.on_sessions_listed(vec![])?;
    assert_buffer_contains(renderer.writer(), "No previous sessions found.");

    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;
    open_resume_picker(&mut renderer).await?;
    renderer.on_sessions_listed(vec![session_info("session-login", "/repo/auth", "Fix login redirect")])?;
    type_string(&mut renderer, "nomatch").await?;
    assert_buffer_contains(renderer.writer(), "(no matching sessions)");
    Ok(())
}

#[tokio::test]
async fn test_resume_picker_uses_cwd_basename_title_fallback() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    open_resume_picker(&mut renderer).await?;
    renderer
        .on_sessions_listed(vec![acp::SessionInfo::new("session-untitled", PathBuf::from("/repo/fallback-name"))])?;

    assert_buffer_contains(renderer.writer(), "fallback-name");
    Ok(())
}

#[tokio::test]
async fn test_resume_picker_mouse_scroll_moves_selection_and_requests_preview() -> TestResult {
    let mut renderer = RendererTest::new().build()?;

    open_resume_picker(&mut renderer).await?;
    assert_list_sessions_command(renderer.commands());
    renderer.on_sessions_listed(vec![
        session_info("session-login", "/repo/auth", "Fix login redirect"),
        session_info("session-billing", "/repo/billing", "Billing cleanup"),
    ])?;
    assert_session_preview_command(renderer.commands(), "session-login");

    renderer.on_mouse_scroll_down().await?;

    assert_session_preview_command(renderer.commands(), "session-billing");
    assert_buffer_contains(renderer.writer(), "Billing cleanup");
    renderer.on_mouse_scroll_up().await?;
    assert_buffer_contains(renderer.writer(), "Fix login redirect");
    Ok(())
}

#[tokio::test]
async fn test_resume_picker_shows_metadata_and_lazy_preview() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    open_resume_picker(&mut renderer).await?;
    renderer.on_sessions_listed(vec![session_info("session-login", "/repo/auth", "Fix login redirect")])?;

    assert_buffer_contains(renderer.writer(), "Fix login redirect");
    assert_buffer_contains(renderer.writer(), &format!("auth · {} · claude-sonnet-4-5 · planner", expected_date()));
    assert_buffer_contains(renderer.writer(), "Session preview");
    assert_buffer_contains(renderer.writer(), "Loading…");

    renderer.on_session_preview_loaded(preview("session-login", "/repo/auth", false))?;

    assert_buffer_contains(renderer.writer(), &format!("Created: {}", expected_date()));
    assert_buffer_not_contains(renderer.writer(), "Prompt: Fix login redirect");
    assert_buffer_contains(renderer.writer(), "user: Fix login redirect");
    assert_buffer_contains(renderer.writer(), "user: Run the login tests");
    assert_buffer_contains(renderer.writer(), "Model: claude-sonnet-4-5  Mode: planner");
    assert_buffer_contains(renderer.writer(), "assistant: Updated the auth callback");
    assert_buffer_contains(renderer.writer(), "Tool calls: 2");
    Ok(())
}

#[tokio::test]
async fn test_resume_picker_hides_preview_on_narrow_terminal() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    open_resume_picker(&mut renderer).await?;
    renderer.on_sessions_listed(vec![session_info("session-login", "/repo/auth", "Fix login redirect")])?;
    renderer.on_session_preview_loaded(preview("session-login", "/repo/auth", true))?;

    assert_buffer_contains(renderer.writer(), "Fix login redirect");
    assert_buffer_contains(renderer.writer(), &format!("auth · {} · claude-sonnet-4-5 · planner", expected_date()));
    assert_buffer_not_contains(renderer.writer(), "Session preview");
    assert_buffer_not_contains(renderer.writer(), "… preview truncated");
    Ok(())
}

#[tokio::test]
async fn test_resume_picker_requests_preview_on_highlight_change_and_reuses_loaded_preview() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    open_resume_picker(&mut renderer).await?;
    renderer.on_sessions_listed(vec![
        session_info("session-login", "/repo/auth", "Fix login redirect"),
        session_info("session-billing", "/repo/billing", "Billing cleanup"),
    ])?;
    renderer.on_session_preview_loaded(preview("session-login", "/repo/auth", false))?;

    press(&mut renderer, Down).await?;
    assert_buffer_contains(renderer.writer(), "Loading…");
    renderer.on_session_preview_failed("session-billing", "missing session")?;
    assert_buffer_contains(renderer.writer(), "Error: missing session");

    press(&mut renderer, Up).await?;
    assert_buffer_contains(renderer.writer(), "user: Fix login redirect");
    assert_buffer_not_contains(renderer.writer(), "Loading…");
    assert_buffer_not_contains(renderer.writer(), "Error: missing session");
    Ok(())
}

#[tokio::test]
async fn test_resume_picker_dispatches_preview_requests_and_respects_capability() -> TestResult {
    let mut renderer = RendererTest::new().build()?;

    open_resume_picker(&mut renderer).await?;
    assert_list_sessions_command(renderer.commands());
    renderer.on_sessions_listed(vec![
        session_info("session-login", "/repo/auth", "Fix login redirect"),
        session_info("session-billing", "/repo/billing", "Billing cleanup"),
    ])?;

    assert_session_preview_command(renderer.commands(), "session-login");
    renderer.on_session_preview_loaded(preview("session-login", "/repo/auth", false))?;

    press(&mut renderer, Down).await?;
    assert_session_preview_command(renderer.commands(), "session-billing");
    renderer.on_session_preview_loaded(preview("session-billing", "/repo/billing", false))?;

    press(&mut renderer, Up).await?;
    assert!(renderer.commands().try_recv().is_err(), "loaded previews should not be requested again");

    let mut renderer = RendererTest::new().session_capabilities(acp::SessionCapabilities::new()).build()?;
    open_resume_picker(&mut renderer).await?;
    assert_list_sessions_command(renderer.commands());
    renderer.on_sessions_listed(vec![session_info("session-login", "/repo/auth", "Fix login redirect")])?;
    assert!(
        renderer.commands().try_recv().is_err(),
        "missing sessionPreview capability should suppress preview requests"
    );
    Ok(())
}

#[tokio::test]
async fn test_prompt_done_clears_running_tool_spinner() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    renderer.on_session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new("tool-1", "Read file")))?;

    renderer.on_prompt_done()?;

    // Ensure prompt_done triggers a render and clears the progress indicator.
    let lines = renderer.writer().get_lines();
    let has_progress = lines.iter().any(|l| l.contains("esc to interrupt"));
    assert!(
        !has_progress,
        "Progress indicator should not remain visible after prompt_done.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_prompt_done_flush_respects_rendering() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    renderer.on_session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("theme should be preserved")),
    )))?;

    renderer.on_prompt_done()?;

    // Should render successfully
    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("theme should be preserved")),
        "Thought text should be visible after prompt_done.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_streaming_chunks_keep_waiting_for_response() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    // Submit prompt to enter waiting state
    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;

    // Send a streaming chunk (should not clear waiting state)
    renderer.on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("hello")),
    )))?;

    // Escape should still trigger cancel (proving we're still waiting)
    let action = renderer.on_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await?;

    // If we're still waiting, escape triggers cancel effect which is handled
    assert!(matches!(action, LoopAction::Continue));
    Ok(())
}

#[tokio::test]
async fn test_on_tick_without_active_state_is_noop() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    let lines_before = renderer.writer().get_lines();

    renderer.on_tick().await?;

    let lines_after = renderer.writer().get_lines();
    assert_eq!(lines_before, lines_after, "Tick should be a no-op when nothing active");
    Ok(())
}

#[tokio::test]
async fn test_in_progress_tool_call_visible_after_initial_render() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    renderer.on_session_update(acp::SessionUpdate::ToolCall(
        acp::ToolCall::new("call_1".to_string(), "Read").raw_input(serde_json::json!({"file": "test.rs"})),
    ))?;

    let expected = expected_with_prompt(&[&p("⠒ Read"), "", &p(PROGRESS_LINE), ""], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
    Ok(())
}

#[tokio::test]
async fn test_in_progress_tool_call_renders_correctly_after_resize() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    renderer.on_session_update(acp::SessionUpdate::ToolCall(
        acp::ToolCall::new("call_1".to_string(), "Read").raw_input(serde_json::json!({"file": "test.rs"})),
    ))?;

    // Terminal resize triggers full re-render at new width
    renderer.on_resize_event(100, 30).await?;

    let expected = expected_with_prompt(&[&p("⠒ Read"), "", &p(PROGRESS_LINE), ""], 100, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
    Ok(())
}

/// Bug repro: completed conversation content must re-render at the new width
/// after a terminal resize. Previously, completed turns were drained to
/// fixed-width terminal scrollback and could not be re-rendered.
#[tokio::test]
async fn test_completed_content_re_renders_at_new_width_after_resize() -> TestResult {
    let initial_width: u16 = 40;
    let mut renderer = RendererTest::new().size((initial_width, 20)).build()?;

    // Complete a full turn: text + tool call + prompt_done
    renderer.on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("First answer")),
    )))?;
    renderer.on_prompt_done()?;

    // Verify content is visible at original width
    let lines_before = renderer.writer().get_lines();
    assert!(
        lines_before.iter().any(|l| l.contains("First answer")),
        "Content should be visible before resize.\nBuffer:\n{}",
        lines_before.join("\n")
    );

    // Widen the terminal — resize both the renderer and the TestTerminal buffer
    let new_width: u16 = 100;
    renderer.test_writer_mut().resize(new_width, 20);
    renderer.on_resize_event(new_width, 20).await?;

    // Content from the completed turn must still be visible and the prompt
    // must be rendered at the new width
    let lines_after = renderer.writer().get_lines();
    assert!(
        lines_after.iter().any(|l| l.contains("First answer")),
        "Completed content should survive resize and re-render at new width.\nBuffer:\n{}",
        lines_after.join("\n")
    );

    let expected = expected_with_prompt(&[&p("First answer")], new_width, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
    Ok(())
}

/// Bug repro: the prompt box must not garble after resizing when there is
/// completed conversation content above it. Previously, stale overflow
/// counts caused the `VisualFrame` visible/scrollback split to break,
/// producing duplicated or corrupted prompt lines.
#[tokio::test]
async fn test_prompt_not_garbled_after_resize_with_completed_content() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 12)).build()?;

    // Build up several completed turns so there's content above the prompt
    renderer.on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("Turn one")),
    )))?;
    renderer.on_prompt_done()?;

    renderer.on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("Turn two")),
    )))?;
    renderer.on_prompt_done()?;

    // Resize the terminal
    renderer.test_writer_mut().resize(60, 10);
    renderer.on_resize_event(60, 10).await?;

    let lines = renderer.writer().get_lines();

    let rule = "─".repeat(60);
    let rule_count = lines.iter().filter(|l| **l == rule).count();
    assert_eq!(rule_count, 2, "Prompt rules should appear exactly twice after resize.\nBuffer:\n{}", lines.join("\n"));

    // Both turns' content should appear exactly once — no duplication from
    // stale overflow counts causing content to appear in both scrollback and
    // the visible frame.
    let turn_one_count = lines.iter().filter(|l| l.contains("Turn one")).count();
    let turn_two_count = lines.iter().filter(|l| l.contains("Turn two")).count();
    assert_eq!(turn_one_count, 1, "Turn one should appear exactly once after resize.\nBuffer:\n{}", lines.join("\n"));
    assert_eq!(turn_two_count, 1, "Turn two should appear exactly once after resize.\nBuffer:\n{}", lines.join("\n"));
    Ok(())
}

#[tokio::test]
async fn test_replay_history_only_visible_after_session_loaded() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;
    let resumed = acp::SessionId::new("resumed-session");
    load_session_via_picker(&mut renderer, resumed.clone(), std::path::PathBuf::from("/project")).await?;

    let first_msg = "1st";
    let second_msg = "2nd";
    renderer.on_session_update_for(
        resumed.clone(),
        acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
            first_msg,
        )))),
    )?;

    renderer.on_session_update_for(
        resumed.clone(),
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
            second_msg,
        )))),
    )?;

    assert_buffer_not_contains(renderer.writer(), first_msg);
    assert_buffer_not_contains(renderer.writer(), second_msg);
    renderer.on_session_loaded(resumed, vec![])?;

    let lines = renderer.writer().get_lines();
    let early_idx = lines.iter().position(|l| l.contains(first_msg)).expect("early line present");
    let late_idx = lines.iter().position(|l| l.contains(second_msg)).expect("late line present");
    assert!(early_idx < late_idx);
    Ok(())
}

fn assert_list_sessions_command(commands: &mut UnboundedReceiver<PromptCommand>) {
    match commands.try_recv().expect("expected list sessions command") {
        PromptCommand::ListSessions => {}
        other => panic!("expected ListSessions command, got {other:?}"),
    }
}

fn assert_session_preview_command(commands: &mut UnboundedReceiver<PromptCommand>, expected_session_id: &str) {
    match commands.try_recv().expect("expected preview command") {
        PromptCommand::SessionPreview(params) => assert_eq!(params.session_id, expected_session_id),
        other => panic!("expected SessionPreview command, got {other:?}"),
    }
}

fn expected_date() -> &'static str {
    "Jun 1 2000 00:00"
}

fn session_info(id: &str, cwd: &str, title: &str) -> acp::SessionInfo {
    let meta = SessionDisplayMeta::new("claude-sonnet-4-5", Some("planner".to_string())).to_meta();
    acp::SessionInfo::new(id.to_string(), PathBuf::from(cwd))
        .title(title.to_string())
        .updated_at("2000-06-01T00:00:00Z".to_string())
        .meta(meta)
}

fn preview(session_id: &str, cwd: &str, truncated: bool) -> SessionPreviewResponse {
    SessionPreviewResponse {
        session_id: session_id.to_string(),
        cwd: PathBuf::from(cwd),
        created_at: "2000-06-01T00:00:00Z".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        selected_mode: Some("planner".to_string()),
        transcript: vec![
            SessionPreviewTurn { role: SessionPreviewRole::User, text: "Fix login redirect".to_string() },
            SessionPreviewTurn { role: SessionPreviewRole::User, text: "Run the login tests".to_string() },
            SessionPreviewTurn { role: SessionPreviewRole::Assistant, text: "Updated the auth callback".to_string() },
        ],
        tool_call_count: 2,
        truncated,
    }
}
