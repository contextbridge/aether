use agent_client_protocol::schema as acp;
use tui::testing::{TestTerminal, assert_buffer_eq};
use tui::{KeyCode, KeyEvent, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_prompt_done_clears_running_tool_spinner() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    // Send a tool call that never gets a terminal update.
    renderer.on_session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new("tool-1", "Read file"))).unwrap();

    renderer.on_prompt_done().unwrap();

    let lines = renderer.writer().get_lines();
    let has_progress = lines.iter().any(|l| l.contains("esc to interrupt"));
    assert!(
        !has_progress,
        "Progress indicator should not remain visible after prompt_done.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_prompt_done_flush_respects_rendering() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("theme should be preserved"),
        ))))
        .unwrap();

    renderer.on_prompt_done().unwrap();

    // Should render successfully
    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("theme should be preserved")),
        "Thought text should be visible after prompt_done.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_streaming_chunks_keep_waiting_for_response() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    // Submit prompt to enter waiting state
    type_string(&mut renderer, "Hello").await;
    press_enter(&mut renderer).await;

    // Send a streaming chunk (should not clear waiting state)
    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("hello"),
        ))))
        .unwrap();

    // Escape should still trigger cancel (proving we're still waiting)
    let action = renderer.on_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).await.unwrap();

    // If we're still waiting, escape triggers cancel effect which is handled
    assert!(matches!(action, LoopAction::Continue));
}

#[tokio::test]
async fn test_on_tick_without_active_state_is_noop() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let lines_before = renderer.writer().get_lines();

    renderer.on_tick().await.unwrap();

    let lines_after = renderer.writer().get_lines();
    assert_eq!(lines_before, lines_after, "Tick should be a no-op when nothing active");
}

#[tokio::test]
async fn test_in_progress_tool_call_visible_after_initial_render() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));

    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::ToolCall(
            acp::ToolCall::new("call_1".to_string(), "Read").raw_input(serde_json::json!({"file": "test.rs"})),
        ))
        .unwrap();

    let expected = expected_with_prompt(&[&p("⠒ Read"), "", &p(PROGRESS_LINE), ""], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_in_progress_tool_call_renders_correctly_after_resize() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::ToolCall(
            acp::ToolCall::new("call_1".to_string(), "Read").raw_input(serde_json::json!({"file": "test.rs"})),
        ))
        .unwrap();

    // Terminal resize triggers full re-render at new width
    renderer.on_resize_event(100, 30).await.unwrap();

    let expected = expected_with_prompt(&[&p("⠒ Read"), "", &p(PROGRESS_LINE), ""], 100, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

/// Bug repro: completed conversation content must re-render at the new width
/// after a terminal resize. Previously, completed turns were drained to
/// fixed-width terminal scrollback and could not be re-rendered.
#[tokio::test]
async fn test_completed_content_re_renders_at_new_width_after_resize() {
    let initial_width: u16 = 40;
    let terminal = TestTerminal::new(initial_width, 20);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (initial_width, 20));
    renderer.initial_render().unwrap();

    // Complete a full turn: text + tool call + prompt_done
    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("First answer"),
        ))))
        .unwrap();
    renderer.on_prompt_done().unwrap();

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
    renderer.on_resize_event(new_width, 20).await.unwrap();

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
}

/// Bug repro: the prompt box must not garble after resizing when there is
/// completed conversation content above it. Previously, stale overflow
/// counts caused the `VisualFrame` visible/scrollback split to break,
/// producing duplicated or corrupted prompt lines.
#[tokio::test]
async fn test_prompt_not_garbled_after_resize_with_completed_content() {
    let terminal = TestTerminal::new(80, 12);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 12));
    renderer.initial_render().unwrap();

    // Build up several completed turns so there's content above the prompt
    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("Turn one"),
        ))))
        .unwrap();
    renderer.on_prompt_done().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("Turn two"),
        ))))
        .unwrap();
    renderer.on_prompt_done().unwrap();

    // Resize the terminal
    renderer.test_writer_mut().resize(60, 10);
    renderer.on_resize_event(60, 10).await.unwrap();

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
}

#[tokio::test]
async fn test_replay_history_only_visible_after_session_loaded() {
    let mut renderer = new_test_renderer((TEST_WIDTH, 40));
    let resumed = acp::SessionId::new("resumed-session");
    load_session_via_picker(&mut renderer, resumed.clone(), std::path::PathBuf::from("/project")).await;

    let first_msg = "1st";
    let second_msg = "2nd";
    renderer
        .on_session_update_for(
            resumed.clone(),
            acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(first_msg),
            ))),
        )
        .unwrap();

    renderer
        .on_session_update_for(
            resumed.clone(),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(second_msg),
            ))),
        )
        .unwrap();

    assert_buffer_not_contains(renderer.writer(), first_msg);
    assert_buffer_not_contains(renderer.writer(), second_msg);
    renderer.on_session_loaded(resumed, vec![]).unwrap();

    let lines = renderer.writer().get_lines();
    let early_idx = lines.iter().position(|l| l.contains(first_msg)).expect("early line present");
    let late_idx = lines.iter().position(|l| l.contains(second_msg)).expect("late line present");
    assert!(early_idx < late_idx);
}
