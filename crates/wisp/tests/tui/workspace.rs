use super::support::*;

#[test]
fn remote_workspace_move_keeps_paths_server_side() {
    let mut ui = TestUiBuilder::new().remote_workspace().workspace_move().dimensions(120, 24).build();
    open_workspace_picker(&mut ui, vec![workspace_entry("/server/next", false)]);
    ui.assert_viewport_contains("/server/next");
    ui.key(key(KeyCode::Enter));
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::MoveWorkspace { .. })));
    ui.deliver_result(workspace_moved("/server/next"));
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::ResumeSession { .. })));
    ui.deliver_result(CommandResult::ResumeSession {
        session_id: "test-session".into(),
        result: Ok(acp::ResumeSessionResponse::new()),
    });
    let status_fetches =
        ui.count_commands(|command| matches!(command, Command::Agent(AgentCommand::FetchWorkspaceStatus { .. })));
    assert_eq!(status_fetches, 1, "workspace move fetches agent-side status after resume");
    ui.settle_tasks();
    ui.assert_viewport_contains("/server/next");
}

#[test]
fn workspace_move_command_hidden_without_capability() {
    let mut ui = TestUi::new();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.assert_viewport_contains("/clear");
    ui.assert_viewport_not_contains("/move");
}

#[test]
fn workspace_move_command_visible_with_capability() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    ui.key(key(KeyCode::Char('/')));
    assert!(ui.app().composer().has_completion());

    ui.assert_viewport_contains("/move");
}

#[test]
fn workspace_move_command_rejected_when_prompt_in_flight() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    ui.submit("hello");
    let _ = ui.next_agent_command().unwrap();

    ui.run_command("/move");

    assert!(ui.app().waiting_for_response());
    let viewport = ui.viewport_text();
    assert!(viewport.lines().any(|l| l.contains("Cannot move") && l.contains("workspace")), "{viewport}");
    ui.assert_viewport_contains("prompt is running");
}

#[test]
fn workspace_move_command_rejected_when_already_listing() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    ui.run_command("/move");
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));

    let list_cmd = ui.next_agent_command().unwrap();
    assert!(matches!(list_cmd, AgentCommand::ListWorkspaces { .. }));

    ui.run_command("/move");
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));

    let viewport = ui.viewport_text();
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("another move is in progress"), "{viewport}");
}

#[test]
fn workspace_list_send_failure_resets_state() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    ui.run_command("/move");
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(CommandResult::WorkspacesListed(Err("send failed".into())));

    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));
    let viewport = ui.viewport_text();
    let joined = viewport.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(joined.contains("Failed to list workspaces"), "{viewport}");
}

#[test]
fn workspace_list_failed_event_resets_state() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    ui.run_command("/move");
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::ListingWorkspaces));
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(workspace_list_failed("network error"));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

    let viewport = ui.viewport_text();
    let collapsed = viewport.replace('\n', " ");
    let words: Vec<&str> = collapsed.split_whitespace().collect();
    let joined = words.join(" ");
    assert!(joined.contains("Failed to list workspaces: network error"), "{viewport}");
}

#[test]
fn workspace_picker_opens_with_existing_workspaces() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(
        &mut ui,
        vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
            workspace_entry("/tmp/sandbox", false),
        ],
    );
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::PickingWorkspace));
    assert!(ui.app().has_modal());

    ui.assert_viewport_contains("/home/user/code/other");
    ui.assert_viewport_contains("/tmp/sandbox");
    ui.assert_viewport_not_contains("/home/user/code/current");
    ui.assert_viewport_contains("Create new workspace");
}

#[test]
fn double_ctrl_c_exits_over_workspace_picker() {
    let mut ui = TestUiBuilder::new().workspace_move().build();
    open_workspace_picker(&mut ui, vec![workspace_entry("/tmp/sandbox", false)]);
    assert!(ui.app().has_modal());
    assert_ctrl_c_exits(&mut ui);
}

#[test]
fn workspace_picker_shows_empty_state_when_no_workspaces() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::PickingWorkspace));
    assert!(ui.app().has_modal());

    ui.assert_viewport_not_contains("No other workspaces available");
}

