use super::support::*;
use acp_utils::testing::{idle_notification, plan_notification, running_notification};
use agent_client_protocol::schema::MaybeUndefined;

#[test]
fn user_ack_and_idle_preserve_one_foreground_turn() {
    let mut ui = TestUi::new();
    ui.submit("hello");
    ui.acp_event(running_notification("test-session").into());
    assert!(ui.app().waiting_for_response());
    ui.acp_event(text_chunk_with_id("reply-1", "world"));
    ui.acp_event(idle_notification("other", Some(acp::StopReason::EndTurn)).into());
    assert!(ui.app().waiting_for_response());
    ui.acp_event(idle_notification("test-session", Some(acp::StopReason::EndTurn)).into());
    assert!(!ui.app().waiting_for_response());
    let text = ui.conversation_text();
    assert_eq!(text.matches("hello").count(), 1, "{text}");
    assert!(text.contains("world"), "{text}");
    assert_eq!(bells(&mut ui), 1);
    ui.complete_prompt(acp::StopReason::EndTurn);
    assert!(ui.take_commands().is_empty());
}

#[test]
fn cancellation_keeps_final_output_and_ignores_stale_prompt_results() {
    let mut ui = TestUi::new();
    ui.submit("hello");
    ui.next_agent_command().unwrap();
    ui.deliver_result(CommandResult::Cancel(Ok(())));
    assert!(ui.app().waiting_for_response(), "cancel is not completion");
    ui.acp_event(text_chunk_with_id("reply", "final output"));
    ui.acp_event(idle_notification("test-session", Some(acp::StopReason::Cancelled)).into());
    assert!(!ui.app().waiting_for_response());
    ui.deliver_result(CommandResult::Prompt { request_id: client_core::request::RequestId::next(), result: Ok(()) });
    assert!(!ui.app().waiting_for_response(), "a result for no queued prompt changes nothing");
    assert_eq!(ui.conversation_text().matches("final output").count(), 1);
}

#[test]
fn config_result_for_a_replaced_conversation_is_ignored() {
    for restored_id in ["restored", "other"] {
        let mut ui = TestUi::new();
        ui.deliver_result(new_session_created("test-session", Vec::new()));
        let stale = ui.app().conversation_id();
        ui.deliver_result(CommandResult::ConfigOptionsUpdated {
            conversation_id: stale,
            result: Ok(acp::SetSessionConfigOptionResponse::new(vec![select_option("model", "stale")])),
        });
        assert_eq!(ui.app().config_options().len(), 1);

        ui.begin_resume(restored_id, "/tmp");
        ui.deliver_result(session_loaded(restored_id, vec![select_option("model", "restored")]));
        ui.deliver_result(CommandResult::ConfigOptionsUpdated {
            conversation_id: stale,
            result: Ok(acp::SetSessionConfigOptionResponse::new(vec![])),
        });
        assert_eq!(ui.app().config_options().len(), 1, "stale result must not erase restored configuration");
    }
}

#[test]
fn replay_snapshot_cannot_replace_a_live_turn() {
    let mut ui = TestUi::new();
    ui.submit("current");
    ui.deliver_result(session_loaded("test-session", Vec::new()));
    assert!(ui.app().waiting_for_response());
    ui.assert_conversation_contains("current");
}

#[test]
fn live_messages_can_be_replaced_cleared_and_appended_after_idle() {
    let mut ui = TestUi::new();
    ui.submit("question");
    ui.acp_event(message("reply", "obsolete answer"));
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.assert_conversation_contains("obsolete answer");
    ui.acp_event(message("reply", "corrected answer"));
    ui.assert_conversation_contains("corrected answer");
    ui.assert_conversation_not_contains("obsolete answer");
    ui.acp_event(session_update(acp::SessionUpdate::AgentMessage(
        acp::AgentMessage::new("reply").content(MaybeUndefined::Null),
    )));
    ui.assert_conversation_not_contains("corrected answer");
    ui.acp_event(text_chunk_with_id("reply", "appended answer"));
    ui.assert_conversation_contains("appended answer");
    assert!(!ui.app().progress_indicator().is_active());
}

#[test]
fn ready_and_replayed_updates_do_not_start_activity() {
    let mut ui = TestUi::new();
    for event in [
        idle_notification("test-session", None).into(),
        message("history", "saved response"),
        text_chunk_with_id("history", " tail"),
    ] {
        ui.acp_event(event);
    }
    assert!(!ui.app().waiting_for_response());
    assert!(!ui.app().progress_indicator().is_active());
    ui.assert_conversation_contains("saved response tail");
}

