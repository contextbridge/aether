use super::support::*;

fn sub_agent_tool_call(session: &str, tool_id: &str, name: &str, args: &str) -> AcpEvent {
    let raw_input: serde_json::Value = serde_json::from_str(args).expect("tool arguments are JSON");
    sub_agent_tool_update(
        session,
        acp::ToolCallUpdate::new(tool_id.to_string()).title(name).name(name).raw_input(raw_input),
    )
}

fn sub_agent_tool_completed(session: &str, tool_id: &str) -> AcpEvent {
    sub_agent_tool_update(session, acp::ToolCallUpdate::new(tool_id.to_string()).status(acp::ToolCallStatus::Completed))
}

fn spawn_tool(app: &TestUi) -> &ToolCall {
    app.app()
        .conversation_items()
        .iter()
        .find_map(|item| match item.content() {
            ConversationContent::Tool(tool) if tool.title() == "spawn_subagent" => Some(tool),
            _ => None,
        })
        .expect("the spawn tool must exist")
}

#[test]
fn running_sub_agent_keeps_agent_busy_after_its_spawn_completes() {
    let mut app = TestUi::new();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));
    assert!(!app.app().is_agent_busy(), "a completed spawn with no sub-agents is idle");

    app.acp_event(sub_agent_started("parent-1", "child-a", "explorer"));
    app.acp_event(sub_agent_tool_call("child-a", "c1", "grep", r#"{"pattern":"test"}"#));
    assert!(app.app().is_agent_busy());

    app.acp_event(sub_agent_tool_completed("child-a", "c1"));
    app.acp_event(sub_agent_finished("parent-1", "child-a", "explorer", acp::StopReason::EndTurn));
    assert!(!app.app().is_agent_busy());
}

#[test]
fn a_sub_agent_finishes_when_its_own_session_goes_idle() {
    let mut app = TestUi::new();
    app.submit("explore");
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(sub_agent_started("parent-1", "child-a", "explorer"));

    app.acp_event(idle_notification("child-a", Some(acp::StopReason::EndTurn)).into());

    assert_eq!(spawn_tool(&app).sub_agents[0].status(), Some(ToolStatus::Success));
    assert!(app.app().is_agent_busy(), "a sub-agent going idle does not end the parent's turn");
}

#[test]
fn a_failed_sub_agent_renders_as_failed() {
    let mut app = TestUi::new();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));
    app.acp_event(sub_agent_started("parent-1", "child-a", "explorer"));
    app.acp_event(sub_agent_finished("parent-1", "child-a", "explorer", acp::ErrorStopReason::new().into()));

    app.assert_viewport_contains("✗ explorer");
}

#[test]
fn sub_agents_and_clears_for_another_session_are_ignored() {
    let mut app = TestUi::new();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    let other_session = SessionId::new("other-session");

    app.acp_event(
        announced(other_session.clone(), "parent-1", acp::SubagentUpdate::new("child-a").state(running_state())).into(),
    );
    app.acp_event(sub_agent_tool_call("child-a", "c1", "grep", "{}"));
    app.acp_event(AcpEvent::ContextCleared(ContextClearedParams { session_id: other_session }));

    let items = app.app().conversation_items();
    assert_eq!(items.len(), 1, "another session's sub-agent and clear leave this conversation alone");
    assert_eq!(spawn_tool(&app).sub_agents, []);
}

#[test]
fn sub_agent_wants_tick_while_running() {
    let mut app = TestUi::new();
    app.submit("explore");
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));
    app.acp_event(sub_agent_started("parent-1", "child-a", "explorer"));
    app.acp_event(sub_agent_tool_call("child-a", "c1", "grep", "{}"));

    assert!(app.app().wants_tick());

    app.acp_event(sub_agent_finished("parent-1", "child-a", "explorer", acp::StopReason::EndTurn));
    app.complete_prompt(acp::StopReason::EndTurn);

    assert!(!app.app().wants_tick());
}

#[test]
fn sub_agent_renders_tree_guides_in_viewport() {
    let mut ui = TestUi::new();
    ui.acp_event(tool_call("parent-1", "spawn_subagent"));
    ui.acp_event(tool_completed("parent-1"));

    ui.acp_event(sub_agent_started("parent-1", "child-a", "explorer"));
    ui.acp_event(sub_agent_tool_call("child-a", "c1", "grep", r#"{"pattern":"test"}"#));
    ui.acp_event(sub_agent_tool_call("child-a", "c2", "read", r#"{"path":"src/main.rs"}"#));

    ui.assert_viewport_contains("explorer");
}

#[test]
fn completed_sub_agent_bash_tool_renders_a_highlighted_command() {
    let mut ui = TestUi::with_dimensions(100, 20);
    ui.acp_event(tool_call("parent-1", "spawn_subagent"));
    ui.acp_event(tool_completed("parent-1"));
    ui.acp_event(sub_agent_started("parent-1", "child-a", "builder"));
    ui.acp_event(sub_agent_tool_call(
        "child-a",
        "bash-1",
        "coding__bash",
        r#"{"command":"if true; then echo $HOME; fi"}"#,
    ));
    ui.acp_event(sub_agent_tool_completed("child-a", "bash-1"));

    assert_shell_command_highlighted(&mut ui, "bash", "if true; then echo $HOME; fi");
}

#[test]
fn a_cancelled_prompt_stops_ticking_for_its_sub_agents() {
    let mut app = TestUi::new();
    app.submit("explore");
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(sub_agent_started("parent-1", "child-a", "explorer"));
    app.acp_event(sub_agent_tool_call("child-a", "c1", "grep", "{}"));

    app.complete_prompt(acp::StopReason::Cancelled);

    assert!(!app.app().wants_tick());
}

#[test]
fn a_replayed_running_sub_agent_is_unconfirmed_once_the_resume_completes() {
    let mut app = TestUi::new();
    app.begin_resume("saved", "/tmp/saved");
    let spawn = acp::ToolCallUpdate::new("parent-1").title("spawn_subagent").status(acp::ToolCallStatus::Completed);
    app.acp_event(acp::UpdateSessionNotification::new("saved", acp::SessionUpdate::ToolCallUpdate(spawn)).into());
    app.acp_event(
        announced("saved", "parent-1", acp::SubagentUpdate::new("child-a").title("explorer").state(running_state()))
            .into(),
    );
    assert!(app.app().wants_tick(), "replayed history still looks live until the resume completes");

    app.deliver_result(CommandResult::ResumeSession {
        session_id: "saved".into(),
        result: Ok(acp::ResumeSessionResponse::new()),
    });

    assert_eq!(spawn_tool(&app).sub_agents[0].status(), None);
    assert!(!app.app().wants_tick());
    app.assert_viewport_contains("? explorer");
}

#[test]
fn sub_agent_multiple_sub_agents_per_parent() {
    let mut ui = TestUi::new();
    ui.acp_event(tool_call("parent-1", "spawn_subagent"));
    ui.acp_event(tool_completed("parent-1"));

    ui.acp_event(sub_agent_started("parent-1", "child-a", "explorer"));
    ui.acp_event(sub_agent_tool_call("child-a", "c1", "grep", "{}"));
    ui.acp_event(sub_agent_started("parent-1", "child-b", "builder"));
    ui.acp_event(sub_agent_tool_call("child-b", "c2", "write", "{}"));

    ui.assert_viewport_contains("explorer");
    ui.assert_viewport_contains("builder");
}
