use super::support::*;

#[test]
fn remote_workspace_move_keeps_paths_server_side() {
    let mut ui = TestUiBuilder::new().remote_workspace().workspace_move().dimensions(120, 24).build();
    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::ListWorkspaces { .. })));
    ui.deliver_result(workspaces_listed(vec![workspace_entry("/server/next", false)]));
    ui.assert_viewport_contains("/server/next");
    ui.key(key(KeyCode::Enter));
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::MoveWorkspace { .. })));
    ui.deliver_result(workspace_moved("/server/next"));
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::ResumeSession { .. })));
    ui.deliver_result(CommandResult::ResumeSession {
        session_id: "test-session".into(),
        result: Ok(acp::ResumeSessionResponse::new()),
    });
    assert!(
        ui.take_commands()
            .iter()
            .any(|command| matches!(command, Command::Agent(AgentCommand::FetchWorkspaceStatus { .. }))),
        "workspace move fetches agent-side status after resume"
    );
    ui.settle_tasks();
    ui.assert_viewport_contains("/server/next");
}

fn make_ui_with_workspace_move() -> TestUi {
    TestUiBuilder::new().workspace_move().build()
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
    let _ = ui.next_agent_command().unwrap();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));

    assert!(ui.app().waiting_for_response());
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
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));

    let list_cmd = ui.next_agent_command().unwrap();
    assert!(matches!(list_cmd, AgentCommand::ListWorkspaces { .. }));

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));

    ui.draw();
    let viewport = ui.viewport_text();
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("another move is in progress"), "{viewport}");
}

#[test]
fn workspace_list_send_failure_resets_state() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(CommandResult::WorkspacesListed(Err("send failed".into())));

    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));
    ui.draw();
    let viewport = ui.viewport_text();
    let joined = viewport.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(joined.contains("Failed to list workspaces"), "{viewport}");
}

#[test]
fn workspace_list_failed_event_resets_state() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspace_list_failed("network error"));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

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
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
        workspace_entry("/tmp/sandbox", false),
    ]));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::PickingWorkspace));
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
    let _ = ui.next_agent_command().unwrap();
    ui.deliver_result(workspaces_listed(vec![workspace_entry("/tmp/sandbox", false)]));
    assert!(ui.app().has_modal());
    assert_ctrl_c_exits(&mut ui);
}

#[test]
fn workspace_picker_shows_empty_state_when_no_workspaces() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::PickingWorkspace));
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
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::PickingWorkspace));
    assert!(ui.app().has_modal());

    ui.key(key(KeyCode::Esc));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));
    assert!(!ui.app().has_modal());
}

#[test]
fn workspace_picker_enter_selects_existing_workspace() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));
    assert!(!ui.app().has_modal());

    let cmd = ui.next_agent_command().unwrap();
    match cmd {
        AgentCommand::MoveWorkspace { session_id, target } => {
            assert_eq!(session_id, "test-session");
            match target {
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
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

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
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

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
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![workspace_entry("/home/user/code/current", true)]));

    ui.key(key(KeyCode::Enter));
    ui.paste("my-new-workspace");
    assert!(ui.app().composer().text().is_empty(), "paste must belong to the workspace editor");
    ui.key(key(KeyCode::Enter));

    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));
    assert!(!ui.app().has_modal());

    let cmd = ui.next_agent_command().unwrap();
    match cmd {
        AgentCommand::MoveWorkspace { session_id, target } => {
            assert_eq!(session_id, "test-session");
            match target {
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
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
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
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));

    let cmd = ui.next_agent_command().unwrap();
    assert!(matches!(cmd, AgentCommand::MoveWorkspace { .. }));

    ui.deliver_result(workspace_moved("/home/user/code/other"));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::LoadingWorkspaceSession { .. }));

    let load_cmd = ui.next_agent_command().unwrap();
    match load_cmd {
        AgentCommand::ResumeSession { session_id, cwd } => {
            assert_eq!(session_id.0.as_ref(), "test-session");
            assert_eq!(cwd, std::path::Path::new("/home/user/code/other"));
        }
        other => panic!("expected LoadSession, got {other:?}"),
    }
    assert!(
        matches!(ui.next_agent_command(), Some(AgentCommand::FetchWorkspaceStatus { .. })),
        "workspace move refreshes agent-side status after resume"
    );

    ui.type_text("/clear");
    ui.key(key(KeyCode::Tab));
    assert!(ui.next_agent_command().is_none(), "workspace replay still owns the transition");
    ui.submit("wait for replay");
    assert!(ui.next_agent_command().is_none());
    ui.deliver_result(session_loaded("test-session", Vec::new()));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));
    ui.submit("ready");
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::Prompt { .. })));
}

