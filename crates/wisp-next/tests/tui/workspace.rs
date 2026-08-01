use super::support::*;

fn make_ui_with_workspace_move() -> TestUi {
    TestUiBuilder::new().workspace_move().build()
}

fn make_failable_ui_with_workspace_move() -> (TestUi, Arc<AtomicBool>) {
    TestUiBuilder::new().workspace_move().build_failable()
}

#[test]
fn workspace_move_command_hidden_without_capability() {
    let mut ui = TestUi::new();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("/clear"), "{viewport}");
    assert!(!viewport.contains("/move"), "{viewport}");
}

#[test]
fn workspace_move_command_visible_with_capability() {
    let mut ui = make_ui_with_workspace_move();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("/move"), "{viewport}");
}

#[test]
fn workspace_move_command_rejected_when_prompt_in_flight() {
    let mut ui = make_ui_with_workspace_move();

    ui.submit("hello");
    let _ = ui.command_rx().try_recv().unwrap();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));

    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.lines().any(|l| l.contains("Cannot move") && l.contains("workspace")), "{viewport}");
    assert!(viewport.lines().any(|l| l.contains("prompt is running")), "{viewport}");
}

#[test]
fn workspace_move_command_rejected_when_already_listing() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Listing);

    let list_cmd = ui.command_rx().try_recv().unwrap();
    assert!(matches!(list_cmd, PromptCommand::ListWorkspaces(_)));

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Listing);

    ui.draw();
    let viewport = ui.viewport_text();
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("another move is in progress"), "{viewport}");
}

#[test]
fn workspace_list_synchronous_failure_resets_state() {
    let (mut ui, fail_signal) = make_failable_ui_with_workspace_move();

    fail_signal.store(true, Ordering::SeqCst);
    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));

    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);
    ui.draw();
    let viewport = ui.viewport_text();
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to list workspaces"), "{viewport}");
}

#[test]
fn workspace_list_failed_event_resets_state() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Listing);
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspace_list_failed("network error"));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);

    ui.draw();
    let viewport = ui.viewport_text();
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to list workspaces: network error"), "{viewport}");
}

#[test]
fn workspace_picker_opens_with_existing_workspaces() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Listing);
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
        workspace_entry("/tmp/sandbox", false),
    ]));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Picking);
    assert!(ui.app().has_modal());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("/home/user/code/other"), "{viewport}");
    assert!(viewport.contains("/tmp/sandbox"), "{viewport}");
    assert!(!viewport.contains("/home/user/code/current"), "current workspace should be excluded:\n{viewport}");
    assert!(viewport.contains("Create new workspace"), "{viewport}");
}

#[test]
fn double_ctrl_c_exits_over_workspace_picker() {
    let mut ui = make_ui_with_workspace_move();
    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();
    ui.acp_event(workspaces_listed(vec![workspace_entry("/tmp/sandbox", false)]));
    assert!(ui.app().has_modal());
    assert_ctrl_c_exits_over_open_layer(ui.app_mut());
}

#[test]
fn workspace_picker_shows_empty_state_when_no_workspaces() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Picking);
    assert!(ui.app().has_modal());

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(!viewport.contains("No other workspaces available"), "{viewport}");
}

#[test]
fn workspace_picker_esc_closes_and_resets_state() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Picking);
    assert!(ui.app().has_modal());

    ui.key(key(KeyCode::Esc));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);
    assert!(!ui.app().has_modal());
}

#[test]
fn workspace_picker_enter_selects_existing_workspace() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Moving);
    assert!(!ui.app().has_modal());

    let cmd = ui.command_rx().try_recv().unwrap();
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
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("Create new workspace"), "{viewport}");

    ui.key(key(KeyCode::Enter));
    ui.draw();
    let viewport2 = ui.viewport_text();
    assert!(viewport2.contains("New workspace"), "{viewport2}");
}

#[test]
fn workspace_naming_new_esc_returns_to_list_mode() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    ui.key(key(KeyCode::Enter));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("New workspace"), "{viewport}");

    ui.key(key(KeyCode::Esc));
    assert!(ui.app().has_modal());
    ui.draw();
    let viewport2 = ui.viewport_text();
    assert!(viewport2.contains("Create new workspace"), "{viewport2}");
}

