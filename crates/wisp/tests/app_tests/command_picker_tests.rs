use agent_client_protocol::schema as acp;
use tui::testing::TestTerminal;
use tui::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_slash_opens_command_picker() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('/'), KeyModifiers::empty()).await;

    assert!(has_command_picker(renderer.writer()), "Typing / on empty buffer should open command picker");
}

#[tokio::test]
async fn test_slash_mid_input_no_picker() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    type_string(&mut renderer, "hello/").await;

    assert!(!has_command_picker(renderer.writer()), "Typing / mid-input should not open command picker");
}

#[tokio::test]
async fn test_command_picker_esc_clears() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('/'), KeyModifiers::empty()).await;
    assert!(has_command_picker(renderer.writer()), "Command picker should be open");

    send_key(&mut renderer, KeyCode::Esc, KeyModifiers::empty()).await;

    assert!(!has_command_picker(renderer.writer()), "Esc should close command picker");
    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains('/')),
        "Input buffer should retain '/' after Esc (matches file picker behavior).\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_command_picker_backspace_empty_closes() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('/'), KeyModifiers::empty()).await;
    assert!(has_command_picker(renderer.writer()), "Command picker should be open");

    send_key(&mut renderer, KeyCode::Backspace, KeyModifiers::empty()).await;

    assert!(!has_command_picker(renderer.writer()), "Backspace on empty query should close command picker");
}

#[tokio::test]
async fn test_available_commands_update_stored() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(vec![
            acp::AvailableCommand::new("search", "Search code"),
            acp::AvailableCommand::new("web", "Browse the web"),
        ])))
        .unwrap();

    // Open command picker and verify commands appear in rendered output
    send_key(&mut renderer, KeyCode::Char('/'), KeyModifiers::empty()).await;

    let names = command_picker_visible_names(renderer.writer());
    assert!(names.iter().any(|n| n == "search"), "Picker should show 'search' command. Got: {names:?}");
    assert!(names.iter().any(|n| n == "web"), "Picker should show 'web' command. Got: {names:?}");
}

#[tokio::test]
async fn test_available_commands_update_extracts_hint() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(vec![
            acp::AvailableCommand::new("search", "Search code")
                .input(acp::AvailableCommandInput::Unstructured(acp::UnstructuredCommandInput::new("query pattern"))),
            acp::AvailableCommand::new("config", "Open settings"),
        ])))
        .unwrap();

    // Open command picker and verify the hint appears in rendered output
    send_key(&mut renderer, KeyCode::Char('/'), KeyModifiers::empty()).await;

    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("query pattern")),
        "Hint text should appear in command picker.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_command_picker_shows_mcp_commands() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    // Feed available commands
    renderer
        .on_session_update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(vec![
            acp::AvailableCommand::new("search", "Search code"),
        ])))
        .unwrap();

    // Open picker
    send_key(&mut renderer, KeyCode::Char('/'), KeyModifiers::empty()).await;

    let names = command_picker_visible_names(renderer.writer());
    assert!(names.iter().any(|n| n == "settings"), "Picker should include built-in settings command. Got: {names:?}");
    assert!(names.iter().any(|n| n == "search"), "Picker should include MCP search command. Got: {names:?}");
}

#[tokio::test]
async fn test_command_picker_ctrl_c_exits() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));
    renderer.initial_render().unwrap();

    send_key(&mut renderer, KeyCode::Char('/'), KeyModifiers::empty()).await;
    assert!(has_command_picker(renderer.writer()), "Command picker should be open");

    let action = renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await
        .unwrap();
    assert!(matches!(action, LoopAction::Continue), "first Ctrl-C should not exit");

    let action = renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await
        .unwrap();
    assert!(matches!(action, LoopAction::Exit), "second Ctrl-C should exit");
}
