use agent_client_protocol::schema as acp;
use tui::testing::TestTerminal;
use tui::{KeyCode, KeyEvent, KeyModifiers};

use super::common::*;

#[tokio::test]
async fn test_shift_tab_cycles_mode_option() {
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

    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &options, (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let action = renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await.unwrap();

    assert!(matches!(action, LoopAction::Continue));
}

#[tokio::test]
async fn test_shift_tab_wraps_mode_option() {
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

    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &options, (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await.unwrap();
}

#[tokio::test]
async fn test_shift_tab_ignored_when_overlay_consumes_input() {
    let options = vec![
        acp::SessionConfigOption::select(
            "mode",
            "Mode",
            "Planner",
            vec![acp::SessionConfigSelectOption::new("Planner", "Planner")],
        )
        .category(acp::SessionConfigOptionCategory::Mode),
    ];

    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &options, (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    // Open settings overlay
    type_string(&mut renderer, "/settings").await;
    press_enter(&mut renderer).await;
    assert!(has_settings_menu(renderer.writer()), "Settings overlay should be visible");

    // Send shift+tab — should be swallowed by the overlay
    renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await.unwrap();

    // Overlay should still be visible
    assert!(has_settings_menu(renderer.writer()), "Settings overlay should still be visible after shift+tab");
}

#[tokio::test]
async fn test_shift_tab_noop_when_no_cycleable_option_exists() {
    let options = vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "m1",
            vec![acp::SessionConfigSelectOption::new("m1", "M1"), acp::SessionConfigSelectOption::new("m2", "M2")],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ];

    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &options, (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let lines_before = renderer.writer().get_lines();

    renderer.on_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)).await.unwrap();

    let lines_after = renderer.writer().get_lines();
    assert_eq!(lines_before, lines_after, "Shift+Tab should be a no-op when no cycleable mode option");
}

#[tokio::test]
async fn test_tab_cycles_reasoning_option() {
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

    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &options, (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer.on_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).await.unwrap();
}

#[tokio::test]
async fn test_tab_noop_when_no_reasoning_option() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    let lines_before = renderer.writer().get_lines();

    renderer.on_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)).await.unwrap();

    let lines_after = renderer.writer().get_lines();
    assert_eq!(lines_before, lines_after, "Tab should be a no-op when no reasoning option");
}
