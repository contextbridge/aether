use super::support::*;
use utils::ReasoningEffort;

fn server_status_entry(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status)
}

fn oauth_server(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_auth_capability(McpServerAuthCapability::OAuth)
}

fn mcp_notification(servers: Vec<McpServerStatusEntry>) -> AcpEvent {
    AcpEvent::McpNotification(McpNotification::ServerStatus { servers })
}

fn auth_complete(method_id: &str) -> CommandResult {
    CommandResult::AuthenticationCompleted { method_id: method_id.to_string() }
}

fn auth_failed(method_id: &str) -> CommandResult {
    CommandResult::AuthenticationFailed { method_id: method_id.to_string() }
}

fn auth_method(id: &str, name: &str, description: Option<&str>) -> acp::AuthMethod {
    let mut agent = acp::AuthMethodAgent::new(id.to_string(), name);
    if let Some(desc) = description {
        agent = agent.description(desc);
    }
    acp::AuthMethod::Agent(agent)
}

fn auth_methods_updated(methods: Vec<acp::AuthMethod>) -> AcpEvent {
    AcpEvent::AuthMethodsUpdated(AuthMethodsUpdatedParams { auth_methods: methods })
}

/// Walks settings down to the MCP servers pane and authenticates the selected
/// server: the flow that makes the agent hand back an OAuth URL.
fn authenticate_selected_oauth_server(ui: &mut TestUi) {
    open_first_config_option(ui);
    ui.key(key(KeyCode::Enter));
}

#[test]
fn settings_modal_header_and_footer_match_the_content_padding() -> Result<(), Box<dyn std::error::Error>> {
    let mut ui = TestUi::new();
    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.draw();

    let buffer = ui.viewport();
    let background = Theme::default().background;
    let modal_rows: Vec<u16> = (0..buffer.area.height)
        .filter(|&y| (0..buffer.area.width).any(|x| buffer.cell((x, y)).is_some_and(|cell| cell.bg == background)))
        .collect();
    let modal_top = modal_rows.first().copied().ok_or("the settings modal should paint its background")?;
    let modal_bottom = modal_rows.last().copied().ok_or("the settings modal should paint its background")?;
    let header_row = row_containing(&buffer, "Configuration").ok_or("settings header")?;
    let first_content_row = row_containing(&buffer, "Theme:").ok_or("first settings row")?;
    let footer_row = row_containing(&buffer, "Esc close").ok_or("settings footer")?;

    assert_eq!(header_row, modal_top + 1);
    assert_eq!(first_content_row, header_row + 2);
    assert_eq!(footer_row, modal_bottom - 1);
    Ok(())
}

#[test]
fn server_status_unhealthy_count_updates_status_line() {
    let mut ui = TestUi::new();
    assert_eq!(ui.app().status_line_model().unhealthy_servers, 0);

    ui.acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        server_status_entry("linear", McpServerStatus::NeedsOAuth),
        server_status_entry("slack", McpServerStatus::Failed { error: "timeout".to_string() }),
    ]));

    assert_eq!(ui.app().status_line_model().unhealthy_servers, 2);
}

#[test]
fn server_status_all_connected_gives_zero_unhealthy() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![
        server_status_entry("a", McpServerStatus::Connected { tool_count: 1 }),
        server_status_entry("b", McpServerStatus::Connected { tool_count: 2 }),
    ]));

    assert_eq!(ui.app().status_line_model().unhealthy_servers, 0);
}

#[test]
fn server_status_empty_clears_count() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![server_status_entry("x", McpServerStatus::Failed { error: "e".to_string() })]));
    assert_eq!(ui.app().status_line_model().unhealthy_servers, 1);

    ui.acp_event(mcp_notification(vec![]));
    assert_eq!(ui.app().status_line_model().unhealthy_servers, 0);
}

#[test]
fn settings_overlay_shows_mcp_servers_entry() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![server_status_entry("github", McpServerStatus::Connected { tool_count: 3 })]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(viewport.contains("MCP Servers"), "settings should show MCP Servers entry:\n{viewport}");
}

#[test]
fn double_ctrl_c_exits_over_settings_overlay() {
    let mut ui = TestUi::new();
    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());
    assert_ctrl_c_exits(&mut ui);
}

#[test]
fn settings_overlay_shows_provider_logins_when_auth_methods_present() {
    let mut ui = TestUiBuilder::new().auth_methods(vec![auth_method("codex", "Codex", None)]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(viewport.contains("Provider Logins"), "settings should show Provider Logins:\n{viewport}");
}

#[test]
fn settings_overlay_no_provider_logins_when_auth_methods_empty() {
    let mut ui = TestUi::new();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(!viewport.contains("Provider Logins"), "should not show Provider Logins when empty:\n{viewport}");
}

#[test]
fn mcp_server_status_pane_renders_entries() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        oauth_server("linear", McpServerStatus::NeedsOAuth),
        server_status_entry("slack", McpServerStatus::Failed { error: "timeout".to_string() }),
    ]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    // Navigate to MCP Servers entry (after Theme entry)
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(viewport.contains("github"), "should show github:\n{viewport}");
    assert!(viewport.contains("5 tools"), "should show tool count:\n{viewport}");
    assert!(viewport.contains("linear"), "should show linear:\n{viewport}");
    assert!(viewport.contains("needs auth"), "should show auth needed:\n{viewport}");
    assert!(viewport.contains("slack"), "should show slack:\n{viewport}");
    assert!(viewport.contains("timeout"), "should show error:\n{viewport}");
}

#[test]
fn mcp_server_status_empty_shows_placeholder() {
    let mut ui = TestUi::new();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(viewport.contains("no MCP servers configured"), "should show placeholder:\n{viewport}");
}

#[test]
fn selecting_oauth_server_emits_authenticate_mcp_server() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![oauth_server("linear", McpServerStatus::NeedsOAuth)]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    let cmd = ui.next_agent_command().unwrap();
    match cmd {
        AgentCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }
}

#[test]
fn selecting_non_oauth_server_is_noop() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![server_status_entry("github", McpServerStatus::Connected { tool_count: 5 })]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    assert_no_commands(&mut ui, "should not emit command for non-OAuth server");
}

