use super::support::*;
use wisp_next::surface::Surface;

fn server_status_entry(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status)
}

fn oauth_server(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_auth_capability(McpServerAuthCapability::OAuth)
}

fn mcp_notification(servers: Vec<McpServerStatusEntry>) -> AcpEvent {
    AcpEvent::McpNotification(McpNotification::ServerStatus { servers })
}

fn auth_complete(method_id: &str) -> AcpEvent {
    AcpEvent::AuthenticateComplete { method_id: method_id.to_string() }
}

fn auth_failed(method_id: &str) -> AcpEvent {
    AcpEvent::AuthenticateFailed { method_id: method_id.to_string(), error: "simulated failure".to_string() }
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

#[test]
fn server_status_unhealthy_count_updates_status_line() {
    let (mut app, _command_rx) = make_app();
    assert_eq!(app.unhealthy_server_count(), 0);

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        server_status_entry("linear", McpServerStatus::NeedsOAuth),
        server_status_entry("slack", McpServerStatus::Failed { error: "timeout".to_string() }),
    ]));

    assert_eq!(app.unhealthy_server_count(), 2);
}

#[test]
fn server_status_all_connected_gives_zero_unhealthy() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("a", McpServerStatus::Connected { tool_count: 1 }),
        server_status_entry("b", McpServerStatus::Connected { tool_count: 2 }),
    ]));

    assert_eq!(app.unhealthy_server_count(), 0);
}

#[test]
fn server_status_empty_clears_count() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry(
        "x",
        McpServerStatus::Failed { error: "e".to_string() },
    )]));
    assert_eq!(app.unhealthy_server_count(), 1);

    app.on_acp_event(mcp_notification(vec![]));
    assert_eq!(app.unhealthy_server_count(), 0);
}

#[test]
fn settings_overlay_shows_mcp_servers_entry() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry(
        "github",
        McpServerStatus::Connected { tool_count: 3 },
    )]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("MCP Servers"), "settings should show MCP Servers entry:\n{viewport}");
}

#[test]
fn settings_overlay_shows_provider_logins_when_auth_methods_present() {
    let (mut app, _command_rx) = AppBuilder::new().auth_methods(vec![auth_method("codex", "Codex", None)]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Provider Logins"), "settings should show Provider Logins:\n{viewport}");
}

#[test]
fn settings_overlay_no_provider_logins_when_auth_methods_empty() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(!viewport.contains("Provider Logins"), "should not show Provider Logins when empty:\n{viewport}");
}

#[test]
fn mcp_server_status_pane_renders_entries() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        oauth_server("linear", McpServerStatus::NeedsOAuth),
        server_status_entry("slack", McpServerStatus::Failed { error: "timeout".to_string() }),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    // Navigate to MCP Servers entry (after Theme entry)
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("github"), "should show github:\n{viewport}");
    assert!(viewport.contains("5 tools"), "should show tool count:\n{viewport}");
    assert!(viewport.contains("linear"), "should show linear:\n{viewport}");
    assert!(viewport.contains("needs authentication"), "should show auth needed:\n{viewport}");
    assert!(viewport.contains("slack"), "should show slack:\n{viewport}");
    assert!(viewport.contains("timeout"), "should show error:\n{viewport}");
}

#[test]
fn mcp_server_status_empty_shows_placeholder() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("no MCP servers configured"), "should show placeholder:\n{viewport}");
}

#[test]
fn selecting_oauth_server_emits_authenticate_mcp_server() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![oauth_server("linear", McpServerStatus::NeedsOAuth)]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    // Press Enter on the server entry
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }
}

#[test]
fn selecting_non_oauth_server_is_noop() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry(
        "github",
        McpServerStatus::Connected { tool_count: 5 },
    )]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));

    assert!(command_rx.try_recv().is_err(), "should not emit command for non-OAuth server");
}

#[test]
fn provider_login_emits_authenticate() {
    let (mut app, mut command_rx) = AppBuilder::new().auth_methods(vec![auth_method("codex", "Codex", None)]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    // Navigate to Provider Logins (menu: Theme, MCP Servers, Provider Logins)
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Authenticate { method_id } => assert_eq!(method_id, "codex"),
        other => panic!("expected Authenticate, got: {other:?}"),
    }
}

