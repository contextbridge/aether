use super::support::*;

#[test]
fn workspace_move_command_hidden_without_capability() {
    let (mut app, _command_rx) = make_app();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_completion());

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/clear"), "{viewport}");
    assert!(!viewport.contains("/move"), "{viewport}");
}

#[test]
fn workspace_move_command_visible_with_capability() {
    let (mut app, _command_rx) = make_app_with_workspace_move();

    app.on_key(key(KeyCode::Char('/')));
    assert!(app.composer().has_completion());

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/move"), "{viewport}");
}

#[test]
fn workspace_move_command_rejected_when_prompt_in_flight() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    submit_prompt(&mut app, "hello");
    let _ = command_rx.try_recv().unwrap();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));

    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);
    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("Cannot move") && l.contains("workspace")), "{viewport}");
    assert!(viewport.lines().any(|l| l.contains("prompt is running")), "{viewport}");
}

#[test]
fn workspace_move_command_rejected_when_already_listing() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Listing);

    let list_cmd = command_rx.try_recv().unwrap();
    assert!(matches!(list_cmd, PromptCommand::ListWorkspaces(_)));

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Listing);

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("another move is in progress"), "{viewport}");
}

#[test]
fn workspace_list_synchronous_failure_resets_state() {
    let (mut app, fail_signal, _command_rx) = make_failable_app_with_workspace_move();

    fail_signal.store(true, Ordering::SeqCst);
    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));

    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);
    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to list workspaces"), "{viewport}");
}

#[test]
fn workspace_list_failed_event_resets_state() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Listing);
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspace_list_failed("network error"));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to list workspaces: network error"), "{viewport}");
}

#[test]
fn workspace_picker_opens_with_existing_workspaces() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Listing);
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
        workspace_entry("/tmp/sandbox", false),
    ]));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Picking);
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("/home/user/code/other"), "{viewport}");
    assert!(viewport.contains("/tmp/sandbox"), "{viewport}");
    assert!(!viewport.contains("/home/user/code/current"), "current workspace should be excluded:\n{viewport}");
    assert!(viewport.contains("Create new workspace"), "{viewport}");
}

#[test]
fn double_ctrl_c_exits_over_workspace_picker() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();
    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();
    app.on_acp_event(workspaces_listed(vec![workspace_entry("/tmp/sandbox", false)]));
    assert!(app.has_modal());
    assert_ctrl_c_exits_over_open_layer(&mut app);
}

#[test]
fn workspace_picker_shows_empty_state_when_no_workspaces() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Picking);
    assert!(app.has_modal());

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("No other workspaces available"), "{viewport}");
}

#[test]
fn workspace_picker_esc_closes_and_resets_state() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Picking);
    assert!(app.has_modal());

    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);
    assert!(!app.has_modal());
}

#[test]
fn workspace_picker_enter_selects_existing_workspace() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Moving);
    assert!(!app.has_modal());

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.session_id, "test-session");
            match params.target {
                acp_utils::notifications::WorkspaceMoveTarget::Existing { path } => {
                    assert_eq!(path, std::path::PathBuf::from("/home/user/code/other"));
                }
                other @ acp_utils::notifications::WorkspaceMoveTarget::New { .. } => {
                    panic!("expected Existing, got {other:?}")
                }
            }
        }
        other => panic!("expected MoveWorkspace, got {other:?}"),
    }
}

#[test]
fn workspace_picker_enter_selects_create_new_and_shows_naming_mode() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Create new workspace"), "{viewport}");

    app.on_key(key(KeyCode::Enter));
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport2 = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport2.contains("New workspace"), "{viewport2}");
}

#[test]
fn workspace_naming_new_esc_returns_to_list_mode() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    app.on_key(key(KeyCode::Enter));
    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("New workspace"), "{viewport}");

    app.on_key(key(KeyCode::Esc));
    assert!(app.has_modal());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport2 = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport2.contains("Create new workspace"), "{viewport2}");
}

#[test]
fn workspace_naming_new_enter_with_name_emits_move_target() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    app.on_key(key(KeyCode::Enter));
    app.on_paste("my-new-workspace");
    assert!(app.composer().text().is_empty(), "paste must belong to the workspace editor");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Moving);
    assert!(!app.has_modal());

    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::MoveWorkspace(params) => {
            assert_eq!(params.session_id, "test-session");
            match params.target {
                acp_utils::notifications::WorkspaceMoveTarget::New { name } => {
                    assert_eq!(name, "my-new-workspace");
                }
                other @ acp_utils::notifications::WorkspaceMoveTarget::Existing { .. } => {
                    panic!("expected New, got {other:?}")
                }
            }
        }
        other => panic!("expected MoveWorkspace, got {other:?}"),
    }
}