#[test]
fn provider_login_emits_authenticate() {
    let mut ui = TestUiBuilder::new().auth_methods(vec![auth_method("codex", "Codex", None)]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    // Navigate to Provider Logins (menu: Theme, MCP Servers, Provider Logins)
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    let cmd = ui.next_agent_command().unwrap();
    match cmd {
        AgentCommand::Authenticate { method_id } => assert_eq!(method_id, "codex"),
        other => panic!("expected Authenticate, got: {other:?}"),
    }
}

#[test]
fn authenticate_complete_updates_correct_entry() {
    let mut ui =
        TestUiBuilder::new().auth_methods(vec![auth_method("a", "A", None), auth_method("b", "B", None)]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    ui.deliver_result(auth_complete("a"));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("logged in"), "should show logged in for 'a':\n{viewport}");
}

#[test]
fn authenticate_failed_resets_to_needs_login() {
    let mut ui = TestUiBuilder::new().auth_methods(vec![auth_method("x", "X", None)]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    ui.deliver_result(auth_failed("x"));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("needs login"), "should show needs login after failure:\n{viewport}");
}

#[test]
fn auth_methods_updated_replaces_provider_entries() {
    let mut ui = TestUiBuilder::new().auth_methods(vec![auth_method("old", "Old", None)]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.acp_event(auth_methods_updated(vec![auth_method("new", "New", None)]));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("New"), "should show new provider:\n{viewport}");
    assert!(!viewport.contains("Old"), "should not show old provider:\n{viewport}");
}

#[test]
fn esc_on_settings_elicitation_cancels_it_and_keeps_the_overlay_open() {
    block_on_local(async {
        let mut ui = TestUi::new();

        ui.type_text("/settings");
        ui.key(key(KeyCode::Tab));
        assert!(ui.app().has_modal());

        let response_rx = with_elicitation(&mut ui, url_elicitation("test", "https://example.com", "el-1")).await;

        ui.key(key(KeyCode::Esc));

        let response = response_rx.await.unwrap();
        assert!(matches!(&response.action, ElicitationAction::Cancel));
        assert!(ui.app().has_modal(), "cancelling the request should leave the settings overlay open");

        ui.key(key(KeyCode::Esc));
        assert!(!ui.app().has_modal(), "the next Esc should close the settings overlay");
    });
}

#[test]
fn oauth_url_prompt_is_shown_in_settings_overlay() {
    block_on_local(async {
        let mut ui = TestUiBuilder::new().dimensions(80, 30).build();
        ui.acp_event(mcp_notification(vec![oauth_server("linear", McpServerStatus::NeedsOAuth)]));
        authenticate_selected_oauth_server(&mut ui);

        with_elicitation(&mut ui, url_elicitation("linear", "https://linear.app/oauth", "aether-oauth")).await;

        ui.draw();
        let viewport = ui.viewport_text();

        assert!(viewport.contains("linear.app"), "should show the host being authorized:\n{viewport}");
        assert!(viewport.contains("open browser"), "should offer to open the browser:\n{viewport}");
        assert!(viewport.contains("linear"), "the pane that started the request should stay visible:\n{viewport}");
    });
}

#[test]
fn enter_on_settings_oauth_prompt_opens_the_browser() {
    block_on_local(async {
        let opened: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
        let mut overlay = SettingsOverlay::new(&[], vec![], &[]);

        let (cx, mut peer) = test_connection().await;
        let (responder, _response_rx) = peer.fake_elicitation(&cx).await;

        overlay.on_elicitation_request(
            url_elicitation("linear", "https://linear.app/oauth", "aether-oauth"),
            responder,
            {
                let opened = opened.clone();
                Arc::new(move |url: &str| {
                    *opened.lock().unwrap() = Some(url.to_string());
                    Ok(())
                })
            },
            Arc::new(|_| Ok(())),
        );

        overlay.on_ui_event(UiEvent::Key(key(KeyCode::Enter)));

        assert_eq!(opened.lock().unwrap().as_deref(), Some("https://linear.app/oauth"));
    });
}

#[test]
fn url_elicitation_enter_accepts_and_clears_settings_prompt() {
    block_on_local(async {
        let mut ui = TestUiBuilder::new().dimensions(80, 30).build();
        ui.acp_event(mcp_notification(vec![oauth_server("linear", McpServerStatus::Authenticating)]));
        authenticate_selected_oauth_server(&mut ui);

        let response_rx =
            with_elicitation(&mut ui, url_elicitation("linear", "https://linear.app/oauth", "aether-oauth")).await;

        ui.key(key(KeyCode::Enter));

        // The harness records URL opens instead of handing them to the host
        // browser, so the assertion is on what Enter asked to open.
        assert_eq!(ui.opened_urls(), vec!["https://linear.app/oauth"], "Enter should open the OAuth URL");

        let response = response_rx.await.unwrap();
        assert!(matches!(&response.action, ElicitationAction::Accept(_)));

        ui.draw();
        let viewport = ui.viewport_text();
        assert!(viewport.contains("Configuration"), "the settings overlay should stay open:\n{viewport}");
        assert!(!viewport.contains("open browser"), "the finished prompt should be gone:\n{viewport}");
    });
}

#[test]
fn form_elicitation_is_answered_inside_the_settings_overlay() {
    block_on_local(async {
        let mut ui = TestUiBuilder::new().dimensions(80, 30).build();
        ui.type_text("/settings");
        ui.key(key(KeyCode::Tab));

        let response_rx = with_elicitation(
            &mut ui,
            form_elicitation(
                "linear",
                "Paste an API token",
                ElicitationSchema::new().property("token", StringPropertySchema::new(), true),
            ),
        )
        .await;

        ui.draw();
        let viewport = ui.viewport_text();
        assert!(viewport.contains("Paste an API token"), "the request should be visible:\n{viewport}");

        ui.type_text("secret");
        ui.key(key(KeyCode::Enter));

        let response = response_rx.await.unwrap();
        assert_eq!(accepted_content(&response)["token"], "secret");
        assert!(ui.app().has_modal(), "answering the request should leave the settings overlay open");
    });
}

#[test]
fn server_status_updated_while_pane_open_refreshes() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![server_status_entry("a", McpServerStatus::Connected { tool_count: 1 })]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.acp_event(mcp_notification(vec![server_status_entry(
        "a",
        McpServerStatus::Failed { error: "crash".to_string() },
    )]));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("crash"), "should show updated status:\n{viewport}");
}

#[test]
fn provider_login_pane_shows_all_statuses() {
    let mut ui = TestUiBuilder::new()
        .auth_methods(vec![
            auth_method("needs", "NeedsLogin", None),
            auth_method("authd", "Authed", Some("authenticated")),
        ])
        .build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(viewport.contains("NeedsLogin"), "should show NeedsLogin:\n{viewport}");
    assert!(viewport.contains("needs login"), "should show needs login status:\n{viewport}");
    assert!(viewport.contains("Authed"), "should show Authed:\n{viewport}");
    assert!(viewport.contains("logged in"), "should show logged in status:\n{viewport}");
}

#[test]
fn esc_from_server_status_returns_to_menu() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![server_status_entry("a", McpServerStatus::Connected { tool_count: 1 })]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Esc));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("MCP Servers"), "should be back at menu:\n{viewport}");
}

#[test]
fn esc_from_provider_login_returns_to_menu() {
    let mut ui = TestUiBuilder::new().auth_methods(vec![auth_method("x", "X", None)]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Esc));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Provider Logins"), "should be back at menu:\n{viewport}");
}

#[test]
fn server_status_summary_updates_in_menu() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![
        server_status_entry("a", McpServerStatus::Connected { tool_count: 1 }),
        server_status_entry("b", McpServerStatus::Failed { error: "err".to_string() }),
    ]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("1 connected") || viewport.contains("1 failed"), "should show summary:\n{viewport}");
}

fn deferred_server_entry(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_deferred_tools(true)
}

fn deferred_oauth_server(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status)
        .with_deferred_tools(true)
        .with_auth_capability(McpServerAuthCapability::OAuth)
}

#[test]
fn server_status_pane_groups_model_visible_and_deferred_with_headers() {
    let mut ui = TestUiBuilder::new().dimensions(40, 17).build();

    ui.acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        deferred_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        deferred_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(viewport.contains("Model-visible"), "should show Model-visible header:\n{viewport}");
    assert!(viewport.contains("Deferred"), "should show Deferred header:\n{viewport}");
    assert!(viewport.contains("github"), "should show github:\n{viewport}");
    assert!(viewport.contains("math"), "should show math:\n{viewport}");
    assert!(viewport.contains("linear"), "should show linear:\n{viewport}");
}

