use agent_client_protocol::schema::v1 as acp;
use tui::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_slash_opens_command_picker() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    press(&mut renderer, KeyCode::Char('/')).await?;

    assert!(has_command_picker(renderer.writer()), "Typing / on empty buffer should open command picker");
    Ok(())
}

#[tokio::test]
async fn test_slash_mid_input_no_picker() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    type_string(&mut renderer, "hello/").await?;

    assert!(!has_command_picker(renderer.writer()), "Typing / mid-input should not open command picker");
    Ok(())
}

#[tokio::test]
async fn test_command_picker_esc_clears() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    press(&mut renderer, KeyCode::Char('/')).await?;
    assert!(has_command_picker(renderer.writer()), "Command picker should be open");

    press(&mut renderer, Esc).await?;

    assert!(!has_command_picker(renderer.writer()), "Esc should close command picker");
    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains('/')),
        "Input buffer should retain '/' after Esc (matches file picker behavior).\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_command_picker_backspace_empty_closes() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    press(&mut renderer, KeyCode::Char('/')).await?;
    assert!(has_command_picker(renderer.writer()), "Command picker should be open");

    press(&mut renderer, Backspace).await?;

    assert!(!has_command_picker(renderer.writer()), "Backspace on empty query should close command picker");
    Ok(())
}

#[tokio::test]
async fn test_available_commands_update_stored() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    renderer.on_session_update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
        vec![acp::AvailableCommand::new("search", "Search code"), acp::AvailableCommand::new("web", "Browse the web")],
    )))?;

    // Open command picker and verify commands appear in rendered output
    press(&mut renderer, KeyCode::Char('/')).await?;

    let names = command_picker_visible_names(renderer.writer());
    assert!(names.iter().any(|n| n == "search"), "Picker should show 'search' command. Got: {names:?}");
    assert!(names.iter().any(|n| n == "web"), "Picker should show 'web' command. Got: {names:?}");
    Ok(())
}

#[tokio::test]
async fn test_available_commands_update_extracts_hint() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    renderer.on_session_update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
        vec![
            acp::AvailableCommand::new("search", "Search code")
                .input(acp::AvailableCommandInput::Unstructured(acp::UnstructuredCommandInput::new("query pattern"))),
            acp::AvailableCommand::new("config", "Open settings"),
        ],
    )))?;

    // Open command picker and verify the hint appears in rendered output
    press(&mut renderer, KeyCode::Char('/')).await?;

    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("query pattern")),
        "Hint text should appear in command picker.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_command_picker_shows_mcp_commands() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    // Feed available commands
    renderer.on_session_update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
        vec![acp::AvailableCommand::new("search", "Search code")],
    )))?;

    // Open picker
    press(&mut renderer, KeyCode::Char('/')).await?;

    let names = command_picker_visible_names(renderer.writer());
    assert!(names.iter().any(|n| n == "settings"), "Picker should include built-in settings command. Got: {names:?}");
    assert!(names.iter().any(|n| n == "search"), "Picker should include MCP search command. Got: {names:?}");
    Ok(())
}

#[tokio::test]
async fn test_command_picker_ctrl_c_exits() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    press(&mut renderer, KeyCode::Char('/')).await?;
    assert!(has_command_picker(renderer.writer()), "Command picker should be open");

    let action = renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await?;
    assert!(matches!(action, LoopAction::Continue), "first Ctrl-C should not exit");

    let action = renderer
        .on_key_event(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await?;
    assert!(matches!(action, LoopAction::Exit), "second Ctrl-C should exit");
    Ok(())
}
