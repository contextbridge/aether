use aether_core::events::{AgentEvent, MessageEvent, ToolEvent, TurnEvent};
use std::sync::Arc;

use aether_core::events::TurnOutcome;
use aether_core::testing::{TestScenario, test_agent};
use llm::testing::llm_response;
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StopReason};
use tokio::sync::Notify;

#[tokio::test]
async fn queued_text_does_not_cancel_active_stream_and_drains_into_single_turn() {
    let Scenario { messages, contexts } = run_queued_scenario(None, &["beep", "boop", "zap"]).await;

    assert!(!messages.iter().any(|m| matches!(m.turn_outcome(), Some(TurnOutcome::Cancelled))),);
    assert_eq!(complete_text(&messages, "msg_1").as_deref(), Some("hello world"));
    assert_eq!(complete_text(&messages, "msg_2").as_deref(), Some("next turn"));
    assert_eq!(contexts.len(), 2);

    let drained = "beep\nboop\nzap";
    assert_eq!(user_texts(&contexts[1]), vec!["original prompt", drained]);
    assert!(assistant_index(&contexts[1], "hello world") < user_index(&contexts[1], drained),);
}

#[tokio::test]
async fn queued_text_suppresses_intermediate_done_between_turns() {
    let Scenario { messages, .. } = run_queued_scenario(None, &["beep", "boop", "zap"]).await;
    let first_complete = messages
        .iter()
        .position(|m| is_complete_text(m, "msg_1", "hello world"))
        .expect("Expected complete text for first turn");

    let second_stream =
        messages.iter().position(|m| is_partial_text_for(m, "msg_2")).expect("Expected streamed text for second turn");

    assert!(
        !messages[first_complete + 1..second_stream]
            .iter()
            .any(|m| matches!(m, AgentEvent::Turn(TurnEvent::Ended { .. }))),
    );
    assert_eq!(messages.iter().filter(|m| matches!(m, AgentEvent::Turn(TurnEvent::Ended { .. }))).count(), 1);
}