#[test]
fn server_status_pane_only_model_visible_renders_no_headers() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        server_status_entry("slack", McpServerStatus::Failed { error: "err".to_string() }),
    ]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(
        !viewport.contains("Model-visible"),
        "should not show Model-visible header when all model-visible:\n{viewport}"
    );
    assert!(!viewport.contains("Deferred"), "should not show Deferred header when no deferred:\n{viewport}");
}

#[test]
fn server_status_pane_only_deferred_shows_deferred_header() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![
        deferred_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        deferred_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();

    assert!(!viewport.contains("Model-visible"), "should not show Model-visible header when all deferred:\n{viewport}");
    assert!(viewport.contains("Deferred"), "should show Deferred header:\n{viewport}");
}

#[test]
fn server_status_navigation_skips_headers_and_spacers() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        deferred_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        deferred_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    // First server: github (Model-visible section, row index 1)
    ui.key(key(KeyCode::Enter));
    assert_no_commands(&mut ui, "github is not OAuth, should be noop");

    // Move down once: should land on math (Deferred section, row index 4 - skipping Model-visible header)
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    assert_no_commands(&mut ui, "math is not OAuth, should be noop");

    // Move down once: should land on linear (Deferred section, row index 5)
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    let cmd = ui.next_agent_command().unwrap();
    match cmd {
        AgentCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }

    // Move up: should go back to math, not land on Deferred header or Spacer
    ui.key(key(KeyCode::Up));
    ui.key(key(KeyCode::Enter));
    assert_no_commands(&mut ui, "math is not OAuth after wrap-up");

    // Move up again: should land back on github
    ui.key(key(KeyCode::Up));
    ui.key(key(KeyCode::Enter));
    assert_no_commands(&mut ui, "github is not OAuth after wrap-up");
}

#[test]
fn deferred_oauth_server_sends_original_server_name() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![deferred_oauth_server("linear", McpServerStatus::NeedsOAuth)]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    let cmd = ui.next_agent_command().unwrap();
    match cmd {
        AgentCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }
}

#[test]
fn connection_closed_cancels_settings_elicitation() {
    block_on_local(async {
        let mut ui = TestUi::new();

        ui.type_text("/settings");
        ui.key(key(KeyCode::Tab));
        assert!(ui.app().has_modal());

        let response_rx =
            with_elicitation(&mut ui, url_elicitation("test", "https://example.com", "el-conn-closed")).await;

        ui.acp_event(AcpEvent::ConnectionClosed);

        let response = response_rx.await.unwrap();
        assert!(matches!(&response.action, ElicitationAction::Cancel));
    });
}

#[test]
fn new_session_created_cancels_settings_elicitation() {
    block_on_local(async {
        let mut ui = TestUi::new();

        ui.type_text("/settings");
        ui.key(key(KeyCode::Tab));
        assert!(ui.app().has_modal());

        let response_rx =
            with_elicitation(&mut ui, url_elicitation("test", "https://example.com", "el-new-session")).await;

        ui.deliver_result(new_session_created("new", vec![]));

        let response = response_rx.await.unwrap();
        assert!(matches!(&response.action, ElicitationAction::Cancel));
    });
}

#[test]
fn server_status_update_entries_preserves_selection_across_group_boundaries() {
    let mut ui = TestUi::new();

    ui.acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        deferred_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    // Navigate to linear (Deferred section, after header + spacer)
    ui.key(key(KeyCode::Down));

    // Send update that flips the grouping - github becomes deferred too, linear stays OAuth
    ui.acp_event(mcp_notification(vec![
        deferred_oauth_server("linear", McpServerStatus::NeedsOAuth),
        deferred_server_entry("github", McpServerStatus::Connected { tool_count: 5 }),
    ]));

    ui.key(key(KeyCode::Enter));
    let cmd = ui.next_agent_command().unwrap();
    match cmd {
        AgentCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer for linear, got: {other:?}"),
    }
}

#[test]
fn settings_builtin_opens_overlay_and_clears_composer() {
    let mut ui = TestUi::new();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));

    // Composer should be cleared
    assert!(ui.app().composer().text().is_empty());
    // Should not emit a Prompt
    assert_no_commands(&mut ui, "settings open should not emit an action command");
    // Overlay should be open (has_modal returns true)
    assert!(ui.app().has_modal());
}

#[test]
fn settings_builtin_is_listed_in_command_picker() {
    let mut ui = TestUi::new();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("/settings"), "built-in /settings should be in command picker:\n{viewport}");
}

#[test]
fn settings_esc_closes_overlay() {
    let mut ui = TestUi::new();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    ui.key(key(KeyCode::Esc));
    assert!(!ui.app().has_modal());
}

#[test]
fn settings_overlay_renders_on_terminal() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![select_option("model", "gpt-4o"), select_option("mode", "code")])
        .build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(
        viewport.contains("model") || viewport.contains("gpt-4o"),
        "settings overlay should show model entry:\n{viewport}"
    );
    assert!(
        viewport.contains("mode") || viewport.contains("code"),
        "settings overlay should show mode entry:\n{viewport}"
    );
    assert!(viewport.contains("Configuration"), "settings should use the shared modal title:\n{viewport}");
    assert!(viewport.contains("Enter select"), "settings should use the modal footer:\n{viewport}");
}

#[test]
fn settings_over_renders_with_no_config_options() {
    let mut ui = TestUi::new();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(
        viewport.contains("no settings options") || viewport.contains("Configuration"),
        "empty settings should show placeholder:\n{viewport}"
    );
}

#[test]
fn settings_overlay_render_clears_only_the_modal_rectangle() {
    let options = vec![LocalConfigOption::from_acp(select_option("model", "gpt-4o"))];
    let mut overlay = SettingsOverlay::new(&options, Vec::new(), &[]);
    let area = ratatui::layout::Rect::new(0, 0, 40, 15);
    let mut buffer = Buffer::filled(area, Cell::new("X"));
    let theme = Theme::default();
    let mut highlighter = SyntaxHighlighter::new();
    let mut cx = DrawContext { theme: &theme, highlighter: &mut highlighter, theme_generation: Generation::default() };

    overlay.render(area, &mut buffer, &mut cx);

    let modal = area.centered(ratatui::layout::Constraint::Percentage(80), ratatui::layout::Constraint::Percentage(80));
    assert_eq!(buffer.cell((area.x, area.y)).unwrap().symbol(), "X");
    assert!(
        (modal.top()..modal.bottom()).all(|y| {
            (modal.left()..modal.right()).all(|x| buffer.cell((x, y)).is_some_and(|cell| cell.symbol() != "X"))
        }),
        "the modal rectangle should be cleared:\n{}",
        buffer_text(&buffer)
    );
}

