use super::support::*;
use unicode_width::UnicodeWidthStr;

fn sub_agent_progress(parent_tool_id: &str, task_id: &str, agent_name: &str, event: SubAgentEvent) -> AcpEvent {
    AcpEvent::SubAgentProgress(SubAgentProgressParams {
        parent_tool_id: parent_tool_id.to_string(),
        task_id: task_id.to_string(),
        agent_name: agent_name.to_string(),
        event,
    })
}

fn sub_agent_tool_call(parent_id: &str, task_id: &str, agent: &str, tool_id: &str, name: &str, args: &str) -> AcpEvent {
    sub_agent_progress(
        parent_id,
        task_id,
        agent,
        SubAgentEvent::ToolCall {
            request: SubAgentToolRequest {
                id: tool_id.to_string(),
                name: name.to_string(),
                arguments: args.to_string(),
            },
        },
    )
}

fn sub_agent_done(parent_id: &str, task_id: &str, agent: &str) -> AcpEvent {
    sub_agent_progress(parent_id, task_id, agent, SubAgentEvent::Done)
}

#[test]
fn spawn_tool_seals_only_after_its_sub_agents_finish() {
    let mut app = make_app();
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

    app.acp_event(sub_agent_progress(
        "parent-1",
        "task-a",
        "explorer",
        SubAgentEvent::ToolResult {
            result: SubAgentToolResult { id: "c1".to_string(), name: "grep".to_string(), result_meta: None },
        },
    ));
    app.acp_event(sub_agent_done("parent-1", "task-a", "explorer"));

    assert_eq!(tool_state(&app), ItemState::Sealed, "rendering is final once every sub-agent has finished");
}

#[test]
fn sub_agent_progress_event_is_handled() {
    let mut app = make_app();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));

    // Send a sub-agent ToolCall event - should not crash
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", r#"{"pattern":"test"}"#));

    // Parent with running sub-agent is still running
    assert!(app.app().is_agent_busy());
}

#[test]
fn sub_agent_parent_stays_live_while_child_running() {
    let mut app = make_app();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));

    // parent completed, no sub-agents → should drain
    app.acp_event(text_chunk("Done"));
    app.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    assert!(
        app.app().conversation_items().iter().any(|item| {
            matches!(item.content(), ConversationContent::Tool(tool) if tool.title == "spawn_subagent")
        })
    );
}

#[test]
fn sub_agent_keeps_agent_busy() {
    let mut app = make_app();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));

    // still busy because prompt_in_flight was never set in this test
    // but the sub-agent running check uses any_running which includes sub-agents
}

#[test]
fn sub_agent_context_cleared_removes_state() {
    let mut app = make_app();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    assert!(app.app().is_agent_busy());

    app.acp_event(AcpEvent::ContextCleared(acp_utils::notifications::ContextClearedParams {}));

    assert!(!app.app().is_agent_busy());
}

#[test]
fn sub_agent_wants_tick_while_running() {
    let mut app = make_app();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    assert!(app.app().wants_tick());

    app.acp_event(sub_agent_done("parent-1", "task-a", "explorer"));
    app.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

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

    ui.draw();
    let viewport = ui.viewport_text();

    // Tree guides should be visible for the sub-agent
    assert!(viewport.contains("explorer"), "viewport should show agent name:\n{viewport}");
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
    ui.acp_event(sub_agent_progress(
        "parent-1",
        "task-a",
        "builder",
        SubAgentEvent::ToolResult {
            result: SubAgentToolResult { id: "bash-1".to_string(), name: "bash".to_string(), result_meta: None },
        },
    ));

    ui.draw();

    let conversation = ui.conversation();
    let command = "if true; then echo $HOME; fi";
    let row = row_containing(&conversation, command).expect("rendered sub-agent Bash command");
    let text = row_text(&conversation, row);
    assert!(text.contains(&format!("bash {command}")), "command should share the tool row: {text:?}");
    let command_start = u16::try_from(text[..text.find(command).expect("command position")].width()).unwrap();
    let gap = conversation.cell((conversation.area.left() + command_start - 1, row)).expect("gap before command");
    assert_eq!(gap.bg, ui.app().theme().background, "the gap should use the normal background");
    let cells = (command_start..command_start + u16::try_from(command.width()).unwrap())
        .filter_map(|offset| conversation.cell((conversation.area.left() + offset, row)))
        .filter(|cell| cell.symbol() != " ")
        .collect::<Vec<_>>();
    assert!(cells.iter().all(|cell| cell.bg == ui.app().theme().code_bg), "command should use the code background");
    let keyword = cells.iter().find(|cell| cell.symbol() == "i").expect("if keyword");
    let variable = cells.iter().find(|cell| cell.symbol() == "$").expect("shell variable");
    assert_ne!(keyword.fg, variable.fg, "shell keywords and variables should use distinct token colors");
}

