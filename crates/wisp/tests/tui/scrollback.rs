use super::support::*;

#[test]
fn completed_large_reply_keeps_its_tail_in_the_viewport() {
    let mut ui = TestUi::new();
    commit_overflowing_reply(&mut ui);
    ui.assert_viewport_contains("finished reply");
    let conversation = ui.conversation_text();
    ui.draw();
    assert_eq!(ui.conversation_text(), conversation);
}

#[test]
fn expanded_user_acknowledgements_keep_the_submitted_display() {
    for prompt in ["/review", "explain @large.rs"] {
        let mut ui = TestUi::new();
        ui.paste(prompt);
        ui.key(key(KeyCode::Enter));
        assert_command(&mut ui, |c| matches!(c, AgentCommand::Prompt { .. }), "submitted display");
        let expanded = "expanded-content-sentinel\n".repeat(100);
        ui.acp_event(session_update(acp::SessionUpdate::UserMessage(
            acp::UserMessage::new("user").content(vec![acp::ContentBlock::from(expanded.clone())]),
        )));
        ui.acp_event(session_update(acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(
            acp::ContentBlock::from(expanded),
            "user",
        ))));
        ui.draw();
        ui.assert_viewport_contains(prompt);
        assert!(!ui.conversation_text().contains("expanded-content-sentinel"));
        ui.assert_history_not_contains("Transcript updated;");
    }
}

#[test]
fn committed_message_replacements_publish_a_corrected_transcript_once() {
    let mut ui = TestUi::new();
    commit_overflowing_reply(&mut ui);
    ui.acp_event(session_update(acp::SessionUpdate::AgentMessage(
        acp::AgentMessage::new("overflow-reply").content(vec![acp::ContentBlock::from("corrected answer")]),
    )));
    ui.draw();
    ui.assert_history_contains("Transcript updated;");
    ui.assert_viewport_contains("corrected answer");
    let history = ui.history_text();
    ui.draw();
    assert_eq!(ui.history_text(), history);
    ui.acp_event(wisp::testing::text_chunk_with_id("overflow-reply", " plus more"));
    ui.draw();
    ui.assert_viewport_contains("corrected answer plus more");
}

#[test]
fn late_chunk_for_a_fully_committed_message_is_not_lost() {
    let mut ui = TestUi::new();
    commit_overflowing_reply(&mut ui);
    ui.acp_event(wisp::testing::text_chunk_with_id("next-reply", &"later response\n\n".repeat(30)));
    ui.draw();
    ui.assert_history_contains("finished reply");
    ui.acp_event(wisp::testing::text_chunk_with_id("overflow-reply", "\n\nlate appendix"));
    ui.draw();
    let history = ui.history_text();
    let corrected = history.split("superseded.").last().unwrap();
    assert!(corrected.contains("late appendix"));
    assert_eq!(corrected.matches("overflow-line-0").count(), 1);
    ui.draw();
    assert_eq!(ui.history_text(), history);
}

#[test]
fn committed_message_clear_publishes_a_corrected_transcript() {
    let mut ui = TestUi::new();
    commit_overflowing_reply(&mut ui);
    ui.acp_event(session_update(acp::SessionUpdate::AgentMessage(
        acp::AgentMessage::new("overflow-reply").content(agent_client_protocol::schema::MaybeUndefined::Null),
    )));
    ui.draw();
    ui.assert_history_contains("Transcript updated;");
    assert!(!ui.viewport_text().contains("overflow-line"));
}

#[test]
fn live_message_upserts_replace_clear_and_append_without_a_history_correction() {
    let mut ui = TestUi::new();
    ui.acp_event(wisp::testing::text_chunk_with_id("mutable", "a longer original"));
    ui.draw();
    for replacement in [Some("é"), None, Some(""), Some("new")] {
        let content = replacement.map_or(agent_client_protocol::schema::MaybeUndefined::Null, |text| {
            agent_client_protocol::schema::MaybeUndefined::Value(vec![acp::ContentBlock::from(text)])
        });
        ui.acp_event(session_update(acp::SessionUpdate::AgentMessage(
            acp::AgentMessage::new("mutable").content(content),
        )));
        ui.draw();
        ui.assert_viewport_not_contains("original");
        ui.acp_event(wisp::testing::text_chunk_with_id("mutable", " tail"));
        ui.draw();
        ui.assert_viewport_contains(&format!("{} tail", replacement.unwrap_or_default()));
    }
    ui.assert_history_not_contains("Transcript updated;");
}

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
    ui.acp_event(wisp::testing::text_chunk_with_id("overflow-reply", &format!("{reply}finished reply")));
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.draw();
    assert!(
        ui.history_text().contains("overflow-line-0"),
        "precondition: a prior reply must have been committed to scrollback"
    );
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

    assert!(matches!(ui.start_new_session(), AgentCommand::NewSession { .. }));
    ui.assert_history_contains("overflow-line-0");

    ui.deliver_result(new_session_created("fresh-session", Vec::new()));

    ui.assert_history_contains("overflow-line-0");
}

#[test]
fn session_switch_preserves_native_scrollback_without_purging() {
    let mut ui = TestUi::new();

    commit_overflowing_reply(&mut ui);

    assert!(matches!(ui.open_session_picker(), AgentCommand::ListSessions));
    ui.deliver_result(sessions_listed(vec![session_info("other", "/tmp/elsewhere", "Other", "2025-01-01T00:00:00Z")]));
    ui.key(key(KeyCode::Enter));
    assert_command(&mut ui, |c| matches!(c, AgentCommand::ResumeSession { .. }), "resume");

    ui.assert_history_contains("overflow-line-0");

    ui.deliver_result(session_loaded("other", Vec::new()));

    ui.assert_history_contains("overflow-line-0");
}

#[test]
fn successful_workspace_move_does_not_purge_native_scrollback() {
    let mut ui = TestUiBuilder::new().workspace_move().build();

    commit_overflowing_reply(&mut ui);

    assert!(matches!(ui.open_workspace_picker(), AgentCommand::ListWorkspaces { .. }));
    ui.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    ui.key(key(KeyCode::Enter));
    assert_command(&mut ui, |c| matches!(c, AgentCommand::MoveWorkspace { .. }), "move");
    ui.deliver_result(workspace_moved("/home/user/code/other"));

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

    assert!(matches!(app.start_new_session(), AgentCommand::NewSession { .. }));

    drain_commands(&mut app);
}

#[test]
fn a_workspace_listing_failure_does_not_purge() {
    let mut app = make_app_with_workspace_move();

    assert!(matches!(app.open_workspace_picker(), AgentCommand::ListWorkspaces { .. }));

    app.deliver_result(workspace_list_failed("network error"));

    drain_commands(&mut app);
}

#[test]
fn a_workspace_move_failure_does_not_purge() {
    let mut app = make_app_with_workspace_move();

    assert!(matches!(app.open_workspace_picker(), AgentCommand::ListWorkspaces { .. }));
    app.deliver_result(workspaces_listed(vec![
        workspace_entry("/home/user/code/current", true),
        workspace_entry("/home/user/code/other", false),
    ]));
    app.key(key(KeyCode::Enter));
    assert!(matches!(app.next_agent_command(), Some(AgentCommand::MoveWorkspace { .. })));

    app.deliver_result(workspace_move_failed("permission denied"));

    drain_commands(&mut app);
}
