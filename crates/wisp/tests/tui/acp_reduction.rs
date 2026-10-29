use super::support::*;
use acp_utils::testing::{idle_notification, running_notification};

#[test]
fn user_ack_and_idle_preserve_one_foreground_turn() {
    let mut ui = TestUi::new();
    ui.submit("hello");
    ui.acp_event(running_notification("test-session").into());
    assert!(ui.app().core().waiting_for_response());
    ui.acp_event(text_chunk_with_id("reply-1", "world"));
    ui.acp_event(idle_notification("other", Some(acp::StopReason::EndTurn)).into());
    assert!(ui.app().core().waiting_for_response());
    ui.acp_event(idle_notification("test-session", Some(acp::StopReason::EndTurn)).into());
    assert!(!ui.app().core().waiting_for_response());
    let text = ui.conversation_text();
    assert_eq!(text.matches("hello").count(), 1, "{text}");
    assert!(text.contains("world"), "{text}");
    assert_eq!(ui.bells(), 1);
    ui.complete_prompt(acp::StopReason::EndTurn);
    assert_eq!(ui.bells(), 1, "a completion with no turn running rings no bell");
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
    assert!(ui.app().core().waiting_for_response());
    ui.complete_prompt(acp::StopReason::Cancelled);
    assert!(!ui.app().core().waiting_for_response());
    assert!(!ui.app().progress_indicator().is_active());
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
        .map(|update| on_session(id, update))
        .collect();
        for event in replay {
            ui.acp_event(event);
        }
        assert_eq!(ui.conversation_text().matches("saved answer").count(), 1);
        session_loaded(&mut ui, Vec::new());
        assert_eq!(ui.app().core().conversation().items().len(), 2);
        assert!(!ui.app().core().waiting_for_response());
        assert!(!ui.app().progress_indicator().is_active());
        ui.assert_conversation_contains("saved answer");
        ui.assert_conversation_not_contains("old prompt");
    }
}

fn update(value: serde_json::Value) -> AcpEvent {
    session_update(serde_json::from_value(value).unwrap())
}