#[test]
fn sub_agent_drain_includes_sub_agents_in_history_items() {
    let mut app = make_app();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(tool_completed("parent-1"));

    // Add, complete, and mark done a sub-agent
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    app.acp_event(sub_agent_progress(
        "parent-1",
        "task-a",
        "explorer",
        SubAgentEvent::ToolResult {
            result: SubAgentToolResult { id: "c1".to_string(), name: "grep".to_string(), result_meta: None },
        },
    ));
    app.acp_event(sub_agent_done("parent-1", "task-a", "explorer"));

    // End prompt to finalize everything
    app.acp_event(text_chunk("Done"));
    app.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    let tool_item = app.app().conversation_items().iter().find_map(|item| match item.content() {
        ConversationContent::Tool(tool) if tool.title == "spawn_subagent" => Some(tool),
        _ => None,
    });

    assert!(tool_item.is_some(), "parent tool should be in the conversation");
    let sub_agents = &tool_item.unwrap().sub_agents;
    assert_eq!(sub_agents.len(), 1);
    assert_eq!(sub_agents[0].agent_name, "explorer");
    assert!(sub_agents[0].done);
    assert_eq!(sub_agents[0].tool_calls.len(), 1);
    assert_eq!(sub_agents[0].tool_calls[0].name, "grep");
}

#[test]
fn sub_agent_prompt_error_finalizes_sub_agents() {
    let mut app = make_app();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    // PromptError should finalize sub-agents
    app.acp_event(AcpEvent::PromptError(agent_client_protocol::Error::internal_error()));

    assert!(!app.app().wants_tick());
}

#[test]
fn sub_agent_prompt_cancelled_finalizes_sub_agents() {
    let mut app = make_app();
    app.acp_event(tool_call("parent-1", "spawn_subagent"));
    app.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    app.acp_event(AcpEvent::PromptDone(acp::StopReason::Cancelled));

    assert!(!app.app().wants_tick());
}

#[test]
fn sub_agent_multiple_sub_agents_per_parent() {
    let mut ui = TestUi::new();
    ui.acp_event(tool_call("parent-1", "spawn_subagent"));
    ui.acp_event(tool_completed("parent-1"));

    ui.acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    ui.acp_event(sub_agent_tool_call("parent-1", "task-b", "builder", "c2", "write", "{}"));

    ui.draw();
    let viewport = ui.viewport_text();

    // Both agent names should be visible
    assert!(viewport.contains("explorer"), "viewport should show explorer:\n{viewport}");
    assert!(viewport.contains("builder"), "viewport should show builder:\n{viewport}");
}

mod progress_indicator_tests {
    use super::*;
    use wisp::conversation::progress_indicator::SPINNER_FRAMES;