#[tokio::test]
async fn user_message_during_tool_execution_is_queued() {
    let request_json = serde_json::json!({ "a": 2, "b": 3 }).to_string();
    let turns = vec![
        llm_response("msg_1").tool_call("call_1", "test__add_numbers", &[&request_json]).build(),
        vec![LlmResponse::start("msg_2"), LlmResponse::text("done"), LlmResponse::done()],
    ];

    // Pause turn 1 right after ToolRequestStart (chunk index 1). At that point the
    // agent has populated `active_requests` and emitted `ToolCall`, so `is_busy()`
    // returns true even though no real tool work has begun.
    let release = Arc::new(Notify::new());
    let result = test_agent()
        .llm_responses(&turns)
        .pause_turn_after(0, 1, Arc::clone(&release))
        .scenario(
            TestScenario::new()
                .user_text("add 2 and 3")
                .wait_for(|m| matches!(m, AgentEvent::Tool(ToolEvent::Call { request, .. }) if request.id == "call_1"))
                .user_text("now add 10 and 20")
                .perform(move || release.notify_one())
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await
        .expect("Agent should run");

    let contexts = result.captured_contexts.lock().expect("captured contexts lock poisoned").clone();
    assert_eq!(contexts.len(), 2, "Expected exactly two LLM calls");

    let second_call = &contexts[1];
    assert!(
        user_texts(second_call).contains(&"now add 10 and 20".to_string()),
        "Queued message should appear in second LLM context, got: {:?}",
        user_texts(second_call),
    );
    assert!(
        second_call.messages().iter().any(|m| matches!(m, ChatMessage::ToolCallResult(Ok(r)) if r.id == "call_1")),
        "Tool result should remain in context after the queued message drains",
    );
}

#[tokio::test]
async fn queued_text_takes_precedence_over_auto_continue() {
    let Scenario { messages, contexts } = run_queued_scenario(Some(StopReason::Length), &["beep"]).await;

    assert!(
        !messages.iter().any(|m| matches!(m, AgentEvent::Turn(TurnEvent::AutoContinue { .. }))),
        "Expected queued text to suppress auto-continue, got: {messages:?}",
    );
    assert_eq!(contexts.len(), 2);
    assert_eq!(user_texts(&contexts[1]), vec!["original prompt", "beep"]);
    assert!(
        !contexts[1].messages().iter().any(|m| matches!(
            m,
            ChatMessage::User { content, .. }
                if ContentBlock::join_text(content).contains("<system-notification>")
        )),
        "Continuation prompt should be suppressed when queued text exists",
    );
}

struct Scenario {
    messages: Vec<AgentEvent>,
    contexts: Vec<Context>,
}

/// Drives a two-turn conversation where one or more user messages are queued
/// while the first turn's LLM stream is deliberately paused mid-flight.
async fn run_queued_scenario(first_stop_reason: Option<StopReason>, queued: &[&str]) -> Scenario {
    let first_done = first_stop_reason.map_or_else(LlmResponse::done, LlmResponse::done_with_stop_reason);
    let turns = vec![
        vec![LlmResponse::start("msg_1"), LlmResponse::text("hello"), LlmResponse::text(" world"), first_done],
        vec![LlmResponse::start("msg_2"), LlmResponse::text("next turn"), LlmResponse::done()],
    ];

    let release = Arc::new(Notify::new());
    let mut scenario =
        TestScenario::new().user_text("original prompt").wait_for(|m| is_partial_text(m, "msg_1", "hello"));
    for text in queued {
        scenario = scenario.user_text(*text);
    }

    let result = test_agent()
        .without_mcp()
        .llm_responses(&turns)
        .pause_turn_after(0, 1, Arc::clone(&release))
        .scenario(scenario.perform(move || release.notify_one()).wait_for_turn_end())
        .run_with_context()
        .await
        .expect("Agent should run");

    Scenario {
        messages: result.messages,
        contexts: result.captured_contexts.lock().expect("captured contexts lock poisoned").clone(),
    }
}

fn is_partial_text(m: &AgentEvent, id: &str, chunk: &str) -> bool {
    matches!(m, AgentEvent::Message(MessageEvent::Text { message_id, chunk: c, is_complete: false, .. }) if message_id == id && c == chunk)
}

fn is_partial_text_for(m: &AgentEvent, id: &str) -> bool {
    matches!(m, AgentEvent::Message(MessageEvent::Text { message_id, is_complete: false, .. }) if message_id == id)
}

fn is_complete_text(m: &AgentEvent, id: &str, chunk: &str) -> bool {
    matches!(m, AgentEvent::Message(MessageEvent::Text { message_id, chunk: c, is_complete: true, .. }) if message_id == id && c == chunk)
}

fn complete_text(messages: &[AgentEvent], id: &str) -> Option<String> {
    messages.iter().find_map(|m| match m {
        AgentEvent::Message(MessageEvent::Text { message_id, chunk, is_complete: true, .. }) if message_id == id => {
            Some(chunk.clone())
        }
        _ => None,
    })
}

fn user_texts(context: &Context) -> Vec<String> {
    context
        .messages()
        .iter()
        .filter_map(|m| match m {
            ChatMessage::User { content, .. } => Some(ContentBlock::join_text(content)),
            _ => None,
        })
        .collect()
}

fn user_index(context: &Context, text: &str) -> usize {
    context
        .messages()
        .iter()
        .position(|m| matches!(m, ChatMessage::User { content, .. } if ContentBlock::join_text(content) == text))
        .unwrap_or_else(|| panic!("Expected user message {text:?}"))
}

fn assistant_index(context: &Context, text: &str) -> usize {
    context
        .messages()
        .iter()
        .position(|m| matches!(m, ChatMessage::Assistant { content, .. } if content == text))
        .unwrap_or_else(|| panic!("Expected assistant message {text:?}"))
}
