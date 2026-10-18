use super::support::*;

fn sub_agent_tool_call(parent_id: &str, task_id: &str, agent: &str, tool_id: &str, name: &str, args: &str) -> AcpEvent {
    let raw_input: serde_json::Value = serde_json::from_str(args).expect("tool arguments are JSON");
    sub_agent_tool_update(
        parent_id,
        task_id,
        agent,
        acp::ToolCallUpdate::new(tool_id.to_string()).title(name).name(name).raw_input(raw_input),
    )
}

fn sub_agent_tool_completed(parent_id: &str, task_id: &str, agent: &str, tool_id: &str) -> AcpEvent {
    sub_agent_tool_update(
        parent_id,
        task_id,
        agent,
        acp::ToolCallUpdate::new(tool_id.to_string()).status(acp::ToolCallStatus::Completed),
    )
}

fn sub_agent_done(parent_id: &str, task_id: &str, agent: &str) -> AcpEvent {
    sub_agent_progress(parent_id, task_id, agent, SubAgentEvent::Done)
}

#[test]
fn spawn_tool_seals_only_after_its_sub_agents_finish() {
    let mut app = TestUi::new();
    let tool_state = |app: &TestUi| {
        app.app()
            .conversation_items()
            .iter()
            .find_map(|item| matches!(item.content(), ConversationContent::Tool(_)).then(|| item.state()))
            .expect("the parent tool item must exist")
    };

    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));
    assert_eq!(
        tool_state(&app),
        ItemState::Open,
        "a background spawn completes before its agents report, so it must stay open"
    );

    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    assert_eq!(tool_state(&app), ItemState::Open, "an active sub-agent keeps the parent redrawable");

    app.acp_event(sub_agent_tool_completed("parent-1", "task-a", "explorer", "c1"));
    app.acp_event(sub_agent_done("parent-1", "task-a", "explorer"));

    assert_eq!(tool_state(&app), ItemState::Sealed, "rendering is final once every sub-agent has finished");
}

#[test]
fn sub_agent_parent_stays_live_while_child_running() {
    let mut app = TestUi::new();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));

    // parent completed, no sub-agents → should drain
    app.acp_event(text_chunk("Done"));
    app.complete_prompt(acp::StopReason::EndTurn);
    assert!(
        app.app().conversation_items().iter().any(|item| {
            matches!(item.content(), ConversationContent::Tool(tool) if tool.title() == "spawn_subagent")
        })
    );
}

#[test]
fn running_sub_agent_keeps_agent_busy_after_its_spawn_completes() {
    let mut app = TestUi::new();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));
    assert!(!app.app().is_agent_busy(), "a completed spawn with no sub-agents is idle");

    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", r#"{"pattern":"test"}"#));
    assert!(app.app().is_agent_busy());

    app.acp_event(sub_agent_tool_completed("parent-1", "task-a", "explorer", "c1"));
    app.acp_event(sub_agent_done("parent-1", "task-a", "explorer"));
    assert!(!app.app().is_agent_busy());
}

#[test]
fn sub_agent_context_cleared_removes_state() {
    let mut app = TestUi::new();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    assert!(app.app().is_agent_busy());

    app.acp_event(context_cleared());

    assert!(!app.app().is_agent_busy());
}

#[test]
fn sub_agent_progress_and_clears_for_another_session_are_ignored() {
    let mut app = TestUi::new();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    let other_session = SessionId::new("other-session");

    app.acp_event(AcpEvent::SubAgentProgress(SubAgentProgressParams {
        session_id: other_session.clone(),
        parent_tool_id: "parent-1".to_string(),
        task_id: "task-a".to_string(),
        agent_name: "explorer".to_string(),
        event: SubAgentEvent::Done,
    }));
    app.acp_event(AcpEvent::ContextCleared(ContextClearedParams { session_id: other_session }));

    let items = app.app().conversation_items();
    assert_eq!(items.len(), 1, "another session's clear leaves this conversation alone");
    assert!(matches!(items[0].content(), ConversationContent::Tool(tool) if tool.sub_agents.is_empty()));
}

#[test]
fn sub_agent_wants_tick_while_running() {
    let mut app = TestUi::new();
    app.submit("explore");
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    assert!(app.app().wants_tick());

    app.acp_event(sub_agent_done("parent-1", "task-a", "explorer"));
    app.complete_prompt(acp::StopReason::EndTurn);

    // after finalization, sub-agent tools are finalized
    assert!(!app.app().wants_tick());
}

#[test]
fn sub_agent_renders_tree_guides_in_viewport() {
    let mut ui = TestUi::new();

    // Start a parent tool call and enter prompt mode
    ui.acp_event(tool_call("parent-1", "spawn_subagent"));
    ui.acp_event(tool_completed("parent-1"));

    // Add sub-agents with child tools
    ui.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", r#"{"pattern":"test"}"#));
    ui.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c2", "read", r#"{"path":"src/main.rs"}"#));

    // Tree guides should be visible for the sub-agent
    ui.assert_viewport_contains("explorer");
}

#[test]
fn completed_sub_agent_bash_tool_renders_a_highlighted_command() {
    let mut ui = TestUi::with_dimensions(100, 20);
    ui.acp_event(tool_call("parent-1", "spawn_subagent"));
    ui.acp_event(tool_completed("parent-1"));
    ui.acp_event(sub_agent_tool_call(
        "parent-1",
        "task-a",
        "builder",
        "bash-1",
        "coding__bash",
        r#"{"command":"if true; then echo $HOME; fi"}"#,
    ));
    ui.acp_event(sub_agent_tool_completed("parent-1", "task-a", "builder", "bash-1"));

    assert_shell_command_highlighted(&mut ui, "bash", "if true; then echo $HOME; fi");
}

#[test]
fn sub_agent_drain_includes_sub_agents_in_history_items() {
    let mut app = TestUi::new();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));

    // Add, complete, and mark done a sub-agent
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    app.acp_event(sub_agent_tool_completed("parent-1", "task-a", "explorer", "c1"));
    app.acp_event(sub_agent_done("parent-1", "task-a", "explorer"));

    // End prompt to finalize everything
    app.acp_event(text_chunk("Done"));
    app.complete_prompt(acp::StopReason::EndTurn);

    let tool_item = app.app().conversation_items().iter().find_map(|item| match item.content() {
        ConversationContent::Tool(tool) if tool.title() == "spawn_subagent" => Some(tool),
        _ => None,
    });

    assert!(tool_item.is_some(), "parent tool should be in the conversation");
    let sub_agents = &tool_item.unwrap().sub_agents;
    assert_eq!(sub_agents.len(), 1);
    assert_eq!(sub_agents[0].agent_name, "explorer");
    assert!(sub_agents[0].done);
    assert_eq!(sub_agents[0].tool_calls.len(), 1);
    assert_eq!(sub_agents[0].tool_calls[0].title(), "grep");
}

#[test]
fn sub_agent_prompt_cancelled_finalizes_sub_agents() {
    let mut app = TestUi::new();
    app.submit("explore");
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    app.complete_prompt(acp::StopReason::Cancelled);

    assert!(!app.app().wants_tick());
}

#[test]
fn sub_agent_multiple_sub_agents_per_parent() {
    let mut ui = TestUi::new();
    ui.acp_event(tool_call("parent-1", "spawn_subagent"));
    ui.acp_event(tool_completed("parent-1"));

    ui.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    ui.acp_event(sub_agent_tool_call("parent-1", "task-b", "builder", "c2", "write", "{}"));

    // Both agent names should be visible
    ui.assert_viewport_contains("explorer");
    ui.assert_viewport_contains("builder");
}
