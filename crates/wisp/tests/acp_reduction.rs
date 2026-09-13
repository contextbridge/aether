#![cfg(feature = "testing")]

use acp_utils::client::AcpEvent;
use agent_client_protocol::schema::v2 as acp;
use wisp::conversation::tool_calls::ToolStatus;
use wisp::testing::{TestUi, session_update};

#[test]
fn user_ack_and_idle_preserve_one_foreground_turn() {
    let mut ui = TestUi::new();
    ui.submit("hello");
    ui.acp_event(update(serde_json::json!({"sessionUpdate":"user_message", "messageId":"user-1", "content":[{"type":"text", "text":"hello"}]})));
    ui.acp_event(update(serde_json::json!({"sessionUpdate":"state_update", "state":"running"})));
    assert!(ui.app().waiting_for_response());
    ui.acp_event(update(serde_json::json!({"sessionUpdate":"agent_message_chunk", "messageId":"reply-1", "content":{"type":"text", "text":"world"}})));
    ui.acp_event(update(serde_json::json!({"sessionUpdate":"state_update", "state":"idle", "stopReason":"end_turn"})));
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
    ui.acp_event(message("agent_message", "reply", "obsolete answer"));
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.assert_conversation_contains("obsolete answer");
    ui.acp_event(message("agent_message", "reply", "corrected answer"));
    ui.assert_conversation_contains("corrected answer");
    ui.assert_conversation_not_contains("obsolete answer");
    ui.acp_event(update(serde_json::json!({"sessionUpdate":"agent_message", "messageId":"reply", "content":null})));
    ui.assert_conversation_not_contains("corrected answer");
    ui.acp_event(update(serde_json::json!({"sessionUpdate":"agent_message_chunk", "messageId":"reply", "content":{"type":"text", "text":"appended answer"}})));
    ui.assert_conversation_contains("appended answer");
    assert!(!ui.app().progress_indicator().is_active());
}

#[test]
fn ready_and_replayed_updates_do_not_start_activity() {
    let mut ui = TestUi::new();
    for event in [
        update(serde_json::json!({"sessionUpdate":"state_update", "state":"idle"})),
        message("agent_message", "history", "saved response"),
        update(
            serde_json::json!({"sessionUpdate":"agent_message_chunk", "messageId":"history", "content":{"type":"text", "text":" tail"}}),
        ),
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
    ui.acp_event(update(serde_json::json!({"sessionUpdate":"state_update", "state":"requires_action"})));
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
    ui.acp_event(update(
        serde_json::json!({"sessionUpdate":"tool_call_update", "toolCallId":"tool", "title":"Read file"}),
    ));
    ui.acp_event(update(serde_json::json!({"sessionUpdate":"tool_call_content_chunk", "toolCallId":"tool", "content":{"type":"content", "content":{"type":"text", "text":"output"}}})));
    ui.acp_event(update(
        serde_json::json!({"sessionUpdate":"tool_call_update", "toolCallId":"tool", "status":"completed"}),
    ));
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
        ui.acp_event(session_update(acp::SessionUpdate::PlanUpdate(acp::PlanUpdate::new(
            acp::PlanUpdateContent::items(
                id,
                vec![acp::PlanEntry::new(text, acp::PlanEntryPriority::Medium, acp::PlanEntryStatus::Pending)],
            ),
        ))));
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
            serde_json::json!({"sessionUpdate":"user_message", "messageId":"user", "content":[{"type":"text", "text":"saved prompt"}]}),
            serde_json::json!({"sessionUpdate":"agent_message", "messageId":"reply", "content":[{"type":"text", "text":"saved answer"}]}),
            serde_json::json!({"sessionUpdate":"state_update", "state":"idle"}),
        ].into_iter().map(|value| acp_utils::client::ReplayableEvent::SessionUpdate(Box::new(acp::UpdateSessionNotification::new("restored", serde_json::from_value(value).unwrap())))).collect();
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

fn message(kind: &str, id: &str, text: &str) -> AcpEvent {
    update(serde_json::json!({"sessionUpdate":kind, "messageId":id, "content":[{"type":"text", "text":text}]}))
}

fn update(value: serde_json::Value) -> AcpEvent {
    session_update(serde_json::from_value(value).unwrap())
}