#[test]
fn settings_overlay_uses_borderless_modal_chrome_and_padded_highlights() {
    let options = vec![LocalConfigOption::from_acp(select_option("model", "gpt-4o"))];
    let mut overlay = SettingsOverlay::new(&options, Vec::new(), &[]);
    let area = ratatui::layout::Rect::new(0, 0, 40, 15);
    let mut buffer = Buffer::filled(area, Cell::new("X"));
    let theme = Theme::default();
    let mut highlighter = SyntaxHighlighter::new();
    let mut cx = DrawContext { theme: &theme, highlighter: &mut highlighter, theme_generation: Generation::default() };

    overlay.render(area, &mut buffer, &mut cx);

    let modal = area.centered(ratatui::layout::Constraint::Percentage(80), ratatui::layout::Constraint::Percentage(80));
    assert!(
        (modal.top()..modal.bottom()).all(|y| {
            (modal.left()..modal.right()).all(|x| {
                buffer.cell((x, y)).is_some_and(|cell| !matches!(cell.symbol(), "╭" | "╮" | "╰" | "╯" | "─" | "│"))
            })
        }),
        "modal should not render an outer border:\n{}",
        buffer_text(&buffer)
    );
    assert_eq!(buffer.cell((modal.right(), modal.y + 1)).unwrap().symbol(), "X");
    let title_column = (modal.x..modal.right())
        .find(|&x| buffer.cell((x, modal.y + 1)).is_some_and(|cell| cell.symbol() == "C"))
        .expect("the modal title should be rendered");
    assert_eq!(title_column, modal.x + 2);
    let footer_column = (modal.x..modal.right())
        .find(|&x| buffer.cell((x, modal.bottom() - 2)).is_some_and(|cell| cell.symbol() == "E"))
        .expect("the modal footer should be rendered");
    assert_eq!(footer_column, modal.x + 2);

    let highlighted_row = (modal.top()..modal.bottom())
        .find(|&y| buffer.cell((modal.x + 2, y)).is_some_and(|cell| cell.bg == theme.text_primary))
        .expect("the selected settings row should be highlighted");
    assert_eq!(buffer.cell((modal.x, highlighted_row)).unwrap().bg, theme.text_primary);
    assert_eq!(buffer.cell((modal.x + 1, highlighted_row)).unwrap().bg, theme.text_primary);
}

#[test]
fn settings_overlay_clears_conversation_content_behind_it() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();
    ui.submit("CHAT_CONTENT_MUST_NOT_SHOW_THROUGH_SETTINGS");
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.draw();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("Configuration"), "settings overlay should render:\n{viewport}");
    assert!(
        !viewport.contains("CHAT_CONTENT_MUST_NOT_SHOW_THROUGH_SETTINGS"),
        "conversation content must be cleared behind settings:\n{viewport}"
    );
}

#[test]
fn settings_overlay_still_valid_after_scrollback() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![select_option("model", "gpt-4o"), select_option("mode", "code")])
        .build();

    // Fill transcript with content to push scrollback
    for i in 0..30 {
        ui.acp_event(text_chunk(&format!("line {i}")));
    }
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.draw();
    ui.draw();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Configuration"), "settings overlay should render after scrollback:\n{viewport}");
}

#[test]
fn settings_overlay_renders_at_narrow_width() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).dimensions(30, 15).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(
        viewport.contains("model") || viewport.contains("gpt-4o"),
        "settings should render at 30 cols:\n{viewport}"
    );
}

#[test]
fn settings_overlay_renders_at_short_height() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![select_option("model", "gpt-4o"), select_option("mode", "code")])
        .build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));

    ui.resize(40, 8);
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(
        viewport.contains("model") || viewport.contains("too small"),
        "settings should handle short terminal:\n{viewport}"
    );
}

#[test]
fn settings_selecting_option_emits_config_option() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![acp::SessionConfigOption::select(
            "model",
            "Model",
            "gpt-4o",
            vec![
                acp::SessionConfigSelectOption::new("gpt-4o", "GPT-4o"),
                acp::SessionConfigSelectOption::new("claude", "Claude"),
            ],
        )])
        .build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    // Down past the Theme entry, Enter to open picker, Down to second option, Enter to confirm
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    // Should have emitted a set_config_option command
    let cmd = ui.next_agent_command().expect("expected set_config_option");
    match cmd {
        AgentCommand::SetConfigOption { config_id, value, .. } => {
            assert_eq!(config_id, "model");
            assert_eq!(value, "claude");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn settings_multi_select_opens_model_selector() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![{
            let mut opt = acp::SessionConfigOption::select(
                "model",
                "Model",
                "",
                vec![
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Anthropic / Sonnet"),
                ],
            );
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            opt = opt.meta(Some(meta));
            opt
        }])
        .build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    // Down past the Theme entry, then Enter should open model selector since multi_select is true
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Model search"), "should show model selector:\n{viewport}");
}

#[test]
fn settings_multi_select_toggle_and_confirm() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![{
            let mut opt = acp::SessionConfigOption::select(
                "model",
                "Model",
                "",
                vec![
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Anthropic / Sonnet"),
                ],
            );
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            opt = opt.meta(Some(meta));
            opt
        }])
        .build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Esc));

    let cmd = ui.next_agent_command().expect("expected set_config_option");
    match cmd {
        AgentCommand::SetConfigOption { config_id, value, .. } => {
            assert_eq!(config_id, "model");
            assert!(value.contains("anthropic:opus"), "value: {value}");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn model_selector_space_types_into_search_instead_of_toggling() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![multi_select_option(
            "model",
            "Model",
            "",
            &[("anthropic:opus", "Opus"), ("anthropic:sonnet", "Sonnet")],
        )])
        .build();
    open_first_config_option(&mut ui);

    ui.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    ui.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::ALT));
    ui.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SUPER));

    ui.key(key(KeyCode::Char(' ')));
    let text = overlay_text(&mut ui);
    assert!(text.contains("Model search:  "), "space should enter the search query:\n{text}");
    assert!(!text.contains("[x]"), "space must not toggle a model:\n{text}");

    ui.key(key(KeyCode::Esc));
    assert_no_commands(&mut ui, "space only filtered the list, so closing commits nothing");
}

#[test]
fn config_option_update_refreshes_settings_overlay() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    ui.acp_event(session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![
        select_option("model", "sonnet"),
        select_option("mode", "code"),
    ]))));

    // Overlay should still be open with updated options
    assert!(ui.app().has_modal());
    let options = ui.app().config_options();
    assert_eq!(options.len(), 2);
}

#[test]
fn config_option_update_failed_shows_in_transcript() {
    let mut ui = TestUi::new();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    ui.deliver_result(CommandResult::ConfigOptionUpdateFailed { error: "invalid model".to_string() });

    // Overlay should still be open
    assert!(ui.app().has_modal());

    // Error should be in transcript
    let messages: Vec<_> = message_texts(&ui).collect();
    let has_error = messages.iter().any(|message| message.contains("invalid model"));
    assert!(has_error, "expected transcript error, got {messages:?}");
}

#[test]
fn connection_closed_clears_settings_overlay() {
    let mut ui = TestUi::new();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    ui.acp_event(AcpEvent::ConnectionClosed);
    assert!(!ui.app().has_modal());
    assert!(ui.app().exit_requested());
}

#[test]
fn new_session_clears_settings_overlay() {
    let mut ui = TestUi::new();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    // New session created should close settings overlay
    ui.deliver_result(new_session_created("new-id", Vec::new()));
    assert!(!ui.app().has_modal());

    // Should have consumed the new session event
    let _ = ui.next_command();
}

#[test]
fn settings_composer_capture_prevents_normal_input() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    let composer_text_before = ui.app().composer().text().to_string();

    // Typing while settings overlay is open should not modify composer
    for c in "hello".chars() {
        ui.key(key(KeyCode::Char(c)));
    }
    assert_eq!(ui.app().composer().text(), composer_text_before);
}

#[test]
fn settings_theme_entry_is_injected_first() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Theme:"), "Theme entry should render first:\n{viewport}");
}

