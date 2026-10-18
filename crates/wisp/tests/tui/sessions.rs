use super::support::*;

#[test]
fn remote_resume_preserves_server_path_without_local_resolution() {
    let cwd =
        std::env::var_os("HOME").map_or_else(|| "/server".into(), std::path::PathBuf::from).join("remote-project");
    let mut ui = TestUiBuilder::new().remote_workspace().dimensions(160, 24).build();
    ui.begin_resume("saved", &cwd.display().to_string());
    ui.deliver_result(CommandResult::ResumeSession {
        session_id: "saved".into(),
        result: Ok(acp::ResumeSessionResponse::new()),
    });
    let status_fetches =
        ui.count_commands(|command| matches!(command, Command::Agent(AgentCommand::FetchWorkspaceStatus { .. })));
    assert_eq!(status_fetches, 1, "remote resume fetches agent-side workspace status");
    ui.settle_tasks();
    ui.assert_viewport_contains(&cwd.display().to_string());
    ui.run_command("/clear");
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::NewSession { cwd: actual }) if actual == cwd));
}

#[test]
fn clear_is_builtin_and_issues_new_session_command() {
    let mut app = TestUi::new();

    app.run_command("/clear");

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
    ui.next_agent_command().unwrap();
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.assert_viewport_contains("old message");

    let old_conversation = ui.app().conversation_id();
    ui.run_command("/clear");
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(new_session_created("new-session", vec![select_option("model", "sonnet")]));

    assert_ne!(ui.app().conversation_id(), old_conversation);
    assert!(!ui.app().conversation_items().iter().any(|item| {
        matches!(item.content(), ConversationContent::Notice(notice) if notice.contains("New session created"))
    }));
    ui.assert_viewport_not_contains("old message");
}

#[test]
fn clear_restores_compatible_config_selections() {
    let options = vec![select_option("model", "opus"), mode_option("code", &["code", "plan", "ask"])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.run_command("/clear");
    let _ = app.next_agent_command().unwrap();

    app.acp_event(AcpEvent::SessionUpdate(Box::new(acp::UpdateSessionNotification::new(
        "new-session",
        acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![select_option("model", "haiku")])),
    ))));
    app.deliver_result(new_session_created(
        "new-session",
        vec![select_option("model", "haiku"), mode_option("ask", &["code", "plan", "ask"])],
    ));

    assert_sets_config_option(&mut app, "model", "opus");
}

#[test]
fn resume_is_builtin_and_lists_sessions() {
    let mut app = TestUi::new();

    app.run_command("/resume");

    let cmd = app.next_agent_command();
    assert!(
        matches!(cmd, Some(AgentCommand::ListSessions)),
        "expected ListSessions command after /resume, got {cmd:?}"
    );
}

#[test]
fn session_list_excludes_active_session() {
    let mut app = TestUi::new();

    app.open_session_picker(vec![
        session_info("test-session", "/tmp/current", "Current", "2025-01-01T00:00:00Z"),
        session_info("other-session", "/tmp/other", "Other", "2025-01-02T00:00:00Z"),
    ]);

    assert!(app.app().has_session_picker());
}

#[test]
fn resume_loads_selected_session() {
    let mut app = TestUi::new();

    app.open_session_picker(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]);

    app.key(key(KeyCode::Enter));

    let cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(&cmd, AgentCommand::ResumeSession { session_id, cwd } if session_id.0.as_ref() == "old" && cwd == &std::path::PathBuf::from("/tmp/old")),
        "expected LoadSession for old session, got {cmd:?}"
    );
}

#[test]
fn empty_session_list_shows_no_sessions() {
    let mut ui = TestUi::new();

    ui.open_session_picker(vec![]);

    assert!(ui.app().has_session_picker());
    ui.assert_viewport_contains("No previous sessions");
}

#[test]
fn esc_closes_session_picker() {
    let mut app = TestUi::new();

    app.open_session_picker(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]);
    assert!(app.app().has_session_picker());

    app.key(key(KeyCode::Esc));
    assert!(!app.app().has_session_picker());
}

#[test]
fn new_session_send_failure_shows_transcript_error() {
    let mut app = TestUiBuilder::new().build();

    app.run_command("/clear");
    assert!(matches!(app.next_command(), Some(Command::Agent(AgentCommand::NewSession { .. }))));
    app.deliver_result(CommandResult::NewSession(Err("send failed".to_string())));

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("new session") && message.contains("fail"));
    assert!(has_error, "expected visible transcript error for new_session failure, got {messages:?}");

    assert!(!app.app().exit_requested(), "app should remain interactive after new_session failure");
}

#[test]
fn list_sessions_send_failure_shows_transcript_error() {
    let mut app = TestUiBuilder::new().build();

    app.run_command("/resume");
    assert!(matches!(app.next_command(), Some(Command::Agent(AgentCommand::ListSessions))));
    app.deliver_result(CommandResult::SessionsListed(Err("send failed".to_string())));

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("list sessions") && message.contains("fail"));
    assert!(has_error, "expected visible transcript error for list_sessions failure, got {messages:?}");

    assert!(!app.app().exit_requested(), "app should remain interactive after list_sessions failure");
}