#[test]
fn workspace_move_success_replays_loaded_session_updates() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspace_moved("/home/user/code/other"));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(session_update_for("test-session", user_message_chunk("buffered-message")));
    ui.deliver_result(session_loaded("test-session", vec![]));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.lines().any(|l| l.contains("buffered-message")), "{viewport}");
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert_eq!(joined.matches("Moved to /home/user/code/other").count(), 1, "{viewport}");

    ui.begin_resume("another-session", "/tmp");
    ui.deliver_result(session_loaded("another-session", Vec::new()));
    ui.assert_viewport_not_contains("Moved to");
    ui.assert_viewport_not_contains("buffered-message");
}

#[test]
fn workspace_move_load_session_failure_recovers() {
    let mut ui = make_ui_with_workspace_move();
    ui.submit("keep this transcript");
    ui.next_agent_command().unwrap();
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspace_moved("/home/user/code/other"));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::LoadingWorkspaceSession { .. }));
    let _ = ui.next_agent_command().unwrap();
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(CommandResult::ResumeSession {
        session_id: "test-session".into(),
        result: Err("send failed".into()),
    });
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

    ui.draw();
    let viewport = ui.viewport_text();
    let joined = viewport.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(joined.contains("Failed to resume session"), "{viewport}");
    ui.assert_conversation_not_contains("keep this transcript");
    assert_eq!(ui.app().session_id().0.as_ref(), "test-session");
    ui.submit("retry after failed reload");
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::Prompt { .. })));
}

#[test]
fn workspace_move_server_side_load_failure_recovers() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspace_moved("/home/user/code/other"));
    let _ = ui.next_agent_command().unwrap();
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::LoadingWorkspaceSession { .. }));

    ui.deliver_result(session_load_failed("internal error"));
    assert!(
        matches!(ui.app().foreground_operation(), ForegroundOperation::Idle),
        "a load that fails on the server should stop the loading indicator"
    );

    ui.acp_event(session_update_for("test-session", user_message_chunk("after the failure")));
    ui.draw();
    let viewport = ui.viewport_text();
    assert!(
        viewport.lines().any(|l| l.contains("after the failure")),
        "updates for the current session should still render after the load fails:\n{viewport}"
    );
}

#[test]
fn workspace_move_failed_event_resets_state() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    let _ = ui.next_agent_command().unwrap();
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));

    ui.deliver_result(workspace_move_failed("permission denied"));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

    ui.draw();
    let viewport = ui.viewport_text();
    assert!(viewport.lines().any(|l| l.contains("Workspace move failed")), "{viewport}");
    assert!(viewport.lines().any(|l| l.contains("permission denied")), "{viewport}");
}

#[test]
fn workspace_move_send_failure_resets_state() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));

    ui.key(key(KeyCode::Enter));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(CommandResult::WorkspaceMoved(Err("send failed".into())));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

    ui.draw();
    let viewport = ui.viewport_text();
    let joined = viewport.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(joined.contains("Workspace move failed"), "{viewport}");
}

#[test]
fn workspace_picker_renders_on_narrow_terminal() {
    let mut ui = make_ui_with_workspace_move();

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
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
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    assert!(ui.app().has_modal());

    ui.acp_event(AcpEvent::ConnectionClosed);
    assert!(!ui.app().has_modal());
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));
}
