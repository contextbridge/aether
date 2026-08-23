use super::support::*;

#[test]
fn clear_is_builtin_and_issues_new_session_command() {
    let mut app = make_app();

    app.type_text("/clear");
    app.key(key(KeyCode::Tab));

    let cmd = app.next_agent_command();
    assert!(
        matches!(cmd, Some(AgentCommand::NewSession { .. })),
        "expected NewSession command after /clear, got {cmd:?}"
    );
}

#[test]
fn clear_creates_new_session_and_resets_state() {
    let mut ui = TestUi::new();

    ui.submit("old message");
    ui.draw();
    ui.assert_viewport_contains("old message");

    ui.type_text("/clear");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    let old_conversation = ui.app().conversation_id();
    ui.acp_event(new_session_created("new-session", vec![select_option("model", "sonnet")]));

    assert_ne!(ui.app().conversation_id(), old_conversation);
    assert!(!ui.app().conversation_items().iter().any(|item| {
        matches!(item.content(), ConversationContent::Notice(notice) if notice.text.contains("New session created"))
    }));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(!viewport.contains("old message"), "old message should be gone after clear:\n{viewport}");
}

#[test]
fn clear_restores_compatible_config_selections() {
    let options = vec![select_option("model", "opus"), mode_option("code", &["code", "plan", "ask"])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.type_text("/clear");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(new_session_created(
        "new-session",
        vec![select_option("model", "haiku"), mode_option("ask", &["code", "plan", "ask"])],
    ));

    let restore_cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(&restore_cmd, AgentCommand::SetConfigOption { config_id, value, .. } if config_id == "model" && value == "opus"),
        "expected model restored to opus, got {restore_cmd:?}"
    );
}

#[test]
fn resume_is_builtin_and_lists_sessions() {
    let mut app = make_app();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));

    let cmd = app.next_agent_command();
    assert!(
        matches!(cmd, Some(AgentCommand::ListSessions)),
        "expected ListSessions command after /resume, got {cmd:?}"
    );
}

#[test]
fn session_list_excludes_active_session() {
    let mut app = make_app();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![
        session_info("test-session", "/tmp/current", "Current", "2025-01-01T00:00:00Z"),
        session_info("other-session", "/tmp/other", "Other", "2025-01-02T00:00:00Z"),
    ]));

    assert!(app.app().has_session_picker());
}

#[test]
fn resume_loads_selected_session() {
    let mut app = make_app();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]));

    app.key(key(KeyCode::Enter));

    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(&cmd, AgentCommand::LoadSession { session_id, cwd } if session_id.0.as_ref() == "old" && cwd == &std::path::PathBuf::from("/tmp/old")),
        "expected LoadSession for old session, got {cmd:?}"
    );
}

#[test]
fn empty_session_list_shows_no_sessions() {
    let mut ui = TestUi::new();

    ui.type_text("/resume");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(sessions_listed(vec![]));

    assert!(ui.app().has_session_picker());
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("No previous sessions"), "expected empty state:\n{viewport}");
}

#[test]
fn esc_closes_session_picker() {
    let mut app = make_app();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.app().has_session_picker());

    app.key(key(KeyCode::Esc));
    assert!(!app.app().has_session_picker());
}

#[test]
fn new_session_send_failure_shows_transcript_error() {
    let mut app = TestUiBuilder::new().build();

    app.type_text("/clear");
    app.key(key(KeyCode::Tab));
    assert!(matches!(app.next_command(), Some(Command::Agent(AgentCommand::NewSession { .. }))));
    app.deliver_result(CommandResult::Failed {
        command: FailedCommand::Other("create new session"),
        error: "send failed".to_string(),
    });

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("new session") && message.contains("fail"));
    assert!(has_error, "expected visible transcript error for new_session failure, got {messages:?}");

    assert!(!app.app().exit_requested(), "app should remain interactive after new_session failure");
}

#[test]
fn list_sessions_send_failure_shows_transcript_error() {
    let mut app = TestUiBuilder::new().build();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    assert!(matches!(app.next_command(), Some(Command::Agent(AgentCommand::ListSessions))));
    app.deliver_result(CommandResult::Failed {
        command: FailedCommand::Other("list sessions"),
        error: "send failed".to_string(),
    });

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("list sessions") && message.contains("fail"));
    assert!(has_error, "expected visible transcript error for list_sessions failure, got {messages:?}");

    assert!(!app.app().exit_requested(), "app should remain interactive after list_sessions failure");
}

#[test]
fn load_session_send_failure_cleans_up_buffer_and_shows_error() {
    let mut app = TestUiBuilder::new().build();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]));
    assert!(app.app().has_session_picker());

    app.key(key(KeyCode::Enter));
    assert!(matches!(app.next_command(), Some(Command::Agent(AgentCommand::LoadSession { .. }))));
    app.deliver_result(CommandResult::Failed { command: FailedCommand::LoadSession, error: "send failed".to_string() });

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("load session") && message.contains("fail"));
    assert!(has_error, "expected visible transcript error for load_session failure, got {messages:?}");

    assert!(!app.app().exit_requested(), "app should remain interactive after load_session failure");
}

#[test]
fn session_preview_loaded_for_selected_session() {
    let mut app = make_app_with_session_preview();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));

    let preview_cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(&preview_cmd, AgentCommand::SessionPreview { session_id } if session_id == "sess-1"),
        "expected preview for first session, got {preview_cmd:?}"
    );
}