#[test]
fn settings_theme_picker_opens_and_shows_default() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Default"), "Theme picker should show Default option:\n{viewport}");
    assert!(viewport.contains("Theme"), "Theme picker should have Theme header:\n{viewport}");
}

#[test]
fn settings_theme_selection_returns_to_menu() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Theme: Default"), "Should return to menu with Default selected:\n{viewport}");
}

#[test]
fn settings_theme_empty_file_list_shows_only_default() {
    let mut ui = TestUiBuilder::new().build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Default"), "Should show Default theme option:\n{viewport}");
}

#[test]
fn opening_settings_requests_the_theme_list() {
    let mut ui = TestUiBuilder::new().build();
    open_settings(&mut ui);

    let commands = ui.take_commands();
    assert!(
        commands.iter().any(|command| matches!(command, Command::Filesystem(FilesystemCommand::ListThemes))),
        "opening settings must ask for the theme list"
    );
}

#[test]
fn listed_themes_appear_in_the_theme_picker() {
    let mut ui = TestUiBuilder::new().build();
    open_settings(&mut ui);
    ui.deliver_result(CommandResult::ThemesListed(vec!["dracula.tmTheme".to_string(), "kanagawa.tmTheme".to_string()]));

    ui.key(key(KeyCode::Enter));
    let text = overlay_text(&mut ui);
    assert!(text.contains("dracula"), "theme picker should list dracula:\n{text}");
    assert!(text.contains("kanagawa"), "theme picker should list kanagawa:\n{text}");
}

#[test]
fn settings_menu_has_a_single_theme_row_after_themes_load() {
    let mut ui = TestUiBuilder::new().build();
    open_settings(&mut ui);
    ui.deliver_result(CommandResult::ThemesListed(vec!["dracula.tmTheme".to_string()]));

    let text = overlay_text(&mut ui);
    assert_eq!(text.matches("Theme:").count(), 1, "the theme row must be replaced, not duplicated:\n{text}");
}

#[test]
fn rapid_theme_changes_settle_on_the_newest_choice() {
    let mut ui = TestUiBuilder::new().build();
    open_settings(&mut ui);
    ui.deliver_result(CommandResult::ThemesListed(vec!["first.tmTheme".to_string(), "second.tmTheme".to_string()]));

    select_theme(&mut ui, "first");
    select_theme(&mut ui, "second");
    settle_theme_tasks_newest_first(&mut ui);

    assert_eq!(
        ui.app().ui_settings().theme.file.as_deref(),
        Some("second.tmTheme"),
        "a theme change that finishes late must not undo the one the user made after it"
    );
}

#[test]
fn theme_entry_preserved_after_config_option_update() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    assert!(ui.app().has_modal());

    // ConfigOptionUpdate arrives — Theme entry must still be first
    ui.acp_event(session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![
        select_option("model", "sonnet"),
    ]))));
    assert!(ui.app().has_modal());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Theme:"), "Theme entry must survive ConfigOptionUpdate:\n{viewport}");
}

#[test]
fn theme_selection_keeps_overlay_open_and_refreshes_display() {
    let mut ui = TestUiBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    // Overlay must stay open
    assert!(ui.app().has_modal(), "Overlay must stay open after theme selection");

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Theme: Default"), "Theme should show Default after selection:\n{viewport}");
}

#[test]
fn model_selector_provider_heading_does_not_skip_rows() {
    let mut ui = TestUiBuilder::new()
        .dimensions(40, 20)
        .config_options(vec![{
            let mut opt = acp::SessionConfigOption::select(
                "model",
                "Model",
                "",
                vec![
                    acp::SessionConfigSelectOption::new("openai:gpt-4o", "OpenAI / GPT-4o"),
                    acp::SessionConfigSelectOption::new("openai:gpt-3.5", "OpenAI / GPT-3.5"),
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Anthropic / Sonnet"),
                ],
            );
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            opt = opt.meta(Some(meta));
            opt
        }])
        .build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("OpenAI"), "OpenAI heading should be visible:\n{viewport}");
    assert!(viewport.contains("GPT-4o"), "GPT-4o should be visible:\n{viewport}");
    assert!(viewport.contains("GPT-3.5"), "GPT-3.5 should be visible:\n{viewport}");

    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Down));
    let viewport = overlay_text(&mut ui);
    assert!(viewport.contains("Anthropic"), "Anthropic heading should be visible after scrolling:\n{viewport}");
    assert!(viewport.contains("Opus"), "Opus should be reachable:\n{viewport}");
    assert!(viewport.contains("Sonnet"), "Sonnet should be reachable:\n{viewport}");
}

#[test]
fn model_selector_skips_disabled_models_and_scrolls_to_the_end() {
    let options: Vec<_> = (0..15)
        .map(|index| {
            let option = acp::SessionConfigSelectOption::new(
                format!("provider:model-{index:02}"),
                format!("Provider / Model {index:02}"),
            );
            if index == 5 { option.description("Unavailable: missing credentials") } else { option }
        })
        .collect();
    let mut model_option = acp::SessionConfigOption::select("model", "Model", "", options);
    let mut meta = serde_json::Map::new();
    meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
    model_option = model_option.meta(Some(meta));
    let mut ui = TestUiBuilder::new().config_options(vec![model_option]).build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    for _ in 0..13 {
        ui.key(key(KeyCode::Down));
    }

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Model 14"), "last enabled model should be focused and visible:\n{viewport}");

    ui.key(key(KeyCode::Down));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Model 14"), "selection should remain at the end of the list:\n{viewport}");
}

#[test]
fn model_selector_focused_item_visible_with_provider_headings() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![{
            let mut opt = acp::SessionConfigOption::select(
                "model",
                "Model",
                "",
                vec![
                    acp::SessionConfigSelectOption::new("openai:gpt-4o", "OpenAI / GPT-4o"),
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Anthropic / Sonnet"),
                    acp::SessionConfigSelectOption::new("google:gemini", "Google / Gemini"),
                    acp::SessionConfigSelectOption::new("google:palm", "Google / PaLM"),
                ],
            );
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            opt = opt.meta(Some(meta));
            opt
        }])
        .build();

    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    // Move down through all items — last item should be visible
    for _ in 0..4 {
        ui.key(key(KeyCode::Down));
    }

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("PaLM"), "Focused last item with headings should be visible:\n{viewport}");
}

/// Opens the settings overlay with `options` and returns what the menu draws.
fn settings_menu_text(options: Vec<acp::SessionConfigOption>) -> String {
    let mut ui = TestUiBuilder::new().config_options(options).build();
    open_settings(&mut ui);
    overlay_text(&mut ui)
}

fn open_settings(ui: &mut TestUi) {
    ui.type_text("/settings");
    ui.key(key(KeyCode::Tab));
}

/// Opens settings and activates the first agent config option: the row under
/// the client-side Theme entry the menu always injects ahead of them.
fn open_first_config_option(ui: &mut TestUi) {
    open_settings(ui);
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
}

fn assert_no_commands(ui: &mut TestUi, message: &str) {
    let commands = ui.take_commands();
    assert!(
        commands.iter().all(|command| matches!(command, Command::Filesystem(FilesystemCommand::ListThemes))),
        "{message}: unexpected commands {commands:?}"
    );
}

/// Opens the Theme picker (the first menu entry), filters it down to `name`, and
/// confirms.
fn select_theme(ui: &mut TestUi, name: &str) {
    ui.key(key(KeyCode::Enter));
    ui.type_text(name);
    ui.key(key(KeyCode::Enter));
}

