use std::sync::Arc;

use aether_core::context::CompactionConfig;
use aether_core::events::{AgentEvent, CompactionOutcome, ContextEvent, LlmCallOutcome, TurnEvent, TurnOutcome};
use aether_core::testing::{TestScenario, test_agent};
use llm::LlmCallPurpose;
use llm::testing::llm_response;
use llm::{ChatMessage, ContentBlock};
use tokio::sync::Notify;

#[tokio::test]
async fn consecutive_compactions_have_unique_ids_and_ordered_terminal_events() {
    let trace = test_agent()
        .llm_responses(&[
            llm_response().text(&[&"s".repeat(400)]).build(),
            llm_response().text(&["one"]).build(),
            llm_response().text(&["summary"]).build(),
            llm_response().text(&["two"]).build(),
        ])
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![user_message(&"x".repeat(400))])
        .scenario(TestScenario::new().user_text("go").wait_for_turn_end().user_text("next").wait_for_turn_end())
        .run_trace()
        .await
        .unwrap();
    let lifecycle: Vec<_> = trace
        .events()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Context(
                context @ (ContextEvent::CompactionStarted { .. }
                | ContextEvent::CompactionResult { .. }
                | ContextEvent::CompactionEnded { .. }),
            ) => Some(context),
            _ => None,
        })
        .collect();
    assert_eq!(lifecycle.len(), 6);
    let mut ids = Vec::new();
    for operation in lifecycle.as_chunks::<3>().0 {
        let [
            ContextEvent::CompactionStarted { compaction_id: start, .. },
            ContextEvent::CompactionResult { compaction_id: result, message_id, .. },
            ContextEvent::CompactionEnded { compaction_id: end, outcome: CompactionOutcome::Completed },
        ] = operation
        else {
            panic!("start, result, completed end required: {operation:?}")
        };
        assert_eq!(start, result);
        assert_eq!(start, end);
        assert_ne!(start.as_str(), message_id.as_str());
        ids.push(start);
    }
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn failed_compaction_retains_identity_and_error_without_a_result() {
    let trace = test_agent()
        .llm_responses(&[
            vec![llm::LlmResponse::Error { message: "provider unavailable".into() }],
            llm_response().text(&["reply"]).build(),
        ])
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![user_message(&"x".repeat(400))])
        .user_text("go")
        .run_trace()
        .await
        .unwrap();
    let start = trace
        .events()
        .iter()
        .find_map(|event| match event {
            AgentEvent::Context(ContextEvent::CompactionStarted { compaction_id, .. }) => Some(compaction_id),
            _ => None,
        })
        .unwrap();
    assert!(trace.events().iter().any(|event| matches!(event, AgentEvent::Context(ContextEvent::CompactionEnded { compaction_id, outcome: CompactionOutcome::Failed { error } }) if compaction_id == start && error.contains("provider unavailable"))));
    assert!(
        !trace.events().iter().any(|event| matches!(event, AgentEvent::Context(ContextEvent::CompactionResult { .. })))
    );
}

fn user_message(text: &str) -> ChatMessage {
    ChatMessage::user(text)
}

#[tokio::test]
async fn oversized_context_is_compacted_before_the_llm_call() {
    let result = test_agent()
        .llm_responses(&[llm_response().text(&["summary"]).build(), llm_response().text(&["hello"]).build()])
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
        .llm_responses(&[llm_response().text(&["summary"]).build()])
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![user_message(&"x".repeat(400))])
        .pause_turn_after(0, 0, release)
        .scenario(TestScenario::new().user_text("go").wait_for_compaction_start().cancel().wait_for_turn_end())
        .run_trace()
        .await
        .unwrap();

    let events = trace.events();
    let lifecycle: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Context(
                context @ (ContextEvent::CompactionStarted { .. }
                | ContextEvent::CompactionEnded { .. }
                | ContextEvent::CompactionResult { .. }),
            ) => Some(serde_json::to_value(context).unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(lifecycle.len(), 2);
    let id = lifecycle[0]["compaction_id"].as_str().expect("persisted compaction identity");
    assert_eq!(lifecycle[1]["compaction_id"], id);
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
        events.iter().any(|event| matches!(
            event,
            AgentEvent::Context(ContextEvent::CompactionEnded { outcome: CompactionOutcome::Cancelled, .. })
        )),
        "cancel should end the semantic compaction lifecycle"
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
            llm_response().text(&["summary"]).build(),
            llm_response().text(&["summary"]).build(),
            llm_response().text(&["hello"]).build(),
        ])
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![user_message(&"x".repeat(400))])
        .pause_turn_after(0, 0, release)
        .scenario(
            TestScenario::new()
                .user_text("go")
                .wait_for_compaction_start()
                .cancel()
                .wait_for_turn_end()
                .user_text("next")
                .wait_for_turn_end(),
        )
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