#[test]
fn authenticate_complete_updates_correct_entry() {
    let (mut app, _command_rx) =
        AppBuilder::new().auth_methods(vec![auth_method("a", "A", None), auth_method("b", "B", None)]).build();

    // Open provider logins and start auth for "a"
    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate "a"

    // Simulate authenticate complete for method "a"
    app.on_acp_event(auth_complete("a"));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("logged in"), "should show logged in for 'a':\n{viewport}");
}

#[test]
fn authenticate_failed_resets_to_needs_login() {
    let (mut app, _command_rx) = AppBuilder::new().auth_methods(vec![auth_method("x", "X", None)]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // Authenticate "x"

    app.on_acp_event(auth_failed("x"));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("needs login"), "should show needs login after failure:\n{viewport}");
}

#[test]
fn auth_methods_updated_replaces_provider_entries() {
    let (mut app, _command_rx) = AppBuilder::new().auth_methods(vec![auth_method("old", "Old", None)]).build();

    // Open provider logins
    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // Replace auth methods
    app.on_acp_event(auth_methods_updated(vec![auth_method("new", "New", None)]));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("New"), "should show new provider:\n{viewport}");
    assert!(!viewport.contains("Old"), "should not show old provider:\n{viewport}");
}

#[test]
fn closing_settings_overlay_cancels_pending_elicitation() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();

        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));
        assert!(app.has_modal());

        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::UrlElicitationParams {
                    meta: None,
                    message: "auth".to_string(),
                    url: "https://example.com".to_string(),
                    elicitation_id: "el-1".to_string(),
                },
            },
            responder,
        });

        app.on_key(key(KeyCode::Esc));

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, acp_utils::notifications::ElicitationAction::Cancel);
    });
}

#[test]
fn server_status_updated_while_pane_open_refreshes() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry("a", McpServerStatus::Connected { tool_count: 1 })]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // Open MCP servers pane

    // Send update while pane is open
    app.on_acp_event(mcp_notification(vec![server_status_entry(
        "a",
        McpServerStatus::Failed { error: "crash".to_string() },
    )]));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("crash"), "should show updated status:\n{viewport}");
}

#[test]
fn provider_login_pane_shows_all_statuses() {
    let (mut app, _command_rx) = AppBuilder::new()
        .auth_methods(vec![
            auth_method("needs", "NeedsLogin", None),
            auth_method("authd", "Authed", Some("authenticated")),
        ])
        .build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("NeedsLogin"), "should show NeedsLogin:\n{viewport}");
    assert!(viewport.contains("needs login"), "should show needs login status:\n{viewport}");
    assert!(viewport.contains("Authed"), "should show Authed:\n{viewport}");
    assert!(viewport.contains("logged in"), "should show logged in status:\n{viewport}");
}

#[test]
fn esc_from_server_status_returns_to_menu() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![server_status_entry("a", McpServerStatus::Connected { tool_count: 1 })]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // Enter MCP Servers
    app.on_key(key(KeyCode::Esc)); // Back to menu

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("MCP Servers"), "should be back at menu:\n{viewport}");
}

#[test]
fn esc_from_provider_login_returns_to_menu() {
    let (mut app, _command_rx) = AppBuilder::new().auth_methods(vec![auth_method("x", "X", None)]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Esc));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Provider Logins"), "should be back at menu:\n{viewport}");
}

#[test]
fn server_status_summary_updates_in_menu() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("a", McpServerStatus::Connected { tool_count: 1 }),
        server_status_entry("b", McpServerStatus::Failed { error: "err".to_string() }),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("1 connected") || viewport.contains("1 failed"), "should show summary:\n{viewport}");
}

fn proxied_server_entry(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_proxied(true)
}

fn proxied_oauth_server(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_proxied(true).with_auth_capability(McpServerAuthCapability::OAuth)
}

#[test]
fn server_status_pane_groups_direct_and_proxied_with_headers() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        proxied_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Direct"), "should show Direct header:\n{viewport}");
    assert!(viewport.contains("Proxied"), "should show Proxied header:\n{viewport}");
    assert!(viewport.contains("github"), "should show github:\n{viewport}");
    assert!(viewport.contains("math"), "should show math:\n{viewport}");
    assert!(viewport.contains("linear"), "should show linear:\n{viewport}");
}

#[test]
fn server_status_pane_only_direct_renders_no_headers() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        server_status_entry("slack", McpServerStatus::Failed { error: "err".to_string() }),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(!viewport.contains("Direct"), "should not show Direct header when all direct:\n{viewport}");
    assert!(!viewport.contains("Proxied"), "should not show Proxied header when no proxied:\n{viewport}");
}