#[test]
fn load_session_send_failure_cleans_up_buffer_and_shows_error() {
    let mut app = TestUiBuilder::new().build();

    app.open_session_picker(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]);
    assert!(app.app().has_session_picker());

    app.key(key(KeyCode::Enter));
    assert!(matches!(app.next_command(), Some(Command::Agent(AgentCommand::ResumeSession { .. }))));
    app.deliver_result(CommandResult::ResumeSession { session_id: "old".into(), result: Err("send failed".into()) });

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("resume session") && message.contains("fail"));
    assert!(has_error, "expected visible transcript error for load_session failure, got {messages:?}");

    assert!(!app.app().exit_requested(), "app should remain interactive after load_session failure");
    app.submit("retry");
    assert!(matches!(app.next_agent_command(), Some(AgentCommand::Prompt { .. })));
}

#[test]
fn session_preview_loaded_for_selected_session() {
    let mut app = TestUiBuilder::new().session_preview().build();

    app.open_session_picker(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]);

    let preview_cmd = app.next_agent_command().unwrap();
    assert!(
        matches!(&preview_cmd, AgentCommand::SessionPreview { session_id } if session_id == "sess-1"),
        "expected preview for first session, got {preview_cmd:?}"
    );
}

#[test]
fn session_preview_updated_when_selection_changes() {
    let mut app = TestUiBuilder::new().session_preview().build();

    app.open_session_picker(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]);
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

    ui.open_session_picker(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]);
    let _ = ui.next_agent_command().unwrap();

    ui.key(key(KeyCode::Down));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(CommandResult::SessionPreviewLoaded {
        session_id: "sess-1".into(),
        result: Ok(session_preview_response("sess-1")),
    });

    ui.assert_viewport_not_contains("hello");
}

#[test]
fn session_preview_failure_shows_error() {
    let mut ui = TestUiBuilder::new().session_preview().dimensions(160, 15).build();

    ui.open_session_picker(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]);
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(CommandResult::SessionPreviewLoaded {
        session_id: "sess-1".to_string(),
        result: Err("server unreachable".to_string()),
    });

    ui.assert_viewport_contains("server unreachable");
}

#[test]
fn loaded_session_replays_typed_notifications_in_order() {
    let mut ui = TestUi::new();

    ui.begin_resume("loaded", "/tmp/loaded");

    ui.assert_viewport_not_contains("buffered message");
    ui.assert_viewport_not_contains("buffered agent");

    for event in [
        acp::UpdateSessionNotification::new(SessionId::new("loaded"), user_message_chunk("buffered message")).into(),
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
    ui.deliver_result(session_loaded("loaded", vec![select_option("model", "sonnet")]));

    ui.assert_viewport_contains("buffered message");
    ui.assert_viewport_contains("buffered agent");
}

#[test]
fn updates_from_the_abandoned_session_do_not_reach_the_loaded_one() {
    let mut ui = TestUi::new();

    ui.begin_resume("loaded", "/tmp/loaded");
    ui.deliver_result(session_loaded("loaded", Vec::new()));

    ui.acp_event(session_update_for("test-session", user_message_chunk("late message from the old session")));

    ui.assert_viewport_not_contains("late message");
}

#[test]
fn loaded_session_uses_server_config_values() {
    let options = vec![select_option("model", "opus"), mode_option("plan", &["code", "plan", "ask"])];
    let mut app = TestUiBuilder::new().config_options(options).build();

    app.begin_resume("loaded", "/tmp/loaded");

    app.deliver_result(session_loaded(
        "loaded",
        vec![select_option("model", "sonnet"), mode_option("code", &["code", "plan", "ask"])],
    ));

    let config = app.app().config_options();
    assert_eq!(config[0].current_value(), Some("sonnet"));
    assert_eq!(config[1].current_value(), Some("code"));
}

#[test]
fn connection_closed_cancels_session_picker() {
    let mut app = TestUi::new();

    app.open_session_picker(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]);
    assert!(app.app().has_session_picker());

    app.acp_event(AcpEvent::ConnectionClosed);
    assert!(!app.app().has_session_picker());
    assert!(app.app().exit_requested());
}

#[test]
fn session_list_error_shows_in_transcript() {
    let mut app = TestUi::new();

    app.run_command("/resume");

    app.deliver_result(CommandResult::SessionsListed(Err("list sessions failed".to_string())));

    let messages: Vec<_> = message_texts(&app).collect();
    let has_error = messages.iter().any(|message| message.contains("list sessions failed"));
    assert!(has_error, "expected visible transcript error, got {messages:?}");
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
    let mut ui = TestUiBuilder::new().session_preview().dimensions(60, 15).build();

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

    app.open_session_picker(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]);
    assert!(app.app().has_session_picker());

    block_on_local(async {
        with_elicitation(&mut app, form_elicitation("test", "", ElicitationSchema::new())).await;
    });

    assert!(!app.app().has_session_picker());
}
