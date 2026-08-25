use aether_core::events::{AgentEvent, ContextEvent, MessageEvent, StreamState, ToolEvent, TurnEvent, TurnOutcome};
use aether_sessions::{
    SessionControlEvent, SessionEvent, UserEvent, context_from_events, conversation_messages_from_events,
};

fn user(content: &str) -> SessionEvent {
    SessionEvent::User(UserEvent::Message { content: vec![llm::ContentBlock::text(content)] })
}

fn complete_text(id: &str, content: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
        message_id: id.into(),
        chunk: content.into(),
        is_complete: true,
    }))
}

fn ended() -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }))
}

fn tool_result(id: &str, name: &str, result: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Result {
        result: llm::ToolCallResult { id: id.into(), name: name.into(), arguments: "{}".into(), result: result.into() },
        result_meta: None,
    }))
}

#[test]
fn reconstructs_conversation_and_ignores_control_events() {
    let messages = conversation_messages_from_events(&[
        user("Hello"),
        SessionEvent::Control(SessionControlEvent::AgentSwitched { from: None, to: Some("coder".into()) }),
        complete_text("message-1", "Hi there!"),
        ended(),
    ]);

    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], llm::ChatMessage::User { .. }));
    assert!(matches!(messages[1], llm::ChatMessage::Assistant { .. }));
}

#[test]
fn reconstructs_successful_tool_calls() {
    let context = context_from_events(&[
        user("Read Cargo.toml"),
        SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest { id: "call-1".into(), name: "read_file".into(), arguments: "{}".into() },
        })),
        tool_result("call-1", "read_file", "file contents"),
        complete_text("message-1", "Here is the file"),
        ended(),
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
        user("Read missing.txt"),
        SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Error {
            error: llm::ToolCallError {
                id: "call-1".into(),
                name: "read_file".into(),
                arguments: Some(r#"{"path":"missing.txt"}"#.into()),
                error: "file not found".into(),
            },
        })),
        ended(),
        SessionEvent::Agent(AgentEvent::Context(ContextEvent::Cleared)),
        user("Start fresh"),
    ];
    let context = context_from_events(&events);
    let before_clear = context_from_events(&events[..3]);

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
    assert_eq!(context_from_events(&[ended()]).message_count(), 0);
}

#[test]
fn completed_turns_reset_the_accumulator() {
    let context = context_from_events(&[
        complete_text("message-1", "Turn 1"),
        ended(),
        complete_text("message-2", "Turn 2"),
        ended(),
    ]);

    assert_eq!(context.message_count(), 2);
}

#[test]
fn compaction_replaces_prior_messages_with_a_summary() {
    let context = context_from_events(&[
        user("Hello"),
        complete_text("message-1", "Hi!"),
        ended(),
        SessionEvent::Agent(AgentEvent::Context(ContextEvent::CompactionResult {
            summary: "Earlier we greeted each other.".into(),
            messages_removed: 2,
        })),
        user("What did we talk about?"),
    ]);

    assert_eq!(context.message_count(), 2);
    assert!(context.messages()[0].is_summary());
}

#[test]
fn complete_messages_are_reconstructed_but_streaming_chunks_are_not() {
    let context = context_from_events(&[
        SessionEvent::Agent(AgentEvent::text("partial", "partial", StreamState::Partial)),
        SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
            message_id: "complete".into(),
            chunk: "complete".into(),
            is_complete: true,
        })),
        ended(),
    ]);

    assert_eq!(context.message_count(), 1);
    assert!(matches!(&context.messages()[0], llm::ChatMessage::Assistant { content, .. } if content == "complete"));
}