/// Runs the queued theme work, handing each batch of results back newest first —
/// the order two saves racing on the runtime can finish in.
fn settle_theme_tasks_newest_first(ui: &mut TestUi) {
    loop {
        let mut batch = Vec::new();
        for command in ui.take_commands() {
            if let Command::Filesystem(FilesystemCommand::ApplyTheme { settings, .. }) = command {
                batch.push(settings);
            }
        }
        if batch.is_empty() {
            return;
        }
        for settings in batch.into_iter().rev() {
            ui.deliver_result(CommandResult::ThemeApplied { settings, theme: Theme::default(), error: None });
        }
    }
}

/// Draws the open overlay at a generous size and returns what it renders.
fn overlay_text(ui: &mut TestUi) -> String {
    ui.resize(80, 24);
    ui.draw();
    ui.viewport_text()
}

fn multi_select_option(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> acp::SessionConfigOption {
    as_multi_select(select_with_values(id, name, current, values))
}

fn as_multi_select(option: acp::SessionConfigOption) -> acp::SessionConfigOption {
    let mut meta = serde_json::Map::new();
    meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
    option.meta(Some(meta))
}

fn model(value: &str, name: &str, meta: SelectOptionMeta) -> acp::SessionConfigSelectOption {
    acp::SessionConfigSelectOption::new(value.to_string(), name.to_string()).meta(meta.into_meta())
}

fn model_selector_ui(option: acp::SessionConfigOption) -> TestUi {
    let mut ui = TestUiBuilder::new().dimensions(100, 40).config_options(vec![option]).build();
    open_first_config_option(&mut ui);
    ui
}

fn capability_column_for(text: &str, model: &str) -> usize {
    let line = text.lines().find(|line| line.contains(model)).expect("model row");
    line[..line.find("img").expect("capability tag")].chars().count()
}

fn cell_fg(buffer: &Buffer, row: u16, needle: &str) -> Option<Color> {
    let symbols: String =
        (buffer.area.left()..buffer.area.right()).filter_map(|x| buffer.cell((x, row))).map(Cell::symbol).collect();
    let start = buffer.area.left() + u16::try_from(symbols.find(needle)?).ok()?;
    buffer.cell((start, row)).map(|cell| cell.fg)
}

fn is_blank(line: &str) -> bool {
    line.trim_matches(|c: char| c.is_whitespace() || "▲▼█│".contains(c)).is_empty()
}

fn assert_capability_columns_align(text: &str) {
    let columns: Vec<usize> = text
        .lines()
        .filter(|line| line.contains("img"))
        .map(|line| line[..line.find("img").expect("checked")].chars().count())
        .collect();
    assert!(columns.len() >= 2, "expected at least two tagged rows:\n{text}");
    let column = columns[0];
    assert!(columns.iter().all(|&found| found == column), "capability columns should line up across rows:\n{text}");
}

fn select_with_values(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> acp::SessionConfigOption {
    let options: Vec<acp::SessionConfigSelectOption> = values
        .iter()
        .map(|(value, label)| acp::SessionConfigSelectOption::new((*value).to_string(), (*label).to_string()))
        .collect();
    acp::SessionConfigOption::select(id.to_string(), name.to_string(), current.to_string(), options)
}

#[test]
fn settings_menu_lists_each_option_with_its_current_value() {
    let text = settings_menu_text(vec![
        select_with_values("model", "Model", "claude", &[("gpt-4o", "GPT-4o"), ("claude", "Claude")]),
        select_with_values("mode", "Mode", "code", &[("code", "Code"), ("chat", "Chat")]),
    ]);

    assert!(text.contains("Model: Claude"), "menu should show the current model:\n{text}");
    assert!(text.contains("Mode: Code"), "menu should show the current mode:\n{text}");
}

#[test]
fn settings_menu_hides_reasoning_effort() {
    // Reasoning effort is edited from inside the model selector, not as its own row.
    let text = settings_menu_text(vec![
        select_with_values("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
        select_with_values("reasoning_effort", "Reasoning", "high", &[("low", "Low"), ("high", "High")]),
    ]);

    assert!(text.contains("Model"), "menu should still show model:\n{text}");
    assert!(!text.contains("Reasoning"), "reasoning effort should not be its own row:\n{text}");
}

#[test]
fn settings_menu_hides_options_with_no_values() {
    let text = settings_menu_text(vec![
        select_with_values("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
        select_with_values("empty", "Empty", "", &[]),
    ]);

    assert!(text.contains("Model"), "menu should show model:\n{text}");
    assert!(!text.contains("Empty"), "an option offering nothing to pick should be hidden:\n{text}");
}

#[test]
fn settings_menu_shows_every_selected_model_for_a_multi_select() {
    let text = settings_menu_text(vec![multi_select_option(
        "model",
        "Model",
        "anthropic:opus,anthropic:sonnet",
        &[("anthropic:opus", "Opus"), ("anthropic:sonnet", "Sonnet")],
    )]);

    assert!(text.contains("Opus, Sonnet"), "menu should name both selected models:\n{text}");
}

#[test]
fn settings_menu_selection_wraps() {
    let mut ui =
        TestUiBuilder::new().config_options(vec![select_with_values("model", "Model", "a", &[("a", "A")])]).build();
    open_settings(&mut ui);

    // Rows run Theme, Model, MCP Servers. Up from the first wraps to the last,
    // so Enter opens the server pane rather than the theme picker.
    ui.key(key(KeyCode::Up));
    ui.key(key(KeyCode::Enter));

    let text = overlay_text(&mut ui);
    assert!(text.contains("no MCP servers configured"), "Up from the first row should wrap to the last:\n{text}");
}

#[test]
fn settings_overlay_renders_placeholder_when_the_terminal_is_tiny() {
    let mut ui =
        TestUiBuilder::new().config_options(vec![select_with_values("model", "Model", "a", &[("a", "A")])]).build();
    open_settings(&mut ui);

    ui.resize(5, 4);
    ui.draw();
    let text = ui.viewport_text();

    assert!(text.contains("term"), "should explain why nothing is drawn:\n{text}");
}

#[test]
fn settings_picker_filters_by_typed_query() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![select_with_values(
            "model",
            "Model",
            "gpt-4o",
            &[("gpt-4o", "GPT-4o"), ("claude", "Claude"), ("gemini", "Gemini")],
        )])
        .build();
    open_first_config_option(&mut ui);

    ui.type_text("clau");

    let text = overlay_text(&mut ui);
    assert!(text.contains("Claude"), "the match should stay:\n{text}");
    assert!(!text.contains("Gemini"), "non-matches should be filtered out:\n{text}");
}

#[test]
fn settings_picker_shows_unavailable_options_with_their_reason() {
    let unavailable = acp::SessionConfigSelectOption::new("claude", "Claude").description("Unavailable: no API key");
    let option = acp::SessionConfigOption::select(
        "model",
        "Model",
        "gpt-4o",
        vec![acp::SessionConfigSelectOption::new("gpt-4o", "GPT-4o"), unavailable],
    );
    let mut ui = TestUiBuilder::new().config_options(vec![option]).build();
    open_first_config_option(&mut ui);

    let text = overlay_text(&mut ui);
    assert!(text.contains("Claude"), "an unavailable option is still listed:\n{text}");

    // Down skips it, so confirming cannot select it.
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    assert_no_commands(&mut ui, "an unavailable option must not be selectable");
}

#[test]
fn settings_picker_esc_returns_to_the_menu_without_changing_anything() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![select_with_values("model", "Model", "a", &[("a", "A"), ("b", "B")])])
        .build();
    open_first_config_option(&mut ui);
    ui.key(key(KeyCode::Down));

    ui.key(key(KeyCode::Esc));

    assert_no_commands(&mut ui, "backing out must not commit the focused value");
    assert!(ui.app().has_modal(), "Esc should leave the pane, not the overlay");
    let text = overlay_text(&mut ui);
    assert!(text.contains("Model: A"), "the menu should still show the unchanged value:\n{text}");
}

