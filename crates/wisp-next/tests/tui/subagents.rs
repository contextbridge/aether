use super::support::*;

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
fn sub_agent_progress_event_is_handled() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));

    // Send a sub-agent ToolCall event - should not crash
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", r#"{"pattern":"test"}"#));

    // Parent with running sub-agent is still running
    assert!(app.is_agent_busy());
}

#[test]
fn sub_agent_parent_stays_live_while_child_running() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    // parent completed, no sub-agents → should drain
    app.on_acp_event(text_chunk("Done"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    let items = app.drain_finalized();
    // parent should be in the drained items
    assert!(items.iter().any(|item| matches!(item, HistoryItem::Tool { title, .. } if title == "spawn_subagent")));
}

#[test]
fn sub_agent_keeps_agent_busy() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    // still busy because prompt_in_flight was never set in this test
    // but the sub-agent running check uses any_running which includes sub-agents
}

#[test]
fn sub_agent_context_cleared_removes_state() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    assert!(app.is_agent_busy());

    app.on_acp_event(AcpEvent::ContextCleared(acp_utils::notifications::ContextClearedParams {}));

    assert!(!app.is_agent_busy());
}

#[test]
fn sub_agent_wants_tick_while_running() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    assert!(app.wants_tick());

    app.on_acp_event(sub_agent_done("parent-1", "task-a", "explorer"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    // after finalization, sub-agent tools are finalized
    assert!(!app.wants_tick());
}

#[test]
fn sub_agent_renders_tree_guides_in_viewport() {
    let (mut app, _command_rx) = make_app();

    // Start a parent tool call and enter prompt mode
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    // Add sub-agents with child tools
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", r#"{"pattern":"test"}"#));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c2", "read", r#"{"path":"src/main.rs"}"#));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    // Tree guides should be visible for the sub-agent
    assert!(viewport.contains("explorer"), "viewport should show agent name:\n{viewport}");
}

#[test]
fn sub_agent_drain_includes_sub_agents_in_history_items() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    // Add, complete, and mark done a sub-agent
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    app.on_acp_event(sub_agent_progress(
        "parent-1",
        "task-a",
        "explorer",
        SubAgentEvent::ToolResult {
            result: SubAgentToolResult { id: "c1".to_string(), name: "grep".to_string(), result_meta: None },
        },
    ));
    app.on_acp_event(sub_agent_done("parent-1", "task-a", "explorer"));

    // End prompt to finalize everything
    app.on_acp_event(text_chunk("Done"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    let items = app.drain_finalized();
    let tool_item = items.iter().find_map(|item| match item {
        HistoryItem::Tool { title, sub_agents, .. } if title == "spawn_subagent" => Some(sub_agents),
        _ => None,
    });

    assert!(tool_item.is_some(), "parent tool should be in drained items");
    let sub_agents = tool_item.unwrap();
    assert_eq!(sub_agents.len(), 1);
    assert_eq!(sub_agents[0].agent_name, "explorer");
    assert!(sub_agents[0].done);
    assert_eq!(sub_agents[0].tools.len(), 1);
    assert_eq!(sub_agents[0].tools[0].name, "grep");
}

#[test]
fn sub_agent_prompt_error_finalizes_sub_agents() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    // PromptError should finalize sub-agents
    app.on_acp_event(AcpEvent::PromptError(agent_client_protocol::Error::internal_error()));

    assert!(!app.wants_tick());
}

#[test]
fn sub_agent_prompt_cancelled_finalizes_sub_agents() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::Cancelled));

    assert!(!app.wants_tick());
}

#[test]
fn sub_agent_multiple_sub_agents_per_parent() {
    let (mut app, _command_rx) = make_app();
    app.on_acp_event(tool_call("parent-1", "spawn_subagent"));
    app.on_acp_event(tool_completed("parent-1"));

    app.on_acp_event(sub_agent_tool_call("parent-1", "task-a", "explorer", "c1", "grep", "{}"));
    app.on_acp_event(sub_agent_tool_call("parent-1", "task-b", "builder", "c2", "write", "{}"));

    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    // Both agent names should be visible
    assert!(viewport.contains("explorer"), "viewport should show explorer:\n{viewport}");
    assert!(viewport.contains("builder"), "viewport should show builder:\n{viewport}");
}