#[test]
fn session_preview_updated_when_selection_changes() {
    let mut app = make_app_with_session_preview();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));
    let _ = app.next_agent_command().unwrap();

    app.key(key(KeyCode::Down));

    let preview_cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(&preview_cmd, AgentCommand::SessionPreview { session_id } if session_id == "sess-2"),
        "expected preview for second session after moving down, got {preview_cmd:?}"
    );
}

#[test]
fn stale_preview_does_not_replace_current() {
    let mut ui = TestUiBuilder::new().session_preview().build();

    ui.type_text("/resume");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));
    let _ = ui.next_agent_command().unwrap();

    ui.key(key(KeyCode::Down));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(AcpEvent::SessionPreviewLoaded(session_preview_response("sess-1")));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(!viewport.contains("hello"), "stale preview should not be shown:\n{viewport}");
}

#[test]
fn session_preview_failure_shows_error() {
    let mut ui = TestUiBuilder::new().session_preview().dimensions(160, 15).build();

    ui.type_text("/resume");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(sessions_listed(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(AcpEvent::SessionPreviewFailed {
        session_id: "sess-1".to_string(),
        error: "server unreachable".to_string(),
    });

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("server unreachable"), "expected error in preview:\n{viewport}");
}

#[test]
fn session_loading_buffer_queues_updates_then_replays() {
    let mut ui = TestUi::new();

    ui.type_text("/resume");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(sessions_listed(vec![session_info("loaded", "/tmp/loaded", "Loaded", "2025-01-01T00:00:00Z")]));
    ui.key(key(KeyCode::Enter));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(session_update_for("loaded", user_message_chunk("buffered message")));
    ui.acp_event(session_update_for(
        "loaded",
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
            "buffered agent",
        )))),
    ));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(!viewport.contains("buffered message"), "buffered updates should not render yet:\n{viewport}");
    assert!(!viewport.contains("buffered agent"), "buffered updates should not render yet:\n{viewport}");

    ui.acp_event(session_loaded("loaded", vec![select_option("model", "sonnet")]));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("buffered message"), "buffered updates should be replayed:\n{viewport}");
    assert!(viewport.contains("buffered agent"), "buffered updates should be replayed:\n{viewport}");
}

#[test]
fn updates_from_the_abandoned_session_do_not_reach_the_loaded_one() {
    let mut ui = TestUi::new();

    ui.type_text("/resume");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(sessions_listed(vec![session_info("loaded", "/tmp/loaded", "Loaded", "2025-01-01T00:00:00Z")]));
    ui.key(key(KeyCode::Enter));
    let _ = ui.next_agent_command().unwrap();
    ui.acp_event(session_loaded("loaded", Vec::new()));

    ui.acp_event(session_update_for("test-session", user_message_chunk("late message from the old session")));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(
        !viewport.contains("late message"),
        "an update for the session that was left behind must not land in the new one:\n{viewport}"
    );
}

#[test]
fn loaded_session_uses_server_config_values() {
    let options = vec![select_option("model", "opus"), mode_option("plan", &["code", "plan", "ask"])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![session_info("loaded", "/tmp/loaded", "Loaded", "2025-01-01T00:00:00Z")]));
    app.key(key(KeyCode::Enter));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(session_loaded(
        "loaded",
        vec![select_option("model", "sonnet"), mode_option("code", &["code", "plan", "ask"])],
    ));

    let config = app.app().config_options();
    assert_eq!(config[0].current_value(), Some("sonnet"));
    assert_eq!(config[1].current_value(), Some("code"));
}

#[test]
fn connection_closed_cancels_session_picker() {
    let mut app = make_app();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.app().has_session_picker());

    app.acp_event(AcpEvent::ConnectionClosed);
    assert!(!app.app().has_session_picker());
    assert!(app.app().exit_requested());
}

#[test]
fn session_list_error_shows_in_transcript() {
    let mut app = make_app();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));

    app.acp_event(AcpEvent::ConfigOptionUpdateFailed { error: "list sessions failed".to_string() });

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("list sessions failed"));
    assert!(has_error, "expected visible transcript error, got {messages:?}");
}

#[test]
fn builtin_clear_appears_in_command_picker() {
    let mut ui = TestUi::new();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("/clear"), "built-in /clear should be in command picker:\n{viewport}");
    assert!(viewport.contains("/resume"), "built-in /resume should be in command picker:\n{viewport}");
}

#[test]
fn narrow_terminal_renders_session_picker_without_preview_pane() {
    let mut ui = TestUiBuilder::new().session_preview().dimensions(60, 15).build();

    ui.type_text("/resume");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(sessions_listed(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Session One"), "narrow picker should show session list:\n{viewport}");
    assert!(!viewport.contains("Session preview"), "narrow picker should hide preview pane:\n{viewport}");
}

#[test]
fn composed_chars_do_not_filter_the_session_picker() {
    let mut ui = TestUi::with_dimensions(60, 15);

    ui.type_text("/resume");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();
    ui.acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));

    // Composed characters must not reach the picker's filter query.
    ui.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    ui.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(!viewport.contains("'o'"), "composed chars must not filter the picker:\n{viewport}");
    assert!(
        viewport.contains("Session One") && viewport.contains("Session Two"),
        "both rows should still show:\n{viewport}"
    );

    // The plain character still filters.
    ui.key(key(KeyCode::Char('o')));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("'o'"), "plain char should set the filter query:\n{viewport}");
}

#[test]
fn new_modal_replaces_session_picker() {
    let mut app = make_app();

    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();

    app.acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.app().has_session_picker());

    block_on_local(async {
        with_elicitation(&mut app, form_elicitation("test", "", ElicitationSchema::new())).await;
    });

    assert!(!app.app().has_session_picker());
}