#[test]
fn requires_action_and_unknown_updates_preserve_the_turn() {
    let mut ui = TestUi::with_dimensions(80, 20);
    ui.submit("question");
    ui.acp_event(session_update(acp::SessionUpdate::StateUpdate(acp::StateUpdate::RequiresAction(
        acp::RequiresActionStateUpdate::new(),
    ))));
    ui.assert_viewport_contains("Waiting for action");
    for value in [
        serde_json::json!({"sessionUpdate":"future_update", "field":true}),
        serde_json::json!({"sessionUpdate":"state_update", "state":"future_state"}),
        serde_json::json!({"sessionUpdate":"terminal_update", "terminalId":"terminal"}),
        serde_json::json!({"sessionUpdate":"terminal_output_chunk", "terminalId":"terminal", "data":"ignored"}),
    ] {
        ui.acp_event(update(value));
    }
    assert!(ui.app().waiting_for_response());
    ui.complete_prompt(acp::StopReason::Cancelled);
    assert!(!ui.app().waiting_for_response());
    assert!(!ui.app().progress_indicator().is_active());
}

#[test]
fn tool_and_plan_updates_are_upserts_and_chunks_append() {
    let mut ui = TestUi::new();
    ui.submit("work");
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new("tool").title("Read file"),
    )));
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallContentChunk(acp::ToolCallContentChunk::new(
        "tool",
        acp::ToolCallContent::Content(Box::new(acp::Content::new("output"))),
    ))));
    ui.acp_event(tool_completed("tool"));
    let tools: Vec<_> = ui
        .app()
        .conversation_items()
        .iter()
        .filter_map(|item| match item.content() {
            ConversationContent::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].title(), "Read file");
    assert_eq!(tools[0].status, ToolStatus::Success);
    for (id, text) in [("a", "old"), ("b", "preserved"), ("a", "replacement")] {
        let entry = acp::PlanEntry::new(text, acp::PlanEntryPriority::Medium, acp::PlanEntryStatus::Pending);
        ui.acp_event(plan_notification("test-session", id, vec![entry]).into());
    }
    let entries = ui.app().plan_entries();
    assert_eq!(entries.iter().map(|entry| entry.content.as_str()).collect::<Vec<_>>(), ["replacement"]);
}

#[test]
fn repeated_resume_replaces_history_without_adopting_a_live_echo() {
    let mut ui = TestUi::new();
    ui.submit("old prompt");
    ui.complete_prompt(acp::StopReason::EndTurn);
    for id in ["restored", "restored-again"] {
        ui.begin_resume(id, "/tmp");
        let replay: Vec<_> = [
            acp::SessionUpdate::UserMessage(acp::UserMessage::new("user").content(vec!["saved prompt".into()])),
            acp::SessionUpdate::AgentMessage(acp::AgentMessage::new("reply").content(vec!["saved answer".into()])),
            idle_notification("restored", None).update,
        ]
        .into_iter()
        .map(|update| session_update_for(id, update))
        .collect();
        for event in replay {
            ui.acp_event(event);
        }
        assert_eq!(ui.conversation_text().matches("saved answer").count(), 1);
        ui.deliver_result(session_loaded(id, Vec::new()));
        assert_eq!(ui.app().conversation_items().len(), 2);
        assert!(!ui.app().waiting_for_response());
        assert!(!ui.app().progress_indicator().is_active());
        ui.assert_conversation_contains("saved answer");
        ui.assert_conversation_not_contains("old prompt");
    }
}

#[test]
fn seeded_history_uses_distinct_message_ids_and_finishes_each_turn() {
    let mut ui = TestUi::new();
    ui.seed_long_history(3);
    let users: Vec<_> = ui
        .app()
        .conversation_items()
        .iter()
        .filter(|item| matches!(item.content(), ConversationContent::User(_)))
        .collect();
    assert_eq!(users.len(), 3);
    assert_ne!(users[0].message_id(), users[1].message_id());
    assert!(!ui.app().waiting_for_response());
}

fn message(id: &str, text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::AgentMessage(acp::AgentMessage::new(id).content(vec![text.into()])))
}

fn update(value: serde_json::Value) -> AcpEvent {
    session_update(serde_json::from_value(value).unwrap())
}
