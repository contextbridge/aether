use agent_client_protocol::schema::v1 as acp;
use tui::KeyCode;
use tui::KeyEvent;
use tui::KeyEventKind;
use tui::KeyEventState;
use tui::KeyModifiers;

use super::common::*;

#[tokio::test]
async fn test_settings_command_opens_menu() -> TestResult {
    let r = open_settings(&make_settings_options(), (80, 24)).await?;
    assert!(has_settings_menu(r.writer()), "Settings menu should be visible");
    assert!(!has_settings_picker(r.writer()), "Settings picker should not be visible");
    Ok(())
}

fn make_provider_auth_methods() -> Vec<acp::AuthMethod> {
    vec![
        acp::AuthMethod::Agent(acp::AuthMethodAgent::new("anthropic", "Anthropic")),
        acp::AuthMethod::Agent(acp::AuthMethodAgent::new("openrouter", "OpenRouter")),
    ]
}

#[tokio::test]
async fn test_auth_methods_updated_notification_refreshes_provider_login_and_persists_on_reopen() -> TestResult {
    let mut r = RendererTest::new().auth_methods(make_provider_auth_methods()).build()?;
    type_string(&mut r, "/settings").await?;
    press(&mut r, Enter).await?;
    press(&mut r, Down).await?;
    press(&mut r, Down).await?;

    press(&mut r, Enter).await?;
    assert_buffer_contains(r.writer(), "Anthropic  ⚡ needs login");

    let updated = vec![
        acp::AuthMethod::Agent(acp::AuthMethodAgent::new("anthropic", "Anthropic").description("authenticated")),
        acp::AuthMethod::Agent(acp::AuthMethodAgent::new("openrouter", "OpenRouter")),
    ];
    r.on_auth_methods_updated(acp_utils::notifications::AuthMethodsUpdatedParams { auth_methods: updated })?;
    assert_buffer_contains(r.writer(), "Anthropic  ✓ logged in");

    press(&mut r, Esc).await?;
    assert!(has_settings_menu(r.writer()));
    press(&mut r, Enter).await?;
    assert_buffer_contains(r.writer(), "Anthropic  ✓ logged in");
    Ok(())
}

#[tokio::test]
async fn test_settings_menu_esc_closes() -> TestResult {
    let mut r = open_settings(&make_settings_options(), (80, 24)).await?;
    assert!(has_settings_menu(r.writer()));
    assert!(!has_settings_picker(r.writer()));

    // Open the picker
    press(&mut r, Enter).await?;
    assert!(has_settings_menu(r.writer()));
    assert!(has_settings_picker(r.writer()));

    // First ESC closes picker
    press(&mut r, Esc).await?;
    assert!(has_settings_menu(r.writer()));
    assert!(!has_settings_picker(r.writer()));

    // Second ESC closes menu
    press(&mut r, Esc).await?;
    assert!(!has_settings_menu(r.writer()));
    Ok(())
}

#[tokio::test]
async fn test_settings_menu_arrow_navigation_single_entry() -> TestResult {
    let mut r = open_settings(&make_settings_options(), (80, 24)).await?;
    assert!(has_settings_menu(r.writer()));

    press(&mut r, Enter).await?;
    assert_buffer_contains(r.writer(), "Model search");

    press(&mut r, Esc).await?;
    press(&mut r, Down).await?;
    press(&mut r, Enter).await?;
    assert_buffer_contains(r.writer(), "Theme search");

    press(&mut r, Esc).await?;
    press(&mut r, Down).await?;
    press(&mut r, Enter).await?;
    assert_buffer_contains(r.writer(), "no MCP servers configured");

    press(&mut r, Esc).await?;
    press(&mut r, Down).await?;
    press(&mut r, Enter).await?;
    assert_buffer_contains(r.writer(), "Model search");
    Ok(())
}

#[tokio::test]
async fn test_settings_single_option_shows_model_picker() -> TestResult {
    let mut r = open_settings(&make_settings_options(), (80, 24)).await?;
    press(&mut r, Enter).await?;
    assert!(has_settings_picker(r.writer()));
    assert_buffer_contains(r.writer(), "Model search");
    Ok(())
}

#[tokio::test]
async fn test_settings_picker_focuses_cursor_on_overlay_query() -> TestResult {
    let mut r = open_settings(&make_settings_options(), (80, 24)).await?;
    press(&mut r, Enter).await?;

    let lines = r.writer().get_lines();
    #[allow(clippy::cast_possible_truncation)]
    let search_row = lines.iter().position(|l| l.contains("Model search:")).expect("search row") as u16;
    let (cursor_col, cursor_row) = r.writer().cursor_position();
    assert_eq!(cursor_row, search_row);
    assert_eq!(cursor_col, 18);
    Ok(())
}

#[tokio::test]
async fn test_settings_picker_filters_model_options() -> TestResult {
    let mut r = open_settings(&make_settings_options(), (80, 24)).await?;
    press(&mut r, Enter).await?;
    type_string(&mut r, "claude").await?;
    assert_buffer_contains(r.writer(), "Claude Sonnet");
    Ok(())
}

#[tokio::test]
async fn test_settings_menu_swallows_other_keys() -> TestResult {
    let config = vec![
        acp::SessionConfigOption::select("model", "Model", "m1", vec![acp::SessionConfigSelectOption::new("m1", "M1")]),
        acp::SessionConfigOption::select(
            "theme",
            "Theme",
            "dark",
            vec![acp::SessionConfigSelectOption::new("dark", "Dark")],
        ),
    ];
    let mut r = open_settings(&config, (80, 24)).await?;
    press(&mut r, KeyCode::Char('z')).await?;
    assert!(has_settings_menu(r.writer()));
    assert_buffer_not_contains(r.writer(), "z");
    Ok(())
}