#[test]
fn server_status_pane_only_proxied_shows_proxied_header() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        proxied_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(!viewport.contains("Direct"), "should not show Direct header when all proxied:\n{viewport}");
    assert!(viewport.contains("Proxied"), "should show Proxied header:\n{viewport}");
}

#[test]
fn server_status_navigation_skips_headers_and_spacers() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        proxied_server_entry("math", McpServerStatus::Connected { tool_count: 3 }),
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // First server: github (Direct section, row index 1)
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "github is not OAuth, should be noop");

    // Move down once: should land on math (Proxied section, row index 4 - skipping Direct header)
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "math is not OAuth, should be noop");

    // Move down once: should land on linear (Proxied section, row index 5)
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }

    // Move up: should go back to math, not land on Proxied header or Spacer
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "math is not OAuth after wrap-up");

    // Move up again: should land back on github
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "github is not OAuth after wrap-up");
}

#[test]
fn proxied_oauth_server_sends_original_server_name() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![proxied_oauth_server("linear", McpServerStatus::NeedsOAuth)]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer, got: {other:?}"),
    }
}

#[test]
fn connection_closed_cancels_settings_elicitation() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();

        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));
        assert!(app.has_modal());

        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::UrlElicitationParams {
                    meta: None,
                    message: "auth".to_string(),
                    url: "https://example.com".to_string(),
                    elicitation_id: "el-conn-closed".to_string(),
                },
            },
            responder,
        });

        app.on_acp_event(AcpEvent::ConnectionClosed);

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Cancel);
    });
}

#[test]
fn new_session_created_cancels_settings_elicitation() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();

        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));
        assert!(app.has_modal());

        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::UrlElicitationParams {
                    meta: None,
                    message: "auth".to_string(),
                    url: "https://example.com".to_string(),
                    elicitation_id: "el-new-session".to_string(),
                },
            },
            responder,
        });

        app.on_acp_event(AcpEvent::NewSessionCreated { session_id: SessionId::new("new"), config_options: vec![] });

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Cancel);
    });
}

#[test]
fn server_status_update_entries_preserves_selection_across_group_boundaries() {
    let (mut app, mut command_rx) = make_app();

    app.on_acp_event(mcp_notification(vec![
        server_status_entry("github", McpServerStatus::Connected { tool_count: 5 }),
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
    ]));

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // Navigate to linear (Proxied section, after header + spacer)
    app.on_key(key(KeyCode::Down));

    // Send update that flips the grouping - github becomes proxied too, linear stays OAuth
    app.on_acp_event(mcp_notification(vec![
        proxied_oauth_server("linear", McpServerStatus::NeedsOAuth),
        proxied_server_entry("github", McpServerStatus::Connected { tool_count: 5 }),
    ]));

    app.on_key(key(KeyCode::Enter));
    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::AuthenticateMcpServer { server_name, .. } => assert_eq!(server_name, "linear"),
        other => panic!("expected AuthenticateMcpServer for linear, got: {other:?}"),
    }
}

#[test]
fn settings_builtin_opens_overlay_and_clears_composer() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    // Composer should be cleared
    assert!(app.composer().text().is_empty());
    // Should not emit a Prompt
    assert!(command_rx.try_recv().is_err());
    // Overlay should be open (has_modal returns true)
    assert!(app.has_modal());
}

#[test]
fn settings_builtin_is_listed_in_command_picker() {
    let (mut app, _command_rx) = make_app();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/settings"), "built-in /settings should be in command picker:\n{viewport}");
}

#[test]
fn settings_esc_closes_overlay() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    app.on_key(key(KeyCode::Esc));
    assert!(!app.has_modal());
}

#[test]
fn settings_overlay_renders_on_terminal() {
    let (mut app, _command_rx) =
        AppBuilder::new().config_options(vec![select_option("model", "gpt-4o"), select_option("mode", "code")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("model") || viewport.contains("gpt-4o"),
        "settings overlay should show model entry:\n{viewport}"
    );
    assert!(
        viewport.contains("mode") || viewport.contains("code"),
        "settings overlay should show mode entry:\n{viewport}"
    );
}

#[test]
fn settings_over_renders_with_no_config_options() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("no settings options") || viewport.contains("Configuration"),
        "empty settings should show placeholder:\n{viewport}"
    );
}

