use super::support::*;

fn drain_commands(app: &mut TestUi) {
    let _ = app.take_commands();
}

fn assert_command(ui: &mut TestUi, expected: impl Fn(&AgentCommand) -> bool, label: &str) {
    let command = ui.next_agent_command().unwrap_or_else(|| panic!("{label} should have sent a command"));
    assert!(expected(&command), "{label} sent {command:?}");
}

fn commit_overflowing_reply(ui: &mut TestUi) {
    ui.submit("talk at length");
    assert_command(ui, |c| matches!(c, AgentCommand::Prompt { .. }), "the overflowing reply");
    let mut reply = String::new();
    for index in 0..30 {
        writeln!(reply, "overflow-line-{index}").unwrap();
        reply.push('\n');
    }
    ui.acp_event(text_chunk(&format!("{reply}still streaming")));
    ui.draw();
    assert!(
        ui.history_text().contains("overflow-line-0"),
        "precondition: a prior reply must have been committed to scrollback"
    );
    // Complete the turn so the app is idle and later commands (e.g. /move) are not blocked.
    ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
}

#[test]
fn context_clear_does_not_purge_native_scrollback() {
    let mut ui = TestUi::new();

    commit_overflowing_reply(&mut ui);

    ui.acp_event(AcpEvent::ContextCleared(ContextClearedParams::default()));

    ui.assert_history_contains("overflow-line-0");
}

#[test]
fn clear_command_replaces_conversation_without_purging_native_scrollback() {
    let mut ui = TestUi::new();

    commit_overflowing_reply(&mut ui);

    ui.type_text("/clear");
    ui.key(key(KeyCode::Tab));
    assert_command(&mut ui, |c| matches!(c, AgentCommand::NewSession { .. }), "/clear");
    ui.assert_history_contains("overflow-line-0");

    ui.acp_event(new_session_created("fresh-session", Vec::new()));

    ui.assert_history_contains("overflow-line-0");
}

#[test]
fn session_switch_preserves_native_scrollback_without_purging() {
    let mut ui = TestUi::new();

    commit_overflowing_reply(&mut ui);

    ui.type_text("/resume");
    ui.key(key(KeyCode::Tab));
    assert_command(&mut ui, |c| matches!(c, AgentCommand::ListSessions), "/resume");
    ui.acp_event(sessions_listed(vec![session_info("other", "/tmp/elsewhere", "Other", "2025-01-01T00:00:00Z")]));
    ui.key(key(KeyCode::Enter));
    assert_command(&mut ui, |c| matches!(c, AgentCommand::LoadSession { .. }), "resume");

    ui.assert_history_contains("overflow-line-0");

    ui.acp_event(session_loaded("other", Vec::new()));

    ui.assert_history_contains("overflow-line-0");
}

#[test]
fn successful_workspace_move_does_not_purge_native_scrollback() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    commit_overflowing_reply(&mut ui);

    ui.type_text("/move");
    ui.key(key(KeyCode::Tab));
    assert_command(&mut ui, |c| matches!(c, AgentCommand::ListWorkspaces { .. }), "/move");
    ui.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    ui.key(key(KeyCode::Enter));
    assert_command(&mut ui, |c| matches!(c, AgentCommand::MoveWorkspace { .. }), "move");
    ui.acp_event(workspace_moved("/home/user/code/other"));

    ui.assert_history_contains("overflow-line-0");
}

#[test]
fn an_ordinary_render_does_not_purge() {
    let mut ui = TestUi::new();

    commit_overflowing_reply(&mut ui);
    ui.draw();

    ui.assert_history_contains("overflow-line-0");
}

#[test]
fn a_new_session_request_that_fails_does_not_purge() {
    let mut app = TestUiBuilder::new().build();

    app.type_text("/clear");
    app.key(key(KeyCode::Tab));
    assert!(matches!(app.next_agent_command(), Some(AgentCommand::NewSession { .. })));

    drain_commands(&mut app);
}

#[test]
fn a_workspace_listing_failure_does_not_purge() {
    let mut app = make_app_with_workspace_move();

    app.type_text("/move");
    app.key(key(KeyCode::Tab));
    assert!(matches!(app.next_agent_command(), Some(AgentCommand::ListWorkspaces { .. })));

    app.acp_event(workspace_list_failed("network error"));

    drain_commands(&mut app);
}

#[test]
fn a_workspace_move_failure_does_not_purge() {
    let mut app = make_app_with_workspace_move();

    app.type_text("/move");
    app.key(key(KeyCode::Tab));
    assert!(matches!(app.next_agent_command(), Some(AgentCommand::ListWorkspaces { .. })));
    app.acp_event(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    app.key(key(KeyCode::Enter));
    assert!(matches!(app.next_agent_command(), Some(AgentCommand::MoveWorkspace { .. })));

    app.acp_event(workspace_move_failed("permission denied"));

    drain_commands(&mut app);
}
