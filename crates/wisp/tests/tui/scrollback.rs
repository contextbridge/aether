use super::support::*;

#[test]
fn completed_large_reply_keeps_its_tail_in_the_viewport() {
    let mut ui = TestUi::new();
    commit_overflowing_reply(&mut ui);
    ui.assert_viewport_contains("finished reply");
    let conversation = ui.conversation_text();
    assert_eq!(ui.conversation_text(), conversation);
}

#[test]
fn committed_message_replacements_publish_a_corrected_transcript_once() {
    let mut ui = TestUi::new();
    commit_overflowing_reply(&mut ui);
    ui.acp_event(session_update(acp::SessionUpdate::AgentMessage(
        acp::AgentMessage::new("overflow-reply").content(vec![acp::ContentBlock::from("corrected answer")]),
    )));
    ui.assert_history_contains("Transcript updated;");
    ui.assert_viewport_contains("corrected answer");
    let history = ui.history_text();
    assert_eq!(ui.history_text(), history);
    ui.acp_event(text_chunk_with_id("overflow-reply", " plus more"));
    ui.assert_viewport_contains("corrected answer plus more");
}

#[test]
fn late_chunk_for_a_fully_committed_message_is_not_lost() {
    let mut ui = TestUi::new();
    commit_overflowing_reply(&mut ui);
    ui.acp_event(text_chunk_with_id("next-reply", &"later response\n\n".repeat(30)));
    ui.assert_history_contains("finished reply");
    ui.acp_event(text_chunk_with_id("overflow-reply", "\n\nlate appendix"));
    let history = ui.history_text();
    let corrected = history.split("superseded.").last().unwrap();
    assert!(corrected.contains("late appendix"));
    assert_eq!(corrected.matches("overflow-line-0").count(), 1);
    assert_eq!(ui.history_text(), history);
}

#[test]
fn committed_message_clear_publishes_a_corrected_transcript() {
    let mut ui = TestUi::new();
    commit_overflowing_reply(&mut ui);
    ui.acp_event(session_update(acp::SessionUpdate::AgentMessage(
        acp::AgentMessage::new("overflow-reply").content(agent_client_protocol::schema::MaybeUndefined::Null),
    )));
    ui.assert_history_contains("Transcript updated;");
    ui.assert_viewport_not_contains("overflow-line");
}

#[test]
fn live_message_upserts_replace_clear_and_append_without_a_history_correction() {
    let mut ui = TestUi::new();
    ui.acp_event(text_chunk_with_id("mutable", "a longer original"));
    ui.draw();
    for replacement in [Some("é"), None, Some(""), Some("new")] {
        let content = replacement.map_or(agent_client_protocol::schema::MaybeUndefined::Null, |text| {
            agent_client_protocol::schema::MaybeUndefined::Value(vec![acp::ContentBlock::from(text)])
        });
        ui.acp_event(session_update(acp::SessionUpdate::AgentMessage(
            acp::AgentMessage::new("mutable").content(content),
        )));
        ui.assert_viewport_not_contains("original");
        ui.acp_event(text_chunk_with_id("mutable", " tail"));
        ui.assert_viewport_contains(&format!("{} tail", replacement.unwrap_or_default()));
    }
    ui.assert_history_not_contains("Transcript updated;");
}

#[test]
fn session_changes_never_purge_native_scrollback() {
    let changes: [(&str, Step); 7] = [
        ("a context clear", |ui| ui.acp_event(context_cleared())),
        ("/clear", |ui| {
            ui.run_command("/clear");
            assert_command(ui, |c| matches!(c, AgentCommand::NewSession { .. }), "/clear");
            ui.deliver_result(new_session_created("fresh-session", Vec::new()));
        }),
        ("a failed /clear", |ui| {
            ui.run_command("/clear");
            ui.deliver_result(CommandResult::NewSession(Err("refused".to_string())));
        }),
        ("a session switch", |ui| {
            ui.begin_resume("other", "/tmp/elsewhere");
            ui.deliver_result(session_loaded("other", Vec::new()));
        }),
        ("a workspace move", |ui| {
            move_to_other_workspace(ui);
            ui.deliver_result(workspace_moved("/home/user/code/other"));
        }),
        ("a failed workspace listing", |ui| {
            ui.run_command("/move");
            ui.deliver_result(workspace_list_failed("network error"));
        }),
        ("a failed workspace move", |ui| {
            move_to_other_workspace(ui);
            ui.deliver_result(workspace_move_failed("permission denied"));
        }),
    ];
    for (change, apply) in changes {
        let mut ui = TestUiBuilder::new().workspace_move().build();
        commit_overflowing_reply(&mut ui);
        apply(&mut ui);
        assert!(ui.history_text().contains("overflow-line-0"), "{change} purged native scrollback");
    }
}

fn assert_command(ui: &mut TestUi, expected: impl Fn(&AgentCommand) -> bool, label: &str) {
    let command = ui.next_agent_command().unwrap_or_else(|| panic!("{label} should have sent a command"));
    assert!(expected(&command), "{label} sent {command:?}");
}

fn commit_overflowing_reply(ui: &mut TestUi) {
    ui.submit("talk at length");
    assert_command(ui, |c| matches!(c, AgentCommand::Prompt { .. }), "the overflowing reply");
    let reply = format!("{}finished reply", numbered_lines("overflow-line", 30));
    ui.acp_event(text_chunk_with_id("overflow-reply", &reply));
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.assert_history_contains("overflow-line-0");
}