#[test]
fn workspace_picker_esc_closes_and_resets_state() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(
        &mut ui,
        vec![workspace_entry("/home/user/code/current", true), workspace_entry("/home/user/code/other", false)],
    );
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::PickingWorkspace));
    assert!(ui.app().has_modal());

    ui.key(key(KeyCode::Esc));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));
    assert!(!ui.app().has_modal());
}

#[test]
fn workspace_picker_enter_selects_existing_workspace() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    move_to_other_workspace(&mut ui);

    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));
    assert!(!ui.app().has_modal());
}

#[test]
fn workspace_picker_enter_selects_create_new_and_shows_naming_mode() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);

    ui.assert_viewport_contains("Create new workspace");

    ui.key(key(KeyCode::Enter));
    ui.assert_viewport_contains("New workspace");
}

#[test]
fn workspace_name_prompt_owns_the_terminal_cursor() {
    let mut ui = TestUiBuilder::new().workspace_move().build();
    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);
    ui.key(key(KeyCode::Enter));

    ui.type_text("proj");
    ui.draw();

    let buffer = ui.backend().buffer();
    let row = row_containing(buffer, "Name: proj").expect("name prompt");
    let text = row_text(buffer, row);
    let column = text[..text.find("Name: proj").unwrap()].width() + "Name: proj".width();
    assert!(ui.backend().cursor_visible(), "the name prompt owns the cursor");
    assert_eq!(ui.backend().cursor_position(), Position::new(u16::try_from(column).unwrap(), row));
}

#[test]
fn workspace_name_scrolls_unicode_text_and_keeps_cursor_visible() {
    let mut ui = TestUiBuilder::new().workspace_move().dimensions(24, 20).build();
    open_workspace_picker(&mut ui, vec![workspace_entry("/workspace/current", true)]);
    ui.key(key(KeyCode::Enter));
    ui.type_text("a界bcdefghijklmnopqrstuvwxyz");
    ui.draw();

    let buffer = ui.backend().buffer();
    let row = row_containing(buffer, "Name:").expect("name prompt");
    let text = row_text(buffer, row);
    assert!(text.contains("xyz"), "{text}");
    assert!(!text.contains("a界"), "the input should scroll: {text}");
    let column = text[..text.find("xyz").unwrap()].width() + 3;
    assert!(ui.backend().cursor_visible());
    assert_eq!(ui.backend().cursor_position(), Position::new(u16::try_from(column).unwrap(), row));

    ui.key(key(KeyCode::Home));
    ui.key(key(KeyCode::Right));
    ui.key(key(KeyCode::Right));
    ui.draw();
    let text = row_text(ui.backend().buffer(), row);
    assert!(text.contains("Name: a界"), "{text}");
    let column = text[..text.find("a界").unwrap()].width() + 3;
    assert_eq!(ui.backend().cursor_position(), Position::new(u16::try_from(column).unwrap(), row));
}

#[test]
fn workspace_naming_new_esc_returns_to_list_mode() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);

    ui.key(key(KeyCode::Enter));
    ui.assert_viewport_contains("New workspace");

    ui.key(key(KeyCode::Esc));
    assert!(ui.app().has_modal());
    ui.assert_viewport_contains("Create new workspace");
}

#[test]
fn workspace_naming_new_enter_with_name_emits_move_target() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(&mut ui, vec![workspace_entry("/home/user/code/current", true)]);

    ui.key(key(KeyCode::Enter));
    ui.paste("my-new-workspace");
    assert!(ui.app().composer().text().is_empty(), "paste must belong to the workspace editor");
    ui.key(key(KeyCode::Enter));

    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));
    assert!(!ui.app().has_modal());

    let cmd = ui.next_agent_command();
    assert!(
        matches!(&cmd, Some(AgentCommand::MoveWorkspace { session_id, target: WorkspaceMoveTarget::New { name } })
            if session_id == "test-session" && name == "my-new-workspace"),
        "{cmd:?}"
    );
}

#[test]
fn workspace_picker_filtering_hides_non_matching() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(
        &mut ui,
        vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/project-a", false),
            workspace_entry("/tmp/test", false),
        ],
    );

    ui.assert_viewport_contains("project-a");
    ui.assert_viewport_contains("/tmp/test");

    // Use a query that only matches one entry
    ui.key(key(KeyCode::Char('j')));
    ui.assert_viewport_contains("project-a");
    ui.assert_viewport_not_contains("/tmp/test");
    ui.assert_viewport_not_contains("Create new");
}

