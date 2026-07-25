use super::support::*;

#[test]
fn clear_is_builtin_and_issues_new_session_command() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));

    let cmd = command_rx.try_recv().ok();
    assert!(
        matches!(cmd, Some(PromptCommand::NewSession { .. })),
        "expected NewSession command after /clear, got {cmd:?}"
    );
}

#[test]
fn clear_creates_new_session_and_resets_state() {
    let (mut app, mut command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());

    submit_prompt(&mut app, "old message");
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    assert!(buffer_text(&viewport_buffer(&mut terminal)).contains("old message"));

    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    let old_generation = app.transcript_generation();
    app.on_acp_event(new_session_created("new-session", vec![select_option("model", "sonnet")]));

    assert_eq!(app.transcript_generation(), old_generation.wrapping_add(1));
    assert_eq!(app.pending_items().len(), 1);
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("old message"), "old message should be gone after clear:\n{viewport}");
}

#[test]
fn clear_restores_compatible_config_selections() {
    let options = vec![select_option("model", "opus"), mode_option("code", &["code", "plan", "ask"])];
    let (mut app, mut command_rx) = AppBuilder::new().config_options(options).build();

    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(new_session_created(
        "new-session",
        vec![select_option("model", "haiku"), mode_option("ask", &["code", "plan", "ask"])],
    ));

    let restore_cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&restore_cmd, PromptCommand::SetConfigOption { config_id, value, .. } if config_id == "model" && value == "opus"),
        "expected model restored to opus, got {restore_cmd:?}"
    );
}

#[test]
fn resume_is_builtin_and_lists_sessions() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));

    let cmd = command_rx.try_recv().ok();
    assert!(
        matches!(cmd, Some(PromptCommand::ListSessions)),
        "expected ListSessions command after /resume, got {cmd:?}"
    );
}

#[test]
fn session_list_excludes_active_session() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![
        session_info("test-session", "/tmp/current", "Current", "2025-01-01T00:00:00Z"),
        session_info("other-session", "/tmp/other", "Other", "2025-01-02T00:00:00Z"),
    ]));

    assert!(app.has_session_picker());
}

#[test]
fn resume_loads_selected_session() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]));

    app.on_key(key(KeyCode::Enter));

    let cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&cmd, PromptCommand::LoadSession { session_id, cwd } if session_id.0.as_ref() == "old" && cwd == &std::path::PathBuf::from("/tmp/old")),
        "expected LoadSession for old session, got {cmd:?}"
    );
}

#[test]
fn empty_session_list_shows_no_sessions() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![]));

    assert!(app.has_session_picker());
    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("No previous sessions"), "expected empty state:\n{viewport}");
}

#[test]
fn esc_closes_session_picker() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.has_session_picker());

    app.on_key(key(KeyCode::Esc));
    assert!(!app.has_session_picker());
}

// ── Sub-agent integration tests ──

#[test]
fn new_session_send_failure_shows_transcript_error() {
    let (mut app, fail_signal, mut command_rx) = AppBuilder::new().build_failable();

    fail_signal.store(true, Ordering::Relaxed);
    type_text(&mut app, "/clear");
    app.on_key(key(KeyCode::Tab));
    assert!(command_rx.try_recv().is_err(), "send should have failed");

    let items = app.drain_finalized();
    let has_error = items
        .iter()
        .any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("new session") && msg.contains("fail")));
    assert!(has_error, "expected visible transcript error for new_session failure, got {items:?}");

    assert!(!app.exit_requested(), "app should remain interactive after new_session failure");
}

#[test]
fn list_sessions_send_failure_shows_transcript_error() {
    let (mut app, fail_signal, mut command_rx) = AppBuilder::new().build_failable();

    fail_signal.store(true, Ordering::Relaxed);
    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    assert!(command_rx.try_recv().is_err(), "send should have failed");

    let items = app.drain_finalized();
    let has_error = items
        .iter()
        .any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("list sessions") && msg.contains("fail")));
    assert!(has_error, "expected visible transcript error for list_sessions failure, got {items:?}");

    assert!(!app.exit_requested(), "app should remain interactive after list_sessions failure");
}

#[test]
fn load_session_send_failure_cleans_up_buffer_and_shows_error() {
    let (mut app, fail_signal, mut command_rx) = AppBuilder::new().build_failable();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old Session", "2025-01-01T00:00:00Z")]));
    assert!(app.has_session_picker());

    fail_signal.store(true, Ordering::Relaxed);
    app.on_key(key(KeyCode::Enter));
    assert!(command_rx.try_recv().is_err(), "send should have failed");

    let items = app.drain_finalized();
    let has_error = items
        .iter()
        .any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("load session") && msg.contains("fail")));
    assert!(has_error, "expected visible transcript error for load_session failure, got {items:?}");

    assert!(!app.exit_requested(), "app should remain interactive after load_session failure");
}

#[test]
fn session_preview_loaded_for_selected_session() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));

    let preview_cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&preview_cmd, PromptCommand::SessionPreview(params) if params.session_id == "sess-1"),
        "expected preview for first session, got {preview_cmd:?}"
    );
}

