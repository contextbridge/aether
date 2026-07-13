use std::sync::Arc;

use aether_core::context::CompactionConfig;
use aether_core::events::{
    AgentEvent, Command, ContextEvent, LlmCallOutcome, LlmCallPurpose, TurnEvent, TurnOutcome, UserCommand,
};
use aether_core::testing::{TestAgentStep, test_agent};
use llm::testing::llm_response;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock};
use tokio::sync::Notify;

fn user_message(text: &str) -> ChatMessage {
    ChatMessage::User { content: vec![ContentBlock::text(text)], timestamp: IsoString::now() }
}

#[tokio::test]
async fn oversized_context_is_compacted_before_the_llm_call() {
    let result = test_agent()
        .llm_responses(&[llm_response("sum").text(&["summary"]).build(), llm_response("msg").text(&["hello"]).build()])
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![user_message(&"x".repeat(400))])
        .user_text("go")
        .run_with_context()
        .await
        .unwrap();

    let contexts = result.captured_contexts.lock().unwrap();
    let messages = contexts.last().expect("chat request should reach the provider").messages();
    assert!(
        matches!(&messages[0], ChatMessage::Summary { content, .. } if content == "summary"),
        "expected the prior conversation compacted into a summary, got {messages:?}"
    );
    assert!(
        matches!(&messages[1], ChatMessage::User { content, .. } if content == &vec![ContentBlock::text("go")]),
        "the fresh user message must survive compaction as a real turn, got {messages:?}"
    );
}

#[tokio::test]
async fn cancel_during_compaction_ends_the_turn() {
    let release = Arc::new(Notify::new());
    let trace = test_agent()
        .llm_responses(&[llm_response("sum").text(&["summary"]).build()])
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![user_message(&"x".repeat(400))])
        .pause_turn_after(0, 0, release)
        .scenario(vec![
            TestAgentStep::send(Command::UserCommand(UserCommand::Text { content: vec![ContentBlock::text("go")] })),
            TestAgentStep::wait_for(|event| {
                matches!(event, AgentEvent::Context(ContextEvent::CompactionStarted { .. }))
            }),
            TestAgentStep::send(Command::UserCommand(UserCommand::Cancel)),
            TestAgentStep::wait_for(|event| matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. }))),
        ])
        .run_trace()
        .await
        .unwrap();

    let events = trace.events();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::Turn(TurnEvent::LlmCallEnded {
                purpose: LlmCallPurpose::Compaction,
                outcome: LlmCallOutcome::Cancelled,
            })
        )),
        "cancel should end the in-flight compaction call"
    );
    assert!(
        matches!(events.last(), Some(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Cancelled }))),
        "the turn must end as cancelled"
    );
}

/// Cancelling a compaction must preserve the prompt that triggered it for the
/// next turn.
#[tokio::test]
async fn cancel_during_compaction_preserves_the_initiating_message() {
    let release = Arc::new(Notify::new());
    let result = test_agent()
        .llm_responses(&[
            llm_response("sum").text(&["summary"]).build(),
            llm_response("sum2").text(&["summary"]).build(),
            llm_response("msg").text(&["hello"]).build(),
        ])
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![user_message(&"x".repeat(400))])
        .pause_turn_after(0, 0, release)
        .scenario(vec![
            TestAgentStep::send(Command::UserCommand(UserCommand::Text { content: vec![ContentBlock::text("go")] })),
            TestAgentStep::wait_for(|event| {
                matches!(event, AgentEvent::Context(ContextEvent::CompactionStarted { .. }))
            }),
            TestAgentStep::send(Command::UserCommand(UserCommand::Cancel)),
            TestAgentStep::wait_for(|event| matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. }))),
            TestAgentStep::send(Command::UserCommand(UserCommand::Text { content: vec![ContentBlock::text("next")] })),
            TestAgentStep::wait_for(|event| matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. }))),
        ])
        .run_with_context()
        .await
        .unwrap();

    let contexts = result.captured_contexts.lock().unwrap();
    let preserved = contexts.iter().any(|context| {
        context
            .messages()
            .iter()
            .any(|message| matches!(message, ChatMessage::User { content, .. } if content == &vec![ContentBlock::text("go")]))
    });
    assert!(
        preserved,
        "the message that initiated the cancelled turn must stay in the conversation, as it did before compaction became cancellable"
    );
}