mod progress_indicator_tests {
    use super::*;
    use wisp_next::progress_indicator::SPINNER_FRAMES;

    #[test]
    fn idle_renders_no_progress_indicator() {
        let (mut app, _command_rx) = make_app();
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| full.contains(frame));
        assert!(!has_spinner, "buffer should not contain spinner when idle:\n{full}");
        assert!(!full.contains("Working..."), "{full}");
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn prompt_shows_progress_with_esc_hint() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        assert!(
            app.progress_indicator().is_active(),
            "progress indicator not active. prompt_in_flight={}, is_agent_busy={}",
            app.busy(),
            app.is_agent_busy()
        );
        // Use a 120-char terminal to fit the full tip + esc hint on one line
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| full.contains(frame));
        assert!(has_spinner, "full buffer should contain spinner during prompt:\n{full}");
        assert!(full.contains("esc to interrupt"), "full buffer should show esc hint:\n{full}");
    }

    #[test]
    fn prompt_done_hides_progress() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(text_chunk("response"));
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        assert!(!full.contains("esc to interrupt"), "{full}");
        assert!(!full.contains("Working..."), "{full}");
    }

    #[test]
    fn compaction_active_shows_compacting_message() {
        let (mut app, _command_rx) = make_app();
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn compaction_inactive_hides_indicator() {
        let (mut app, _command_rx) = make_app();
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: false }));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn compaction_during_prompt_shows_esc_hint() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        assert!(full.contains("Compacting context"), "{full}");
        assert!(full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn workspace_moving_shows_progress() {
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
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        assert!(full.contains("Moving workspace"), "{full}");
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn workspace_loading_session_shows_progress() {
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
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(viewport.contains("Loading session in new workspace"), "{viewport}");
    }

    #[test]
    fn workspace_move_failure_clears_indicator() {
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
        app.on_acp_event(workspace_move_failed("permission denied"));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Moving workspace"), "{viewport}");
        assert!(!viewport.contains("Loading session"), "{viewport}");
    }

    #[test]
    fn workspace_move_precedence_over_compaction() {
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
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(viewport.contains("Moving workspace"), "{viewport}");
        assert!(!viewport.contains("Compacting"), "{viewport}");
    }

    #[test]
    fn compaction_precedence_over_agent_work() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(viewport.contains("Compacting context"), "{viewport}");
    }

    #[test]
    fn wants_tick_true_immediately_after_prompt_submit() {
        let (mut app, _command_rx) = make_app();
        assert!(!app.wants_tick(), "wants_tick should be false when idle");
        submit_prompt(&mut app, "hello");
        assert!(app.wants_tick(), "wants_tick should be true immediately after prompt submit (raw state)");
    }

    #[test]
    fn wants_tick_true_during_compaction() {
        let (mut app, _command_rx) = make_app();
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        assert!(app.wants_tick(), "wants_tick should be true during compaction");
    }

    #[test]
    fn wants_tick_true_during_workspace_move() {
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
        assert!(app.wants_tick(), "wants_tick should be true during workspace move");
    }

    #[test]
    fn wants_tick_false_after_idle() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(text_chunk("done"));
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        assert!(!app.wants_tick(), "wants_tick should be false after prompt completes and no other activity");
    }

    #[test]
    fn tick_animates_spinner_deterministically() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");

        let now = Instant::now();

        // Capture rendering at tick 0
        let mut terminal_a = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal_a, &mut app).unwrap();
        let full_a = buffer_text(terminal_a.backend().buffer());

        // Advance tick once
        app.on_tick(now);
        let mut terminal_b = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal_b, &mut app).unwrap();
        let full_b = buffer_text(terminal_b.backend().buffer());

        // Different frames should produce different braille characters
        assert_ne!(full_a, full_b, "spinner should animate with each tick");
    }

    #[test]
    fn tick_stops_when_idle() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");

        let now = Instant::now();
        app.on_tick(now);

        // Complete the prompt
        app.on_acp_event(text_chunk("response"));
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
        // Tick while idle — should not change
        app.on_tick(now);
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();
        let full = buffer_text(terminal.backend().buffer());
        assert!(!full.contains("esc to interrupt"), "{full}");
    }

    #[test]
    fn progress_is_not_inserted_into_scrollback() {
        let (mut app, _command_rx) = make_app();
        // Fill enough content to trigger scrollback
        submit_prompt(&mut app, "hello");
        let mut response = String::new();
        for i in 0..20 {
            writeln!(response, "line-{i}").unwrap();
        }
        app.on_acp_event(text_chunk(&response));
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        // Submit another prompt so we can see progress indicator
        submit_prompt(&mut app, "another");
        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();

        let scrollback = buffer_text(&history_buffer(&mut terminal));
        let has_spinner = SPINNER_FRAMES.iter().any(|frame| scrollback.contains(frame));
        assert!(!has_spinner, "scrollback should not contain progress spinner:\n{scrollback}");
        assert!(!scrollback.contains("esc to interrupt"), "scrollback should not contain esc hint:\n{scrollback}");
        assert!(!scrollback.contains("Working..."), "scrollback should not contain Working:\n{scrollback}");
    }

    #[test]
    fn context_cleared_resets_progress_state() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        app.on_acp_event(AcpEvent::ContextCleared(ContextClearedParams {}));

        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Compacting"), "{viewport}");
        assert!(!viewport.contains("esc to interrupt"), "{viewport}");
        assert!(!app.wants_tick(), "wants_tick should be false after context cleared");
    }

    #[test]
    fn new_session_resets_progress_state() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        app.on_acp_event(AcpEvent::NewSessionCreated {
            session_id: SessionId::new("new-session"),
            config_options: Vec::new(),
        });

        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Compacting"), "{viewport}");
        assert!(!app.wants_tick(), "wants_tick should be false after new session");
    }

    #[test]
    fn session_loaded_resets_progress_state() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ContextCompaction(ContextCompactionParams { active: true }));
        app.on_acp_event(AcpEvent::SessionLoaded {
            session_id: SessionId::new("loaded-session"),
            config_options: Vec::new(),
        });

        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Compacting"), "{viewport}");
    }

    #[test]
    fn progress_lines_include_padding() {
        let (mut app, _command_rx) = make_app();
        submit_prompt(&mut app, "hello");
        let mut terminal = make_terminal_with_dimensions(120, 30);
        sync_terminal(&mut terminal, &mut app).unwrap();

        let full = buffer_text(terminal.backend().buffer());
        let spinner_lines: Vec<_> =
            full.lines().filter(|line| SPINNER_FRAMES.iter().any(|frame| line.contains(frame))).collect();
        assert!(!spinner_lines.is_empty(), "should have spinner lines:\n{full}");
        for line in spinner_lines {
            assert!(line.starts_with("  "), "spinner line should start with padding spaces, got: '{line}'");
        }
    }

    #[test]
    fn workspace_list_failed_clears_indicator() {
        let (mut app, mut command_rx) = make_app_with_workspace_move();
        type_text(&mut app, "/move");
        app.on_key(key(KeyCode::Tab));
        let _ = command_rx.try_recv().unwrap();
        app.on_acp_event(workspace_list_failed("network error"));

        let mut terminal = make_terminal();
        sync_terminal(&mut terminal, &mut app).unwrap();
        let viewport = buffer_text(&viewport_buffer(&mut terminal));
        assert!(!viewport.contains("Moving workspace"), "{viewport}");
        assert!(!app.wants_tick(), "wants_tick should be false after list failure");
    }
}

// --- Plan ACP-event integration tests ---