#[test]
fn settings_picker_confirming_the_current_value_sends_nothing() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![select_with_values("model", "Model", "a", &[("a", "A"), ("b", "B")])])
        .build();
    open_first_config_option(&mut ui);

    ui.key(key(KeyCode::Enter));

    assert_no_commands(&mut ui, "re-picking the current value is not a change");
}

#[test]
fn settings_picker_confirm_updates_the_menu_immediately() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![select_with_values("model", "Model", "a", &[("a", "A"), ("b", "B")])])
        .build();
    open_first_config_option(&mut ui);
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));

    let text = overlay_text(&mut ui);
    assert!(text.contains("Model: B"), "the menu should not wait for the agent round-trip:\n{text}");
}

#[test]
fn model_selector_shows_capability_tags_and_provider_stripped_names() {
    let option = acp::SessionConfigOption::select(
        "model",
        "Model",
        "",
        vec![model(
            "anthropic:opus",
            "Anthropic / Opus",
            SelectOptionMeta { supports_image: true, ..Default::default() },
        )],
    );
    let mut ui = model_selector_ui(as_multi_select(option));

    let text = overlay_text(&mut ui);
    assert!(text.contains("Anthropic"), "should show the provider heading:\n{text}");
    assert!(text.contains("Opus"), "should list the model:\n{text}");
    assert!(!text.contains("Anthropic / Opus"), "the provider prefix is redundant per row:\n{text}");
    assert!(text.contains("img"), "should tag image support:\n{text}");
}

#[test]
fn model_selector_provider_headings_have_a_blank_row_above() {
    let option = acp::SessionConfigOption::select(
        "model",
        "Model",
        "",
        vec![
            acp::SessionConfigSelectOption::new("openai:gpt-4o", "OpenAI / GPT-4o"),
            acp::SessionConfigSelectOption::new("openai:gpt-3.5", "OpenAI / GPT-3.5"),
            acp::SessionConfigSelectOption::new("anthropic:opus", "Anthropic / Opus"),
        ],
    );
    let mut ui = model_selector_ui(as_multi_select(option));

    let text = overlay_text(&mut ui);
    let lines: Vec<&str> = text.lines().collect();
    let heading = lines.iter().position(|line| line.contains("OpenAI")).expect("OpenAI heading");
    assert!(heading > 0 && is_blank(lines[heading - 1]), "the first heading needs space below the header:\n{text}");

    ui.type_text("anthropic");
    let text = overlay_text(&mut ui);
    let lines: Vec<&str> = text.lines().collect();
    let heading = lines.iter().position(|line| line.contains("Anthropic")).expect("Anthropic heading");
    assert!(
        heading > 0 && is_blank(lines[heading - 1]),
        "a heading must not run into the previous group's models:\n{text}"
    );
}

#[test]
fn model_selector_groups_collapsed_unavailable_providers_under_one_heading() {
    let collapsed = acp::SessionConfigSelectOption::new("__unavailable:bedrock", "Bedrock (5 models)")
        .description("Unavailable: set AWS_BEARER_TOKEN_BEDROCK");
    let option = acp::SessionConfigOption::select(
        "model",
        "Model",
        "",
        vec![collapsed, acp::SessionConfigSelectOption::new("openai:gpt-4o", "OpenAI / GPT-4o")],
    );
    let mut ui = model_selector_ui(as_multi_select(option));
    ui.type_text("bedrock");

    let text = overlay_text(&mut ui);
    assert!(text.contains("Unavailable"), "collapsed providers should share an Unavailable heading:\n{text}");
    assert!(!text.contains("_unavailable"), "the value prefix is not a heading:\n{text}");
    assert!(text.contains("Bedrock (5 models)"), "the collapsed row should stay:\n{text}");
}

#[test]
fn model_selector_has_no_column_title_row() {
    let mut ui =
        model_selector_ui(multi_select_option("model", "Model", "", &[("anthropic:opus", "Anthropic / Opus")]));

    let text = overlay_text(&mut ui);
    for column in ["Effort", "Capabilities", "Status"] {
        assert!(!text.contains(column), "the {column} column title should be gone:\n{text}");
    }
}

#[test]
fn model_selector_header_labels_are_dimmer_than_the_text_they_label() {
    let mut ui = model_selector_ui(multi_select_option(
        "model",
        "Model",
        "anthropic:opus",
        &[("anthropic:opus", "Anthropic / Opus")],
    ));
    ui.type_text("op");
    ui.resize(80, 24);
    ui.draw();

    let buffer = ui.backend().buffer().clone();
    let theme = Theme::default();
    let search = row_containing(&buffer, "Model search:").expect("the search row");
    assert_eq!(cell_fg(&buffer, search, "Model search:"), Some(theme.text_secondary));
    assert_eq!(cell_fg(&buffer, search, "op"), Some(theme.text_primary));

    let selected = row_containing(&buffer, "Selected:").expect("the selected row");
    assert_eq!(cell_fg(&buffer, selected, "Selected:"), Some(theme.text_secondary));
    assert_eq!(cell_fg(&buffer, selected, "Opus"), Some(theme.text_primary));
}

#[test]
fn model_selector_strips_colon_separated_provider_prefixes() {
    let option = acp::SessionConfigOption::select(
        "model",
        "Model",
        "",
        vec![acp::SessionConfigSelectOption::new("deepseek:v3", "DeepSeek: DeepSeek-V3")],
    );
    let mut ui = model_selector_ui(as_multi_select(option));

    let text = overlay_text(&mut ui);
    assert!(
        text.lines().any(|line| line.trim() == "DeepSeek"),
        "the provider display name should be the heading:\n{text}"
    );
    assert!(text.contains("DeepSeek-V3"), "the model row should keep the model name:\n{text}");
    assert!(!text.contains("DeepSeek: DeepSeek-V3"), "the prefix is redundant under its heading:\n{text}");
}

#[test]
fn model_selector_uses_acp_groups_as_provider_headings() {
    let mut option =
        acp::SessionConfigOption::select("model", "Model", "", Vec::<acp::SessionConfigSelectOption>::new());
    if let acp::SessionConfigKind::Select(select) = &mut option.kind {
        select.options = acp::SessionConfigSelectOptions::Grouped(vec![
            acp::SessionConfigSelectGroup::new(
                "anthropic",
                "Anthropic",
                vec![
                    acp::SessionConfigSelectOption::new("anthropic:opus", "Opus"),
                    acp::SessionConfigSelectOption::new("anthropic:sonnet", "Sonnet"),
                ],
            ),
            acp::SessionConfigSelectGroup::new(
                "openai",
                "OpenAI",
                vec![acp::SessionConfigSelectOption::new("openai:gpt", "GPT")],
            ),
        ]);
    }
    let mut ui = model_selector_ui(as_multi_select(option));

    let text = ui.viewport_text();
    let anthropic = text.find("Anthropic").expect("Anthropic heading");
    let opus = text.find("Opus").expect("Opus model");
    let sonnet = text.find("Sonnet").expect("Sonnet model");
    let openai = text.find("OpenAI").expect("OpenAI heading");
    assert!(anthropic < opus && opus < sonnet && sonnet < openai, "models should be grouped:\n{text}");

    ui.type_text("openai");
    let text = ui.viewport_text();
    let openai = text.find("OpenAI").expect("filtered OpenAI heading");
    let gpt = text.find("GPT").expect("filtered GPT model");
    assert!(openai < gpt, "the provider heading should remain above filtered models:\n{text}");
}