#[test]
fn settings_overlay_render_clears_covered_buffer_cells() {
    let options = vec![select_option("model", "gpt-4o")];
    let mut overlay = wisp_next::settings_overlay::SettingsOverlay::new(&options, Vec::new(), &[]);
    let area = ratatui::layout::Rect::new(0, 0, 40, 15);
    let mut buffer = Buffer::filled(area, Cell::new("X"));
    let theme = Theme::default();
    let mut highlighter = wisp_next::syntax::SyntaxHighlighter::new();
    let mut cx =
        wisp_next::render_context::RenderContext { theme: &theme, highlighter: &mut highlighter, theme_generation: 0 };

    overlay.render(area, &mut buffer, &mut cx);

    assert!(!buffer_text(&buffer).contains('X'), "overlay must clear every covered cell");
}

#[test]
fn settings_overlay_clears_conversation_content_behind_it() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();
    submit_prompt(&mut app, "CHAT_CONTENT_MUST_NOT_SHOW_THROUGH_SETTINGS");
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Configuration"), "settings overlay should render:\n{viewport}");
    assert!(
        !viewport.contains("CHAT_CONTENT_MUST_NOT_SHOW_THROUGH_SETTINGS"),
        "conversation content must be cleared behind settings:\n{viewport}"
    );
}

#[test]
fn settings_overlay_still_valid_after_scrollback() {
    let (mut app, _command_rx) =
        AppBuilder::new().config_options(vec![select_option("model", "gpt-4o"), select_option("mode", "code")]).build();

    // Fill transcript with content to push scrollback
    for i in 0..30 {
        app.on_acp_event(text_chunk(&format!("line {i}")));
    }
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    // Now open settings
    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Configuration"), "settings overlay should render after scrollback:\n{viewport}");
}

#[test]
fn settings_overlay_renders_at_narrow_width() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal_with_width(30);
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("model") || viewport.contains("gpt-4o"),
        "settings should render at 30 cols:\n{viewport}"
    );
}

#[test]
fn settings_overlay_renders_at_short_height() {
    let (mut app, _command_rx) =
        AppBuilder::new().config_options(vec![select_option("model", "gpt-4o"), select_option("mode", "code")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));

    let mut terminal = make_terminal_with_width(40);
    terminal.backend_mut().resize(40, 8);
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("model") || viewport.contains("too small"),
        "settings should handle short terminal:\n{viewport}"
    );
}

#[test]
fn settings_selecting_option_emits_config_option() {
    let (mut app, mut command_rx) = AppBuilder::new()
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

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // Down past the Theme entry, Enter to open picker, Down to second option, Enter to confirm
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // Should have emitted a set_config_option command
    let cmd = command_rx.try_recv().expect("expected set_config_option");
    match cmd {
        PromptCommand::SetConfigOption { config_id, value, .. } => {
            assert_eq!(config_id, "model");
            assert_eq!(value, "claude");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn settings_multi_select_opens_model_selector() {
    let (mut app, _command_rx) = AppBuilder::new()
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

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // Down past the Theme entry, then Enter should open model selector since multi_select is true
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Model search"), "should show model selector:\n{viewport}");
}

#[test]
fn settings_multi_select_toggle_and_confirm() {
    let (mut app, mut command_rx) = AppBuilder::new()
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

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down)); // Skip Theme entry
    app.on_key(key(KeyCode::Enter)); // Open model selector

    app.on_key(key(KeyCode::Enter)); // Toggle first model
    app.on_key(key(KeyCode::Esc)); // Confirm and close

    let cmd = command_rx.try_recv().expect("expected set_config_option");
    match cmd {
        PromptCommand::SetConfigOption { config_id, value, .. } => {
            assert_eq!(config_id, "model");
            assert!(value.contains("anthropic:opus"), "value: {value}");
        }
        other => panic!("expected SetConfigOption, got: {other:?}"),
    }
}

#[test]
fn config_option_update_refreshes_settings_overlay() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // Simulate config update from server
    app.on_acp_event(session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![
        select_option("model", "sonnet"),
        select_option("mode", "code"),
    ]))));

    // Overlay should still be open with updated options
    assert!(app.has_modal());
    let options = app.config_options();
    assert_eq!(options.len(), 2);
}

