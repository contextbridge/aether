use acp_utils::client::PromptCommand;
use agent_client_protocol::schema::{
    self as acp, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
};
use tui::{KeyCode, KeyEvent, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_shift_tab_cycles_mode_option() -> TestResult {
    let options = vec![
        acp::SessionConfigOption::select(
            "mode",
            "Mode",
            "Planner",
            vec![
                acp::SessionConfigSelectOption::new("Planner", "Planner"),
                acp::SessionConfigSelectOption::new("Coder", "Coder"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Mode),
    ];

    let mut renderer = RendererTest::new().config_options(&options).build()?;

    let action = renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await?;

    assert!(matches!(action, LoopAction::Continue));
    Ok(())
}

#[tokio::test]
async fn test_shift_tab_wraps_mode_option() -> TestResult {
    let options = vec![
        acp::SessionConfigOption::select(
            "mode",
            "Mode",
            "Coder",
            vec![
                acp::SessionConfigSelectOption::new("Planner", "Planner"),
                acp::SessionConfigSelectOption::new("Coder", "Coder"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Mode),
    ];

    let mut renderer = RendererTest::new().config_options(&options).build()?;

    renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await?;
    Ok(())
}

#[tokio::test]
async fn test_shift_tab_advances_local_mode_before_server_response() -> TestResult {
    let options = vec![
        SessionConfigOption::select(
            "mode",
            "Mode",
            "Cheap",
            vec![
                SessionConfigSelectOption::new("Cheap", "Cheap"),
                SessionConfigSelectOption::new("Fast", "Fast"),
                SessionConfigSelectOption::new("Local", "Local"),
            ],
        )
        .category(SessionConfigOptionCategory::Mode),
    ];

    let mut renderer = RendererTest::new().config_options(&options).build()?;
    renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await?;
    renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await?;

    assert_set_config(renderer.commands().try_recv()?, "Fast");
    assert_set_config(renderer.commands().try_recv()?, "Local");
    Ok(())
}

#[tokio::test]
async fn test_shift_tab_ignored_when_overlay_consumes_input() -> TestResult {
    let options = vec![
        acp::SessionConfigOption::select(
            "mode",
            "Mode",
            "Planner",
            vec![acp::SessionConfigSelectOption::new("Planner", "Planner")],
        )
        .category(acp::SessionConfigOptionCategory::Mode),
    ];

    let mut renderer = RendererTest::new().config_options(&options).build()?;

    // Open settings overlay
    type_string(&mut renderer, "/settings").await?;
    press(&mut renderer, Enter).await?;
    assert!(has_settings_menu(renderer.writer()), "Settings overlay should be visible");

    // Send shift+tab — should be swallowed by the overlay
    renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await?;

    // Overlay should still be visible
    assert!(has_settings_menu(renderer.writer()), "Settings overlay should still be visible after shift+tab");
    Ok(())
}

#[tokio::test]
async fn test_shift_tab_noop_when_no_cycleable_option_exists() -> TestResult {
    let options = vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "m1",
            vec![acp::SessionConfigSelectOption::new("m1", "M1"), acp::SessionConfigSelectOption::new("m2", "M2")],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ];

    let mut renderer = RendererTest::new().config_options(&options).build()?;

    let lines_before = renderer.writer().get_lines();

    renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await?;

    let lines_after = renderer.writer().get_lines();
    assert_eq!(lines_before, lines_after, "Shift+Tab should be a no-op when no cycleable mode option");
    Ok(())
}

#[tokio::test]
async fn test_tab_cycles_reasoning_option() -> TestResult {
    use acp_utils::config_option_id::ConfigOptionId;

    let options = vec![acp::SessionConfigOption::select(
        ConfigOptionId::ReasoningEffort.as_str(),
        "Reasoning",
        "none",
        vec![
            acp::SessionConfigSelectOption::new("none", "None"),
            acp::SessionConfigSelectOption::new("low", "Low"),
            acp::SessionConfigSelectOption::new("medium", "Medium"),
        ],
    )];

    let mut renderer = RendererTest::new().config_options(&options).build()?;

    renderer.on_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).await?;
    Ok(())
}

#[tokio::test]
async fn test_tab_noop_when_no_reasoning_option() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    let lines_before = renderer.writer().get_lines();

    renderer.on_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).await?;

    let lines_after = renderer.writer().get_lines();
    assert_eq!(lines_before, lines_after, "Tab should be a no-op when no reasoning option");
    Ok(())
}

fn assert_set_config(command: PromptCommand, expected_value: &str) {
    match command {
        PromptCommand::SetConfigOption { config_id, value, .. } => {
            assert_eq!(config_id, "mode");
            assert_eq!(value, expected_value);
        }

        other => panic!("expected SetConfigOption command, got {other:?}"),
    }
}
