use super::support::*;

#[test]
fn remote_resume_preserves_server_path_without_local_resolution() {
    let cwd =
        std::env::var_os("HOME").map_or_else(|| "/server".into(), std::path::PathBuf::from).join("remote-project");
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::remote_workspace).dimensions(160, 24).build();
    ui.begin_resume("saved", &cwd.display().to_string());
    ui.answer::<Resume>(Ok(acp::ResumeSessionResponse::new()));
    assert_eq!(
        ui.app().core().session().workspace_status().display_dir,
        cwd.display().to_string(),
        "remote resume shows the agent's status of the resumed session's workspace"
    );
    ui.assert_viewport_contains(&cwd.display().to_string());
    ui.run_command("/clear");
    assert_eq!(ui.held::<NewSession>().0.cwd.0, cwd);
}

#[test]
fn clear_creates_new_session_and_resets_state() {
    let mut ui = TestUi::new();

    ui.submit("old message");
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.assert_viewport_contains("old message");

    let old_conversation = ui.app().core().conversation().id();
    ui.run_command("/clear");

    new_session_created(&mut ui, "new-session", vec![select_option("model", "sonnet")]);

    assert_ne!(ui.app().core().conversation().id(), old_conversation);
    assert!(!ui.app().core().conversation().items().iter().any(|item| {
        matches!(item.content(), ConversationContent::Notice(notice) if notice.text.contains("New session created"))
    }));
    ui.assert_viewport_not_contains("old message");
}

#[test]
fn session_list_excludes_active_session() {
    let mut app = TestUi::new();

    app.open_session_picker(vec![
        session_info("test-session", "/tmp/current", "Current", "2025-01-01T00:00:00Z"),
        session_info("other-session", "/tmp/other", "Other", "2025-01-02T00:00:00Z"),
    ]);

    assert!(app.app().has_modal());
}

#[test]
fn resume_loads_selected_session() {
    let mut app = TestUi::new();

    app.open_session_picker(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]);

    app.key(key(KeyCode::Enter));

    let (request, _) = app.held::<Resume>();
    assert_eq!((request.session_id.0.as_ref(), request.cwd.0.as_path()), ("old", Path::new("/tmp/old")));
}

#[test]
fn empty_session_list_shows_no_sessions() {
    let mut ui = TestUi::new();

    ui.open_session_picker(vec![]);

    assert!(ui.app().has_modal());
    ui.assert_viewport_contains("No previous sessions");
}

#[test]
fn esc_closes_session_picker() {
    let mut app = TestUi::new();

    app.open_session_picker(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]);
    assert!(app.app().has_modal());

    app.key(key(KeyCode::Esc));
    assert!(!app.app().has_modal());
}

#[test]
fn session_preview_loaded_for_selected_session() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::session_preview).build();

    app.open_session_picker(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]);

    assert_eq!(app.held::<Preview>().0.session_id, "sess-1");
}

#[test]
fn session_preview_updated_when_selection_changes() {
    let mut app = TestUiBuilder::new().session(SessionCoreConfigBuilder::session_preview).build();

    app.open_session_picker(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]);

    app.key(key(KeyCode::Down));

    let previewed: Vec<_> =
        std::iter::from_fn(|| app.try_held::<Preview>()).map(|(request, _)| request.session_id).collect();
    assert_eq!(previewed, ["sess-1", "sess-2"]);
}

#[test]
fn stale_preview_does_not_replace_current() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::session_preview).build();

    ui.open_session_picker(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]);

    ui.key(key(KeyCode::Down));

    ui.answer::<Preview>(Ok(session_preview("sess-1", "/tmp/project")));

    ui.assert_viewport_not_contains("hello");
}

#[test]
fn session_preview_failure_shows_error() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::session_preview).dimensions(160, 15).build();

    ui.open_session_picker(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]);

    ui.answer::<Preview>(Err(agent_error("server unreachable")));

    ui.assert_viewport_contains("server unreachable");
}

#[test]
fn loaded_session_replays_typed_notifications_in_order() {
    let mut ui = TestUi::new();

    ui.begin_resume("loaded", "/tmp/loaded");

    ui.assert_viewport_not_contains("buffered message");
    ui.assert_viewport_not_contains("buffered agent");

    for event in [
        on_session("loaded", user_message("user-message", vec!["buffered message".into()])),
        acp::UpdateSessionNotification::new(
            SessionId::new("loaded"),
            acp::SessionUpdate::AgentMessage(
                acp::AgentMessage::new("replayed-agent")
                    .content(vec![acp::ContentBlock::Text(acp::TextContent::new("buffered agent"))]),
            ),
        )
        .into(),
    ] {
        ui.acp_event(event);
    }
    ui.assert_conversation_contains("buffered message");
    session_loaded(&mut ui, vec![select_option("model", "sonnet")]);

    ui.assert_viewport_contains("buffered message");
    ui.assert_viewport_contains("buffered agent");
}

#[test]
fn updates_from_the_abandoned_session_do_not_reach_the_loaded_one() {
    let mut ui = TestUi::new();

    ui.begin_resume("loaded", "/tmp/loaded");
    session_loaded(&mut ui, Vec::new());

    ui.acp_event(on_session(
        "test-session",
        user_message("user-message", vec!["late message from the old session".into()]),
    ));

    ui.assert_viewport_not_contains("late message");
}

#[test]
fn loaded_session_uses_server_config_values() {
    let options = vec![select_option("model", "opus"), mode_option("plan", &["code", "plan", "ask"])];
    let mut app = TestUiBuilder::new().session(|session| session.config_options(options)).build();

    app.begin_resume("loaded", "/tmp/loaded");

    session_loaded(&mut app, vec![select_option("model", "sonnet"), mode_option("code", &["code", "plan", "ask"])]);

    let config = app.app().core().session().config_options();
    assert_eq!(config[0].current_value(), Some("sonnet"));
    assert_eq!(config[1].current_value(), Some("code"));
}

#[test]
fn builtin_clear_appears_in_command_picker() {
    let mut ui = TestUi::new();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.assert_viewport_contains("/clear");
    ui.assert_viewport_contains("/resume");
}

#[test]
fn narrow_terminal_renders_session_picker_without_preview_pane() {
    let mut ui = TestUiBuilder::new().session(SessionCoreConfigBuilder::session_preview).dimensions(60, 15).build();

    ui.open_session_picker(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]);

    ui.assert_viewport_contains("Session One");
    ui.assert_viewport_not_contains("Session preview");
}

#[test]
fn composed_chars_do_not_filter_the_session_picker() {
    let mut ui = TestUi::with_dimensions(60, 15);

    ui.open_session_picker(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]);

    // Composed characters must not reach the picker's filter query.
    ui.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    ui.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT));
    ui.assert_viewport_not_contains("'o'");
    ui.assert_viewport_contains("Session One");
    ui.assert_viewport_contains("Session Two");

    // The plain character still filters.
    ui.key(key(KeyCode::Char('o')));
    ui.assert_viewport_contains("'o'");
}

#[test]
fn new_modal_replaces_session_picker() {
    let mut app = TestUi::new();

    app.open_session_picker(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]);
    app.assert_viewport_contains("Old Session");

    app.elicit(form_elicitation("test", "Approve?", ElicitationSchema::new()));

    app.assert_viewport_contains("Approve?");
    app.assert_viewport_not_contains("Old Session");
}