#[test]
fn workspace_move_success_updates_cwd_and_reloads_session() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    move_to_other_workspace(&mut ui);
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));

    ui.deliver_result(workspace_moved("/home/user/code/other"));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::LoadingWorkspaceSession { .. }));

    let load_cmd = ui.next_agent_command();
    assert!(
        matches!(&load_cmd, Some(AgentCommand::ResumeSession { session_id, cwd })
            if session_id.0.as_ref() == "test-session" && cwd == std::path::Path::new("/home/user/code/other")),
        "{load_cmd:?}"
    );
    assert!(
        matches!(ui.next_agent_command(), Some(AgentCommand::FetchWorkspaceStatus { .. })),
        "workspace move refreshes agent-side status after resume"
    );

    ui.run_command("/clear");
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
    let mut ui = TestUiBuilder::new().workspace_move().build();

    move_to_other_workspace(&mut ui);

    ui.deliver_result(workspace_moved("/home/user/code/other"));
    let _ = ui.next_agent_command().unwrap();

    ui.acp_event(session_update_for("test-session", user_message_chunk("buffered-message")));
    ui.deliver_result(session_loaded("test-session", vec![]));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

    ui.assert_viewport_contains("buffered-message");
    let viewport = ui.viewport_text();
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
    let mut ui = TestUiBuilder::new().workspace_move().build();
    ui.submit("keep this transcript");
    ui.next_agent_command().unwrap();
    ui.complete_prompt(acp::StopReason::EndTurn);

    move_to_other_workspace(&mut ui);

    ui.deliver_result(workspace_moved("/home/user/code/other"));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::LoadingWorkspaceSession { .. }));
    let _ = ui.next_agent_command().unwrap();
    let _ = ui.next_agent_command().unwrap();

    ui.deliver_result(CommandResult::ResumeSession {
        session_id: "test-session".into(),
        result: Err("send failed".into()),
    });
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

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
    let mut ui = TestUiBuilder::new().workspace_move().build();

    move_to_other_workspace(&mut ui);

    ui.deliver_result(workspace_moved("/home/user/code/other"));
    let _ = ui.next_agent_command().unwrap();
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::LoadingWorkspaceSession { .. }));

    ui.deliver_result(session_load_failed("internal error"));
    assert!(
        matches!(ui.app().foreground_operation(), ForegroundOperation::Idle),
        "a load that fails on the server should stop the loading indicator"
    );

    ui.acp_event(session_update_for("test-session", user_message_chunk("after the failure")));
    ui.assert_viewport_contains("after the failure");
}

#[test]
fn workspace_move_failed_event_resets_state() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    move_to_other_workspace(&mut ui);
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));

    ui.deliver_result(workspace_move_failed("permission denied"));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

    ui.assert_viewport_contains("Workspace move failed");
    ui.assert_viewport_contains("permission denied");
}

#[test]
fn workspace_move_send_failure_resets_state() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    move_to_other_workspace(&mut ui);
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::MovingWorkspace));

    ui.deliver_result(CommandResult::WorkspaceMoved(Err("send failed".into())));
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));

    let viewport = ui.viewport_text();
    let joined = viewport.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(joined.contains("Workspace move failed"), "{viewport}");
}

#[test]
fn workspace_picker_renders_on_narrow_terminal() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(
        &mut ui,
        vec![workspace_entry("/home/user/code/current", true), workspace_entry("/home/user/code/project-a", false)],
    );

    ui.assert_viewport_contains("project-a");
}

#[test]
fn workspace_move_picker_closes_when_connection_closes() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    open_workspace_picker(
        &mut ui,
        vec![workspace_entry("/home/user/code/current", true), workspace_entry("/home/user/code/other", false)],
    );
    assert!(ui.app().has_modal());

    ui.acp_event(AcpEvent::ConnectionClosed);
    assert!(!ui.app().has_modal());
    assert!(matches!(ui.app().foreground_operation(), ForegroundOperation::Idle));
}