#[test]
fn model_selector_shows_reasoning_bar_on_the_focused_row() {
    let with_reasoning = model(
        "anthropic:opus",
        "Anthropic / Opus",
        SelectOptionMeta {
            reasoning_levels: vec![ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High],
            supports_image: true,
            ..Default::default()
        },
    );
    let plain =
        model("deepseek:chat", "DeepSeek / Chat", SelectOptionMeta { supports_image: true, ..Default::default() });
    let option = acp::SessionConfigOption::select("model", "Model", "", vec![with_reasoning, plain]);
    let mut ui = model_selector_ui(as_multi_select(option));

    let text = ui.viewport_text();
    assert!(text.contains("none [···]"), "the focused reasoning model should show the effort bar inline:\n{text}");
    assert_capability_columns_align(&text);
    let capability_column = capability_column_for(&text, "Opus");

    ui.key(key(KeyCode::Tab));
    let text = ui.viewport_text();
    assert!(text.contains("low [■··]"), "the bar should track the cycled effort:\n{text}");
    assert_capability_columns_align(&text);
    assert_eq!(
        text.matches("[").count(),
        text.matches("low [■··]").count() + text.matches("[ ] ").count(),
        "only the focused row should carry a reasoning bar:\n{text}"
    );

    ui.key(key(KeyCode::Down));
    let text = ui.viewport_text();
    assert!(!text.contains('■'), "a model without reasoning levels shows no bar:\n{text}");
    assert_eq!(capability_column_for(&text, "Opus"), capability_column, "columns must not move with focus:\n{text}");
}

#[test]
fn model_selector_does_not_commit_reasoning_when_only_focus_moves() {
    let reasoning_model = model(
        "anthropic:opus",
        "Anthropic / Opus",
        SelectOptionMeta {
            reasoning_levels: vec![ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High],
            ..Default::default()
        },
    );
    let plain_model = model("deepseek:chat", "DeepSeek / Chat", SelectOptionMeta::default());
    let mut ui = TestUiBuilder::new()
        .config_options(vec![
            as_multi_select(acp::SessionConfigOption::select(
                "model",
                "Model",
                "anthropic:opus",
                vec![reasoning_model, plain_model],
            )),
            reasoning_option("high", &["low", "medium", "high"]),
        ])
        .build();
    open_first_config_option(&mut ui);

    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Esc));

    assert_no_commands(&mut ui, "moving focus without selecting a model must not commit reasoning");
}

#[test]
fn model_selector_toggling_twice_leaves_nothing_to_commit() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![multi_select_option("model", "Model", "", &[("anthropic:opus", "Opus")])])
        .build();
    open_first_config_option(&mut ui);

    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Esc));

    assert_no_commands(&mut ui, "toggling back to the original selection is not a change");
}

#[test]
fn model_selector_preselects_the_current_value() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![multi_select_option(
            "model",
            "Model",
            "anthropic:opus,anthropic:sonnet",
            &[("anthropic:opus", "Opus"), ("anthropic:sonnet", "Sonnet")],
        )])
        .build();
    open_first_config_option(&mut ui);

    ui.key(key(KeyCode::Esc));

    assert_no_commands(&mut ui, "closing without edits changes nothing");
}

#[test]
fn model_selector_toggles_only_what_the_query_left_visible() {
    let mut ui = TestUiBuilder::new()
        .config_options(vec![multi_select_option(
            "model",
            "Model",
            "",
            &[("anthropic:opus", "Opus"), ("openai:gpt-4o", "GPT-4o"), ("google:gemini", "Gemini")],
        )])
        .build();
    open_first_config_option(&mut ui);

    ui.type_text("gpt");
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Esc));

    match ui.next_agent_command().expect("expected the filtered model to be committed") {
        AgentCommand::SetConfigOption { value, .. } => assert_eq!(value, "openai:gpt-4o"),
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn config_option_update_keeps_every_option_and_the_theme_row() {
    let mut ui =
        TestUiBuilder::new().config_options(vec![select_with_values("model", "Model", "a", &[("a", "A")])]).build();
    open_settings(&mut ui);

    ui.acp_event(session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![
        select_with_values("model", "Model", "b", &[("a", "A"), ("b", "B")]),
        select_with_values("mode", "Mode", "code", &[("code", "Code")]),
    ]))));

    let text = overlay_text(&mut ui);
    assert!(text.contains("Theme"), "the client-side theme row must survive an agent push:\n{text}");
    assert!(text.contains("Model: B"), "the updated value should show:\n{text}");
    assert!(text.contains("Mode: Code"), "a newly-arrived option should show:\n{text}");
}

#[test]
fn configured_keybinding_replaces_the_default_chord() {
    let mut settings = UiSettings::default();
    settings.keybindings =
        Some(wisp::settings::KeybindingsSettings { toggle_git_diff: Some("ctrl+d".to_string()), ..Default::default() });
    let mut ui = TestUiBuilder::new().settings(settings).build();

    ui.key(ctrl('g'));
    ui.draw();
    assert!(!ui.viewport_text().contains("Git Diff"), "default chord must be unbound after rebinding");

    ui.key(ctrl('d'));
    ui.draw();
    assert!(ui.viewport_text().contains("Git Diff"), "configured chord must open the git diff");
}

#[test]
fn configured_picker_bindings_fire_on_chords() {
    let mut settings = UiSettings::default();
    settings.keybindings = Some(wisp::settings::KeybindingsSettings {
        open_command_picker: Some("ctrl+p".to_string()),
        open_file_picker: Some("ctrl+f".to_string()),
        ..Default::default()
    });
    let directory = TempDir::new().unwrap();
    let mut ui = TestUiBuilder::new().settings(settings).working_dir(directory.path().to_path_buf()).build();

    ui.key(ctrl('p'));
    assert!(ui.app().composer().has_completion(), "a configured chord must open the command picker");
    ui.key(key(KeyCode::Esc));

    ui.key(ctrl('f'));
    assert!(ui.app().composer().has_completion(), "a configured chord must open the file picker");
    assert!(matches!(ui.next_command(), Some(Command::Filesystem(_))), "the file picker must request its index");
}

#[test]
fn command_picker_still_opens_on_a_typed_slash_with_text_present() {
    let mut ui = TestUiBuilder::new().build();

    ui.type_text("see ");
    ui.key(key(KeyCode::Char('/')));
    assert!(!ui.app().composer().has_completion(), "a slash inside text must not open the command picker");
    assert_eq!(ui.app().composer().text(), "see /");

    for _ in 0..5 {
        ui.key(key(KeyCode::Backspace));
    }
    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion(), "a slash on an empty composer must open the command picker");
    assert_eq!(ui.app().composer().text(), "/");
}