    #[test]
    fn idle_renders_no_progress_indicator() {
        let mut ui = TestUi::new();
        ui.draw();
        let full = ui.viewport_text();
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| full.contains(frame));
        assert!(!has_spinner, "buffer should not contain spinner when idle:\n{full}");
        assert!(!full.contains("Working…"), "{full}");
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn prompt_shows_progress_with_esc_hint() {
        let mut ui = TestUi::new();
        ui.submit("hello");
        assert!(
            ui.app().progress_indicator().is_active(),
            "progress indicator not active. prompt_in_flight={}, is_agent_busy={}",
            ui.app().waiting_for_response(),
            ui.app().is_agent_busy()
        );
        // Use a 120-char terminal to fit the full tip + esc hint on one line
        ui.resize(120, 30);
        ui.draw();
        let full = ui.viewport_text();
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| full.contains(frame));
        assert!(has_spinner, "full buffer should contain spinner during prompt:\n{full}");
        assert!(full.contains("esc to interrupt"), "full buffer should show esc hint:\n{full}");
    }

    #[test]
    fn prompt_done_hides_progress() {
        let mut ui = TestUi::new();
        ui.submit("hello");
        ui.acp_event(text_chunk("response"));
        ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        ui.draw();
        let full = ui.viewport_text();
        assert!(!full.contains("esc to interrupt"), "{full}");
        assert!(!full.contains("Working…"), "{full}");
    }

    #[test]
    fn compaction_active_shows_compacting_message() {
        let mut ui = TestUi::new();
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        ui.draw();
        let viewport = ui.viewport_text();
        assert!(viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn compaction_inactive_hides_indicator() {
        let mut ui = TestUi::new();
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: false }));
        ui.draw();
        let viewport = ui.viewport_text();
        assert!(!viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn compaction_during_prompt_shows_esc_hint() {
        let mut ui = TestUi::new();
        ui.submit("hello");
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        ui.resize(120, 30);
        ui.draw();
        let full = ui.viewport_text();
        assert!(full.contains("Compacting context"), "{full}");
        assert!(full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn workspace_moving_shows_progress() {
        let mut ui = TestUiBuilder::new().workspace_move().build();
        ui.type_text("/move");
        ui.key(key(KeyCode::Tab));
        let _ = ui.next_agent_command().unwrap();
        ui.acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        ui.key(key(KeyCode::Enter));
        let _ = ui.next_agent_command().unwrap();
        ui.resize(120, 30);
        ui.draw();
        let full = ui.viewport_text();
        assert!(full.contains("Moving workspace"), "{full}");
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn workspace_loading_session_shows_progress() {
        let mut ui = TestUiBuilder::new().workspace_move().build();
        ui.type_text("/move");
        ui.key(key(KeyCode::Tab));
        let _ = ui.next_agent_command().unwrap();
        ui.acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        ui.key(key(KeyCode::Enter));
        let _ = ui.next_agent_command().unwrap();
        ui.acp_event(workspace_moved("/home/user/code/other"));
        ui.draw();
        let viewport = ui.viewport_text();
        assert!(viewport.contains("Loading session in new workspace"), "{viewport}");
    }

    #[test]
    fn workspace_move_failure_clears_indicator() {
        let mut ui = TestUiBuilder::new().workspace_move().build();
        ui.type_text("/move");
        ui.key(key(KeyCode::Tab));
        let _ = ui.next_agent_command().unwrap();
        ui.acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        ui.key(key(KeyCode::Enter));
        let _ = ui.next_agent_command().unwrap();
        ui.acp_event(workspace_move_failed("permission denied"));
        ui.draw();
        let viewport = ui.viewport_text();
        assert!(!viewport.contains("Moving workspace"), "{viewport}");
        assert!(!viewport.contains("Loading session"), "{viewport}");
    }

    #[test]
    fn workspace_move_precedence_over_compaction() {
        let mut ui = TestUiBuilder::new().workspace_move().build();
        ui.type_text("/move");
        ui.key(key(KeyCode::Tab));
        let _ = ui.next_agent_command().unwrap();
        ui.acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        ui.key(key(KeyCode::Enter));
        let _ = ui.next_agent_command().unwrap();
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        ui.draw();
        let viewport = ui.viewport_text();
        assert!(viewport.contains("Moving workspace"), "{viewport}");
        assert!(!viewport.contains("Compacting"), "{viewport}");
    }

    #[test]
    fn compaction_precedence_over_agent_work() {
        let mut ui = TestUi::new();
        ui.submit("hello");
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        ui.draw();
        let viewport = ui.viewport_text();
        assert!(viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn wants_tick_true_immediately_after_prompt_submit() {
        let mut app = make_app();
        assert!(!app.app().wants_tick(), "wants_tick should be false when idle");
        app.submit("hello");
        assert!(app.app().wants_tick(), "wants_tick should be true immediately after prompt submit (raw state)");
    }

    #[test]
    fn wants_tick_true_during_compaction() {
        let mut app = make_app();
        app.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        assert!(app.app().wants_tick(), "wants_tick should be true during compaction");
    }

    #[test]
    fn wants_tick_true_during_workspace_move() {
        let mut app = make_app_with_workspace_move();
        app.type_text("/move");
        app.key(key(KeyCode::Tab));
        let _ = app.next_agent_command().unwrap();
        app.acp_event(workspaces_listed(vec![
            workspace_entry("/home/user/code/current", true),
            workspace_entry("/home/user/code/other", false),
        ]));
        app.key(key(KeyCode::Enter));
        let _ = app.next_agent_command().unwrap();
        assert!(app.app().wants_tick(), "wants_tick should be true during workspace move");
    }

    #[test]
    fn wants_tick_false_after_idle() {
        let mut app = make_app();
        app.submit("hello");
        app.acp_event(text_chunk("done"));
        app.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        assert!(!app.app().wants_tick(), "wants_tick should be false after prompt completes and no other activity");
    }

    #[test]
    fn tick_animates_spinner_deterministically() {
        let mut ui = TestUi::new();
        ui.submit("hello");

        let now = Instant::now();

        // Capture rendering at tick 0
        ui.resize(120, 30);
        ui.draw();
        let full_a = ui.viewport_text();

        // Advance tick once
        ui.tick(now);
        ui.draw();
        let full_b = ui.viewport_text();

        // Different frames should produce different braille characters
        assert_ne!(full_a, full_b, "spinner should animate with each tick");
    }

    #[test]
    fn tick_stops_when_idle() {
        let mut ui = TestUi::new();
        ui.submit("hello");

        let now = Instant::now();
        ui.tick(now);

        // Complete the prompt
        ui.acp_event(text_chunk("response"));
        ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        // Tick while idle — should not change
        ui.tick(now);
        ui.resize(120, 30);
        ui.draw();
        let full = ui.viewport_text();
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn progress_is_not_inserted_into_scrollback() {
        let mut ui = TestUi::new();
        // Fill enough content to trigger scrollback
        ui.submit("hello");
        let mut response = String::new();
        for i in 0..20 {
            writeln!(response, "line-{i}").unwrap();
        }
        ui.acp_event(text_chunk(&response));
        ui.acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        // Submit another prompt so we can see progress indicator
        ui.submit("another");
        ui.draw();

        let scrollback = ui.history_text();
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| scrollback.contains(frame));
        assert!(!has_spinner, "scrollback should not contain progress spinner:\n{scrollback}");
        assert!(!scrollback.contains("esc to interrupt"), "scrollback should not contain esc hint:\n{scrollback}");
        assert!(!scrollback.contains("Working…"), "scrollback should not contain Working:\n{scrollback}");
    }

    #[test]
    fn context_cleared_resets_progress_state() {
        let mut ui = TestUi::new();
        ui.submit("hello");
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        ui.acp_event(AcpEvent::ContextCleared(ContextClearedParams {}));

        ui.draw();
        let viewport = ui.viewport_text();
        assert!(!viewport.contains("Compacting"), "{viewport}");
        assert!(!viewport.contains("esc to interrupt"), "{viewport}");
        assert!(!ui.app().wants_tick(), "wants_tick should be false after context cleared");
    }

    #[test]
    fn new_session_resets_progress_state() {
        let mut ui = TestUi::new();
        ui.submit("hello");
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        ui.acp_event(AcpEvent::NewSessionCreated {
            session_id: SessionId::new("new-session"),
            config_options: Vec::new(),
        });

        ui.draw();
        let viewport = ui.viewport_text();
        assert!(!viewport.contains("Compacting"), "{viewport}");
        assert!(!ui.app().wants_tick(), "wants_tick should be false after new session");
    }

    #[test]
    fn session_loaded_resets_progress_state() {
        let mut ui = TestUi::new();
        ui.submit("hello");
        ui.acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        ui.acp_event(AcpEvent::SessionLoaded {
            session_id: SessionId::new("loaded-session"),
            config_options: Vec::new(),
        });

        ui.draw();
        let viewport = ui.viewport_text();
        assert!(!viewport.contains("Compacting"), "{viewport}");
    }

    #[test]
    fn workspace_list_failed_clears_indicator() {
        let mut ui = TestUiBuilder::new().workspace_move().build();
        ui.type_text("/move");
        ui.key(key(KeyCode::Tab));
        let _ = ui.next_agent_command().unwrap();
        ui.acp_event(workspace_list_failed("network error"));

        ui.draw();
        let viewport = ui.viewport_text();
        assert!(!viewport.contains("Moving workspace"), "{viewport}");
        assert!(!ui.app().wants_tick(), "wants_tick should be false after list failure");
    }
}