#[test]
fn config_option_update_failed_shows_in_transcript() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    app.on_acp_event(AcpEvent::ConfigOptionUpdateFailed { error: "invalid model".to_string() });

    // Overlay should still be open
    assert!(app.has_modal());

    // Error should be in transcript
    let items = app.drain_finalized();
    let has_error = items.iter().any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("invalid model")));
    assert!(has_error, "expected transcript error, got {items:?}");
}

#[test]
fn connection_closed_clears_settings_overlay() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    app.on_acp_event(AcpEvent::ConnectionClosed);
    assert!(!app.has_modal());
    assert!(app.exit_requested());
}

#[test]
fn new_session_clears_settings_overlay() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // New session created should close settings overlay
    app.on_acp_event(new_session_created("new-id", Vec::new()));
    assert!(!app.has_modal());

    // Should have consumed the new session event
    let _ = command_rx.try_recv().ok();
}

#[test]
fn settings_composer_capture_prevents_normal_input() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    let composer_text_before = app.composer().text().to_string();

    // Typing while settings overlay is open should not modify composer
    for c in "hello".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.composer().text(), composer_text_before);
}

#[test]
fn settings_theme_entry_is_injected_first() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Theme:"), "Theme entry should render first:\n{viewport}");
}

#[test]
fn settings_theme_picker_opens_and_shows_default() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter)); // Open Theme picker (first entry)

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Default"), "Theme picker should show Default option:\n{viewport}");
    assert!(viewport.contains("Theme"), "Theme picker should have Theme header:\n{viewport}");
}

#[test]
fn settings_theme_selection_returns_to_menu() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter)); // Open Theme picker
    app.on_key(key(KeyCode::Enter)); // Confirm default

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Theme: Default"), "Should return to menu with Default selected:\n{viewport}");
}

#[test]
fn settings_theme_empty_file_list_shows_only_default() {
    let (mut app, _command_rx) = AppBuilder::new().build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter)); // Open Theme picker

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Default"), "Should show Default theme option:\n{viewport}");
}

// ── Regression tests for review findings ──

#[test]
fn theme_entry_preserved_after_config_option_update() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    assert!(app.has_modal());

    // ConfigOptionUpdate arrives — Theme entry must still be first
    app.on_acp_event(session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![
        select_option("model", "sonnet"),
    ]))));
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Theme:"), "Theme entry must survive ConfigOptionUpdate:\n{viewport}");
}

#[test]
fn theme_selection_keeps_overlay_open_and_refreshes_display() {
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![select_option("model", "gpt-4o")]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter)); // Open Theme picker
    app.on_key(key(KeyCode::Enter)); // Confirm default theme

    // Overlay must stay open
    assert!(app.has_modal(), "Overlay must stay open after theme selection");

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Theme: Default"), "Theme should show Default after selection:\n{viewport}");
}

#[test]
fn model_selector_provider_heading_does_not_skip_rows() {
    let (mut app, _command_rx) = AppBuilder::new()
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

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down)); // Skip Theme
    app.on_key(key(KeyCode::Enter)); // Open model selector

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    // All four models must appear — headings should not consume model rows
    assert!(viewport.contains("GPT-4o"), "GPT-4o should be visible:\n{viewport}");
    assert!(viewport.contains("GPT-3.5"), "GPT-3.5 should be visible:\n{viewport}");
    assert!(viewport.contains("Opus"), "Opus should be visible:\n{viewport}");
    assert!(viewport.contains("Sonnet"), "Sonnet should be visible:\n{viewport}");
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
    let (mut app, _command_rx) = AppBuilder::new().config_options(vec![model_option]).build();

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    for _ in 0..13 {
        app.on_key(key(KeyCode::Down));
    }

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Model 14"), "last enabled model should be focused and visible:\n{viewport}");

    app.on_key(key(KeyCode::Down));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Model 14"), "selection should remain at the end of the list:\n{viewport}");
}

#[test]
fn model_selector_focused_item_visible_with_provider_headings() {
    let (mut app, _command_rx) = AppBuilder::new()
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

    type_text(&mut app, "/settings");
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down)); // Skip Theme
    app.on_key(key(KeyCode::Enter)); // Open model selector

    // Move down through all items — last item should be visible
    for _ in 0..4 {
        app.on_key(key(KeyCode::Down));
    }

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("PaLM"), "Focused last item with headings should be visible:\n{viewport}");
}

// ── Composer framing ────────────────────────────────────────────