#[tokio::test]
async fn test_settings_menu_ctrl_c_exits() -> TestResult {
    let mut r = open_settings(&make_settings_options(), (80, 24)).await?;
    let action = r
        .on_key_event(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        })
        .await?;
    assert!(matches!(action, LoopAction::Continue), "first Ctrl-C should not exit");
    let action = r
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

#[tokio::test]
async fn test_settings_menu_updates_on_config_option_event() -> TestResult {
    let mut r = open_settings(&make_settings_options(), (80, 24)).await?;
    let new_config = vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "anthropic:claude-sonnet-4-5",
            vec![
                acp::SessionConfigSelectOption::new("openrouter:openai/gpt-4o", "OpenRouter / GPT-4o"),
                acp::SessionConfigSelectOption::new("anthropic:claude-sonnet-4-5", "Anthropic / Claude Sonnet 4.5"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ];
    r.on_session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(new_config)))?;
    assert_buffer_contains(r.writer(), "Claude Sonnet");
    Ok(())
}

#[tokio::test]
async fn test_settings_clears_input_buffer() -> TestResult {
    let r = open_settings(&make_settings_options(), (80, 24)).await?;
    assert_buffer_not_contains(r.writer(), "/settings");
    Ok(())
}

#[tokio::test]
async fn test_settings_with_no_options_shows_placeholder() -> TestResult {
    let r = open_settings(&[], (80, 24)).await?;
    assert!(has_settings_menu(r.writer()));
    assert_buffer_contains(r.writer(), "MCP Servers");
    Ok(())
}

#[tokio::test]
async fn test_settings_overlay_renders_after_large_overflow_scrollback() -> TestResult {
    let config_options = make_settings_options();
    let mut r = RendererTest::new().size((80, 8)).config_options(&config_options).build()?;

    for i in 0..50 {
        r.on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(format!("Line {i:02} with enough content to wrap in 40 cols")),
        ))))?;
    }

    type_string(&mut r, "/settings").await?;
    press(&mut r, Enter).await?;
    assert!(has_settings_menu(r.writer()));
    assert_buffer_contains(r.writer(), "Configuration");
    assert_buffer_contains(r.writer(), "Model");
    Ok(())
}

#[tokio::test]
async fn test_settings_overlay_open_close_after_overflow_keeps_prompt_and_layout_valid() -> TestResult {
    let config_options = make_settings_options();
    let mut r = RendererTest::new().size((80, 8)).config_options(&config_options).build()?;

    for i in 0..50 {
        r.on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(format!("Line {i:02} with enough content to wrap in 40 cols")),
        ))))?;
    }

    type_string(&mut r, "/settings").await?;
    press(&mut r, Enter).await?;
    assert!(has_settings_menu(r.writer()));
    assert_buffer_contains(r.writer(), "Configuration");

    press(&mut r, Esc).await?;
    assert!(!has_settings_menu(r.writer()));

    let lines = r.writer().get_lines();
    assert!(lines.iter().any(|l| l.chars().all(|c| c == '─') && !l.is_empty()), "Prompt rule should be visible");
    assert!(lines.iter().any(|l| !l.trim().is_empty()), "Frame should not be empty");
    Ok(())
}

#[tokio::test]
async fn test_settings_option_update_refreshes_mode_display() -> TestResult {
    let initial = vec![
        acp::SessionConfigOption::select(
            "mode",
            "Mode",
            "planner",
            vec![
                acp::SessionConfigSelectOption::new("planner", "Planner"),
                acp::SessionConfigSelectOption::new("coder", "Coder"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Mode),
    ];
    let mut r = RendererTest::new().config_options(&initial).build()?;

    let updated = vec![
        acp::SessionConfigOption::select(
            "mode",
            "Mode",
            "coder",
            vec![
                acp::SessionConfigSelectOption::new("planner", "Planner"),
                acp::SessionConfigSelectOption::new("coder", "Coder"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Mode),
    ];
    r.on_session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(updated)))?;
    assert_buffer_contains(r.writer(), "Coder");
    Ok(())
}

#[tokio::test]
async fn test_server_status_notification_updates_overlay_state() -> TestResult {
    let mut r = open_settings(&[], (TEST_WIDTH, 40)).await?;
    let notification = acp_utils::notifications::McpNotification::ServerStatus {
        servers: vec![acp_utils::notifications::McpServerStatusEntry::new(
            "docs",
            acp_utils::notifications::McpServerStatus::Connected { tool_count: 0 },
        )],
    };
    r.on_mcp_notification(notification)?;
    assert!(has_settings_menu(r.writer()));
    Ok(())
}

#[tokio::test]
async fn test_empty_server_status_notification_clears_open_settings_menu_summary() -> TestResult {
    let mut r = open_settings(&[], (TEST_WIDTH, 40)).await?;
    r.on_mcp_notification(acp_utils::notifications::McpNotification::ServerStatus {
        servers: vec![acp_utils::notifications::McpServerStatusEntry::new(
            "docs",
            acp_utils::notifications::McpServerStatus::Connected { tool_count: 0 },
        )],
    })?;
    assert_buffer_contains(r.writer(), "MCP Servers: 1 connected");

    r.on_mcp_notification(acp_utils::notifications::McpNotification::ServerStatus { servers: vec![] })?;

    assert_buffer_contains(r.writer(), "MCP Servers: none");
    assert_buffer_not_contains(r.writer(), "MCP Servers: 1 connected");
    Ok(())
}