#[test]
fn workspace_picker_filtering_hides_non_matching() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/project-a", false),
        workspace_entry("/tmp/test", false),
    ]));

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("project-a"), "{viewport}");
    assert!(viewport.contains("/tmp/test"), "{viewport}");

    // Use a query that only matches one entry
    app.on_key(key(KeyCode::Char('j')));
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport2 = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport2.contains("project-a"), "{viewport2}");
    assert!(!viewport2.contains("/tmp/test"), "{viewport2}");
    assert!(!viewport2.contains("Create new"), "{viewport2}");
}

#[test]
fn workspace_move_success_updates_cwd_and_reloads_session() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Moving);

    let cmd = command_rx.try_recv().unwrap();
    assert!(matches!(cmd, PromptCommand::MoveWorkspace { .. }));

    app.on_acp_event(workspace_moved("/home/user/code/other"));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::LoadingSession);

    let load_cmd = command_rx.try_recv().unwrap();
    match load_cmd {
        PromptCommand::LoadSession { session_id, cwd } => {
            assert_eq!(session_id.0.as_ref(), "test-session");
            assert_eq!(cwd, std::path::Path::new("/home/user/code/other"));
        }
        other => panic!("expected LoadSession, got {other:?}"),
    }

    app.on_acp_event(session_loaded("test-session", Vec::new()));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);
}

#[test]
fn workspace_move_success_buffers_and_replays_session_updates() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspace_moved("/home/user/code/other"));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(session_update_for("test-session", user_message_chunk("buffered-message")));

    app.on_acp_event(session_loaded("test-session", Vec::new()));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("buffered-message")), "{viewport}");
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Moved to /home/user/code/other"), "{viewport}");
}

#[test]
fn workspace_move_load_session_failure_recovers() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    fail_signal.store(true, Ordering::SeqCst);
    app.on_acp_event(workspace_moved("/home/user/code/other"));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("Failed to reload session")), "{viewport}");
}

#[test]
fn workspace_move_server_side_load_failure_recovers() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspace_moved("/home/user/code/other"));
    let _ = command_rx.try_recv().unwrap();
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::LoadingSession);

    app.on_acp_event(AcpEvent::PromptError(agent_client_protocol::Error::internal_error()));
    assert_eq!(
        app.workspace_move_state(),
        WorkspaceMoveState::Idle,
        "a load that fails on the server should stop the loading indicator"
    );

    app.on_acp_event(session_update_for("test-session", user_message_chunk("after the failure")));
    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.lines().any(|l| l.contains("after the failure")),
        "updates should stop being buffered once the load fails:\n{viewport}"
    );
}

#[test]
fn workspace_move_failed_event_resets_state() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    app.on_key(key(KeyCode::Enter));
    let _ = command_rx.try_recv().unwrap();
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Moving);

    app.on_acp_event(workspace_move_failed("permission denied"));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("Workspace move failed")), "{viewport}");
    assert!(viewport.lines().any(|l| l.contains("permission denied")), "{viewport}");
}

#[test]
fn workspace_move_synchronous_error_resets_state() {
    let (mut app, fail_signal, mut command_rx) = make_failable_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    fail_signal.store(true, Ordering::SeqCst);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);

    let mut terminal = make_terminal();
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to move workspace"), "{viewport}");
}

#[test]
fn workspace_picker_renders_on_narrow_terminal() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/project-a", false),
    ]));

    let mut terminal = make_terminal_with_width(40);
    let mut renderer = Renderer::new(&UiSettings::default());
    renderer.draw(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("project-a"), "{viewport}");
}

#[test]
fn workspace_move_picker_closes_when_connection_closes() {
    let (mut app, mut command_rx) = make_app_with_workspace_move();

    type_text(&mut app, "/move");
    app.on_key(key(KeyCode::Tab));
    let _ = command_rx.try_recv().unwrap();

    app.on_acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    assert!(app.has_modal());

    app.on_acp_event(AcpEvent::ConnectionClosed);
    assert!(!app.has_modal());
    assert_eq!(app.workspace_move_state(), WorkspaceMoveState::Idle);
}