#[test]
fn workspace_naming_new_enter_with_name_emits_move_target() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    ui.key(key(KeyCode::Enter));
    ui.paste("my-new-workspace");
    assert!(ui.app().composer().text().is_empty(), "paste must belong to the workspace editor");
    ui.key(key(KeyCode::Enter));

    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Moving);
    assert!(!ui.app().has_modal());

    let cmd = ui.command_rx().try_recv().unwrap();
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
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/project-a", false),
        workspace_entry("/tmp/test", false),
    ]));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("project-a"), "{viewport}");
    assert!(viewport.contains("/tmp/test"), "{viewport}");

    // Use a query that only matches one entry
    ui.key(key(KeyCode::Char('j')));
    ui.draw();
    let viewport2 = ui.viewport_text();
    assert!(viewport2.contains("project-a"), "{viewport2}");
    assert!(!viewport2.contains("/tmp/test"), "{viewport2}");
    assert!(!viewport2.contains("Create new"), "{viewport2}");
}

#[test]
fn workspace_move_success_updates_cwd_and_reloads_session() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Moving);

    let cmd = ui.command_rx().try_recv().unwrap();
    assert!(matches!(cmd, PromptCommand::MoveWorkspace { .. }));

    ui.acp_event(workspace_moved("/home/user/code/other"));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::LoadingSession);

    let load_cmd = ui.command_rx().try_recv().unwrap();
    match load_cmd {
        PromptCommand::LoadSession { session_id, cwd } => {
            assert_eq!(session_id.0.as_ref(), "test-session");
            assert_eq!(cwd, std::path::Path::new("/home/user/code/other"));
        }
        other => panic!("expected LoadSession, got {other:?}"),
    }

    ui.acp_event(session_loaded("test-session", Vec::new()));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);
}

#[test]
fn workspace_move_success_buffers_and_replays_session_updates() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspace_moved("/home/user/code/other"));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(session_update_for("test-session", user_message_chunk("buffered-message")));

    ui.acp_event(session_loaded("test-session", Vec::new()));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.lines().any(|l| l.contains("buffered-message")), "{viewport}");
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Moved to /home/user/code/other"), "{viewport}");
}

#[test]
fn workspace_move_load_session_failure_recovers() {
    let (mut ui, fail_signal) = make_failable_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    let _ = ui.command_rx().try_recv().unwrap();

    fail_signal.store(true, Ordering::SeqCst);
    ui.acp_event(workspace_moved("/home/user/code/other"));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.lines().any(|l| l.contains("Failed to reload session")), "{viewport}");
}

#[test]
fn workspace_move_server_side_load_failure_recovers() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspace_moved("/home/user/code/other"));
    let _ = ui.command_rx().try_recv().unwrap();
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::LoadingSession);

    ui.acp_event(AcpEvent::PromptError(agent_client_protocol::Error::internal_error()));
    assert_eq!(
        ui.app().workspace_move_state(),
        WorkspaceMoveState::Idle,
        "a load that fails on the server should stop the loading indicator"
    );

    ui.acp_event(session_update_for("test-session", user_message_chunk("after the failure")));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(
        viewport.lines().any(|l| l.contains("after the failure")),
        "updates should stop being buffered once the load fails:\n{viewport}"
    );
}

#[test]
fn workspace_move_failed_event_resets_state() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    let _ = ui.command_rx().try_recv().unwrap();
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Moving);

    ui.acp_event(workspace_move_failed("permission denied"));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.lines().any(|l| l.contains("Workspace move failed")), "{viewport}");
    assert!(viewport.lines().any(|l| l.contains("permission denied")), "{viewport}");
}

#[test]
fn workspace_move_synchronous_error_resets_state() {
    let (mut ui, fail_signal) = make_failable_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    fail_signal.store(true, Ordering::SeqCst);
    ui.key(key(KeyCode::Enter));
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);

    ui.draw();
    let viewport = ui.viewport_text();
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to move workspace"), "{viewport}");
}

#[test]
fn workspace_picker_renders_on_narrow_terminal() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/project-a", false),
    ]));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.contains("project-a"), "{viewport}");
}

#[test]
fn workspace_move_picker_closes_when_connection_closes() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.command_rx().try_recv().unwrap();

    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    assert!(ui.app().has_modal());

    ui.acp_event(AcpEvent::ConnectionClosed);
    assert!(!ui.app().has_modal());
    assert_eq!(ui.app().workspace_move_state(), WorkspaceMoveState::Idle);
}
