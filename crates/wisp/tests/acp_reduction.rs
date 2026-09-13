#![cfg(feature = "testing")]

use acp_utils::client::{AcpEvent, ReplayableEvent};
use acp_utils::testing::{idle_notification, plan_notification, running_notification};
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v2 as acp;
use wisp::conversation::tool_calls::ToolStatus;
use wisp::testing::{TestUi, session_update, text_chunk_with_id, tool_completed};

#[test]
fn user_ack_and_idle_preserve_one_foreground_turn() {
    let mut ui = TestUi::new();
    ui.submit("hello");
    ui.acp_event(session_update(acp::SessionUpdate::UserMessage(
        acp::UserMessage::new("user-1").content(vec!["hello".into()]),
    )));
    ui.acp_event(session_update(running_notification("test-session").update));
    assert!(ui.app().waiting_for_response());
    ui.acp_event(text_chunk_with_id("reply-1", "world"));
    ui.acp_event(session_update(idle_notification("test-session", Some(acp::StopReason::EndTurn)).update));
    assert!(ui.app().waiting_for_response(), "raw idle must not finish a turn twice");
    ui.acp_event(AcpEvent::PromptCompleted { session_id: "other".into(), stop_reason: acp::StopReason::EndTurn });
    assert!(ui.app().waiting_for_response());
    ui.acp_event(AcpEvent::PromptCompleted {
        session_id: "test-session".into(),
        stop_reason: acp::StopReason::EndTurn,
    });
    assert!(!ui.app().waiting_for_response());
    let text = ui.conversation_text();
    assert_eq!(text.matches("hello").count(), 1, "{text}");
    assert!(text.contains("world"), "{text}");
    let commands = ui.executor_mut().take_commands();
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(
                command,
                wisp::command::Command::Terminal(wisp::command::TerminalCommand::RingBell)
            ))
            .count(),
        1
    );
    ui.complete_prompt(acp::StopReason::EndTurn);
    assert!(ui.executor_mut().take_commands().is_empty());
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
        session_update(idle_notification("test-session", None).update),
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
            wisp::conversation::ConversationContent::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].title(), "Read file");
    assert_eq!(tools[0].status, ToolStatus::Success);
    for (id, text) in [("a", "old"), ("b", "preserved"), ("a", "replacement")] {
        ui.acp_event(session_update(
            plan_notification(
                "test-session",
                id,
                vec![acp::PlanEntry::new(text, acp::PlanEntryPriority::Medium, acp::PlanEntryStatus::Pending)],
            )
            .update,
        ));
    }
    let entries = ui.app().plan_entries();
    assert_eq!(entries.iter().map(|entry| entry.content.as_str()).collect::<Vec<_>>(), ["replacement"]);
}

#[test]
fn repeated_resume_replaces_history_without_adopting_a_live_echo() {
    let mut ui = TestUi::new();
    ui.submit("old prompt");
    ui.complete_prompt(acp::StopReason::EndTurn);
    for _ in 0..2 {
        let replay = [
            acp::SessionUpdate::UserMessage(acp::UserMessage::new("user").content(vec!["saved prompt".into()])),
            acp::SessionUpdate::AgentMessage(acp::AgentMessage::new("reply").content(vec!["saved answer".into()])),
            idle_notification("restored", None).update,
        ]
        .into_iter()
        .map(|update| ReplayableEvent::SessionUpdate(Box::new(acp::UpdateSessionNotification::new("restored", update))))
        .collect();
        ui.acp_event(AcpEvent::SessionResumed(acp_utils::client::ResumedSession {
            session_id: "restored".into(),
            response: acp::ResumeSessionResponse::new(),
            replay,
        }));
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
        .filter(|item| matches!(item.content(), wisp::conversation::ConversationContent::User(_)))
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