#[test]
fn session_preview_updated_when_selection_changes() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));
    let _ = command_rx.try_recv().unwrap();

    app.on_key(key(KeyCode::Down));

    let preview_cmd = command_rx.try_recv().unwrap();
    assert!(
        matches!(&preview_cmd, PromptCommand::SessionPreview(params) if params.session_id == "sess-2"),
        "expected preview for second session after moving down, got {preview_cmd:?}"
    );
}

#[test]
fn stale_preview_does_not_replace_current() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![
        session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z"),
        session_info("sess-2", "/tmp/two", "Session Two", "2025-01-02T00:00:00Z"),
    ]));
    let _ = command_rx.try_recv().unwrap();

    app.on_key(key(KeyCode::Down));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::SessionPreviewLoaded(session_preview_response("sess-1")));

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("hello"), "stale preview should not be shown:\n{viewport}");
}

#[test]
fn session_preview_failure_shows_error() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(AcpEvent::SessionPreviewFailed {
        session_id: "sess-1".to_string(),
        error: "server unreachable".to_string(),
    });

    let mut terminal = make_terminal_with_width(160);
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("server unreachable"), "expected error in preview:\n{viewport}");
}

#[test]
fn session_loading_buffer_queues_updates_then_replays() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("loaded", "/tmp/loaded", "Loaded", "2025-01-01T00:00:00Z")]));
    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(session_update_for("loaded", user_message_chunk("buffered message")));
    app.on_acp_event(session_update_for(
        "loaded",
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
            "buffered agent",
        )))),
    ));

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("buffered message"), "buffered updates should not render yet:\n{viewport}");
    assert!(!viewport.contains("buffered agent"), "buffered updates should not render yet:\n{viewport}");

    app.on_acp_event(session_loaded("loaded", vec![select_option("model", "sonnet")]));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("buffered message"), "buffered updates should be replayed:\n{viewport}");
    assert!(viewport.contains("buffered agent"), "buffered updates should be replayed:\n{viewport}");
}

#[test]
fn loaded_session_uses_server_config_values() {
    let options = vec![select_option("model", "opus"), mode_option("plan", &["code", "plan", "ask"])];
    let (mut app, mut command_rx) = AppBuilder::new().config_options(options).build();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("loaded", "/tmp/loaded", "Loaded", "2025-01-01T00:00:00Z")]));
    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(session_loaded(
        "loaded",
        vec![select_option("model", "sonnet"), mode_option("code", &["code", "plan", "ask"])],
    ));

    let config = app.config_options();
    let acp::SessionConfigKind::Select(model) = &config[0].kind else {
        panic!("expected model");
    };
    assert_eq!(model.current_value.0.as_ref(), "sonnet");
    let acp::SessionConfigKind::Select(mode) = &config[1].kind else {
        panic!("expected mode");
    };
    assert_eq!(mode.current_value.0.as_ref(), "code");
}

#[test]
fn connection_closed_cancels_session_picker() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.has_session_picker());

    app.on_acp_event(AcpEvent::ConnectionClosed);
    assert!(!app.has_session_picker());
    assert!(app.exit_requested());
}

#[test]
fn session_list_error_shows_in_transcript() {
    let (mut app, _command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));

    app.on_acp_event(AcpEvent::ConfigOptionUpdateFailed { error: "list sessions failed".to_string() });

    let items = app.drain_finalized();
    let has_error =
        items.iter().any(|item| matches!(item, HistoryItem::User(msg) if msg.contains("list sessions failed")));
    assert!(has_error, "expected visible transcript error, got {items:?}");
}

#[test]
fn builtin_clear_appears_in_command_picker() {
    let (mut app, _command_rx) = make_app();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_overlay());

    let mut terminal = make_terminal();
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/clear"), "built-in /clear should be in command picker:\n{viewport}");
    assert!(viewport.contains("/resume"), "built-in /resume should be in command picker:\n{viewport}");
}

#[test]
fn narrow_terminal_renders_session_picker_without_preview_pane() {
    let (mut app, mut command_rx) = make_app_with_session_preview();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("sess-1", "/tmp/one", "Session One", "2025-01-01T00:00:00Z")]));

    let mut terminal = make_terminal_with_width(60);
    let mut renderer = Presenter::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Session One"), "narrow picker should show session list:\n{viewport}");
    assert!(!viewport.contains("Session preview"), "narrow picker should hide preview pane:\n{viewport}");
}

#[test]
fn new_modal_replaces_session_picker() {
    let (mut app, mut command_rx) = make_app();

    type_text(&mut app, "/resume");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(sessions_listed(vec![session_info("old", "/tmp/old", "Old", "2025-01-01T00:00:00Z")]));
    assert!(app.has_session_picker());

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (cx, mut peer) = test_connection().await;
        let (responder, _response_rx) = peer.fake_elicitation(&cx).await;
        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
            responder,
        });
    });

    assert!(!app.has_session_picker());
}
