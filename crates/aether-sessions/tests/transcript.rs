use aether_core::events::{AgentEvent, ContextEvent, TurnOutcome};
use aether_sessions::testing::{
    agent_switched, assistant_text, compaction_result, partial_text, tool_call, tool_error, tool_result, turn_ended,
    user_message,
};
use aether_sessions::{SessionEvent, context_from_events, conversation_messages_from_events};

#[test]
fn reconstruction_preserves_stored_message_identity() {
    let context = context_from_events(&[assistant_text("stored-id", "Response"), turn_ended(TurnOutcome::Completed)]);
    let value = serde_json::to_value(&context.messages()[0]).unwrap();
    assert_eq!(value["message_id"], "stored-id");
}

#[test]
fn reconstruction_preserves_messages_within_one_turn() {
    let context = context_from_events(&[
        assistant_text("first", "First response"),
        assistant_text("second", "Second response"),
        turn_ended(TurnOutcome::Completed),
    ]);
    assert_eq!(context.message_count(), 2);
    assert!(
        matches!(&context.messages()[0], llm::ChatMessage::Assistant { content, .. } if content == "First response")
    );
    assert!(
        matches!(&context.messages()[1], llm::ChatMessage::Assistant { content, .. } if content == "Second response")
    );
}

#[test]
fn reconstructs_conversation_and_ignores_control_events() {
    let messages = conversation_messages_from_events(&[
        user_message("Hello"),
        agent_switched(None, Some("coder")),
        assistant_text("message-1", "Hi there!"),
        turn_ended(TurnOutcome::Completed),
    ]);

    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], llm::ChatMessage::User { .. }));
    assert!(matches!(messages[1], llm::ChatMessage::Assistant { .. }));
}

#[test]
fn task_completion_during_iteration_preserves_pending_tools() {
    let task = aether_core::events::ToolEvent::TaskCancelled {
        request: llm::ToolCallRequest {
            id: "background-call".into(),
            name: "background".into(),
            arguments: "{}".into(),
        },
        task_id: "task".into(),
    };
    let context = context_from_events(&[
        tool_result("call", "read", "contents"),
        SessionEvent::Agent(AgentEvent::Tool(task.clone())),
        assistant_text("assistant", "done"),
        turn_ended(TurnOutcome::Completed),
    ]);
    assert_eq!(context.message_count(), 3);
    assert_eq!(context.messages()[0].message_id().as_str(), "assistant");
    assert!(context.messages()[1].is_tool_result());
    assert_eq!(context.messages()[2].message_id(), task.task_context_message().unwrap().message_id());
}

#[test]
fn reconstructs_successful_tool_calls() {
    let context = context_from_events(&[
        user_message("Read Cargo.toml"),
        tool_call("call-1", "read_file", "{}"),
        tool_result("call-1", "read_file", "file contents"),
        assistant_text("message-1", "Here is the file"),
        turn_ended(TurnOutcome::Completed),
    ]);

    assert_eq!(context.message_count(), 3);
    assert!(
        matches!(&context.messages()[1], llm::ChatMessage::Assistant { content, tool_calls, .. } if content == "Here is the file" && tool_calls.len() == 1)
    );
    assert!(context.messages()[2].is_tool_result());
}

#[test]
fn reconstructs_tools_failures_and_context_boundaries() {
    let events = [
        user_message("Read missing.txt"),
        tool_error("call-1", "read_file", "file not found"),
        assistant_text("tool-response", ""),
        turn_ended(TurnOutcome::Completed),
        SessionEvent::Agent(AgentEvent::Context(ContextEvent::Cleared)),
        user_message("Start fresh"),
    ];
    let context = context_from_events(&events);
    let before_clear = context_from_events(&events[..4]);

    assert_eq!(before_clear.message_count(), 3);
    assert!(
        matches!(before_clear.messages()[2], llm::ChatMessage::ToolCallResult(Err(ref error)) if error.error == "file not found")
    );
    assert_eq!(context.message_count(), 1);
    assert!(matches!(context.messages()[0], llm::ChatMessage::User { .. }));
}

#[test]
fn empty_or_contentless_turns_do_not_add_messages() {
    assert_eq!(context_from_events(&[]).message_count(), 0);
    assert_eq!(context_from_events(&[turn_ended(TurnOutcome::Completed)]).message_count(), 0);
}

#[test]
fn completed_turns_reset_the_accumulator() {
    let context = context_from_events(&[
        assistant_text("message-1", "Turn 1"),
        turn_ended(TurnOutcome::Completed),
        assistant_text("message-2", "Turn 2"),
        turn_ended(TurnOutcome::Completed),
    ]);

    assert_eq!(context.message_count(), 2);
}

#[test]
fn compaction_replaces_prior_messages_with_a_summary() {
    let context = context_from_events(&[
        user_message("Hello"),
        assistant_text("message-1", "Hi!"),
        turn_ended(TurnOutcome::Completed),
        compaction_result("Earlier we greeted each other.", 2),
        user_message("What did we talk about?"),
    ]);

    assert_eq!(context.message_count(), 2);
    assert!(context.messages()[0].is_summary());
}

#[test]
fn complete_messages_are_reconstructed_but_streaming_chunks_are_not() {
    let context = context_from_events(&[
        partial_text("partial", "partial"),
        assistant_text("complete", "complete"),
        turn_ended(TurnOutcome::Completed),
    ]);

    assert_eq!(context.message_count(), 1);
    assert!(matches!(&context.messages()[0], llm::ChatMessage::Assistant { content, .. } if content == "complete"));
}
