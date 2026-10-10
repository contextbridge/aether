use aether_core::events::{AgentEvent, Command, MessageEvent, ToolEvent, TurnEvent, UserCommand};
use std::sync::Arc;

use aether_core::events::TurnOutcome;
use aether_core::testing::{AgentTrace, TestResult, TestScenario, test_agent};
use llm::testing::llm_response;
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StopReason};
use tokio::sync::Notify;

#[tokio::test]
async fn queued_text_does_not_cancel_active_stream_and_drains_into_single_turn() {
    let Scenario { messages, contexts } = run_queued_scenario(None, &["beep", "boop", "zap"]).await;

    assert!(!messages.iter().any(|m| matches!(m.turn_outcome(), Some(TurnOutcome::Cancelled))),);
    assert_eq!(complete_text(&messages, 0).as_deref(), Some("hello world"));
    assert_eq!(complete_text(&messages, 1).as_deref(), Some("next turn"));
    assert_eq!(contexts.len(), 2);

    assert_eq!(user_texts(&contexts[1]), vec!["original prompt", "beep", "boop", "zap"]);
    assert!(assistant_index(&contexts[1], "hello world") < user_index(&contexts[1], "beep"));
    let user_ids: std::collections::HashSet<_> = contexts[1]
        .messages()
        .iter()
        .filter(|message| matches!(message, ChatMessage::User { .. }))
        .map(ChatMessage::message_id)
        .collect();
    assert_eq!(user_ids.len(), 4);
}

#[tokio::test]
async fn queued_text_suppresses_intermediate_done_between_turns() {
    let Scenario { messages, .. } = run_queued_scenario(None, &["beep", "boop", "zap"]).await;
    let first_complete = messages
        .iter()
        .position(|m| is_complete_text(m, "hello world"))
        .expect("Expected complete text for first turn");

    let second_stream =
        messages.iter().position(|m| is_partial_text(m, "next turn")).expect("Expected streamed text for second turn");

    assert!(
        !messages[first_complete + 1..second_stream]
            .iter()
            .any(|m| matches!(m, AgentEvent::Turn(TurnEvent::Ended { .. }))),
    );
    assert_eq!(messages.iter().filter(|m| matches!(m, AgentEvent::Turn(TurnEvent::Ended { .. }))).count(), 1);
}

#[tokio::test]
async fn user_message_during_tool_input_streaming_is_queued() {
    let request_json = serde_json::json!({ "a": 2, "b": 3 }).to_string();
    let turns = vec![
        llm_response().tool_call("call_1", "test__add_numbers", &[&request_json]).build(),
        llm_response().text(&["done"]).build(),
    ];

    let release = Arc::new(Notify::new());
    let result = test_agent()
        .llm_responses(&turns)
        .pause_turn_after(0, 1, Arc::clone(&release))
        .scenario(
            TestScenario::new()
                .user_text("add 2 and 3")
                .wait_for(|m| matches!(m, AgentEvent::Tool(ToolEvent::InputStarted { id, .. }) if id == "call_1"))
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

#[tokio::test]
async fn queued_text_is_inserted_after_the_current_reply() {
    let trace = AgentTrace::from_events(run_queued_scenario(None, &["beep"]).await.messages);
    let inserted = trace.positions(is_user_message_inserted);
    let reply = trace.position(|m| is_complete_text(m, "hello world"));
    let next_reply = trace.position(|m| is_partial_text(m, "next turn"));

    assert_eq!(inserted.len(), 2, "the prompt and the queued text are each inserted once: {:?}", trace.events());
    assert!(inserted[0] < reply && reply < inserted[1] && inserted[1] < next_reply, "{:?}", trace.events());
}

#[tokio::test]
async fn text_arriving_after_a_turn_ends_starts_a_new_turn_while_the_finished_stream_closes() -> TestResult<()> {
    let turn_1 = llm_response().text(&["hello"]).build();
    let turn_2 = llm_response().text(&["next turn"]).build();
    let after_done = turn_1.len() - 1;
    let result = test_agent()
        .without_mcp()
        .llm_responses(&[turn_1, turn_2])
        .pause_turn_after(0, after_done, Arc::new(Notify::new()))
        .scenario(
            TestScenario::new().user_text("original prompt").wait_for_turn_end().user_text("beep").wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().expect("captured contexts lock poisoned").clone();
    assert_eq!(user_texts(&contexts[1]), vec!["original prompt", "beep"]);
    Ok(())
}

#[tokio::test]
async fn cancelled_turn_discards_queued_text() -> TestResult<()> {
    let first_turn = llm_response().text(&["hello", " world"]).build();
    let scenario = run_interrupted_scenario(first_turn, |scenario, _| scenario.cancel()).await?;
    assert_queued_text_discarded(scenario, &["original prompt", "again"]);
    Ok(())
}

#[tokio::test]
async fn failed_turn_discards_queued_text() -> TestResult<()> {
    let first_turn = llm_response().text(&["hello"]).build_ending_with_error("boom");
    let release_into_failure =
        |scenario: TestScenario, release: Arc<Notify>| scenario.perform(move || release.notify_one());
    let scenario = run_interrupted_scenario(first_turn, release_into_failure).await?;
    assert_queued_text_discarded(scenario, &["original prompt", "again"]);
    Ok(())
}

#[tokio::test]
async fn clearing_context_discards_queued_text() -> TestResult<()> {
    let first_turn = llm_response().text(&["hello", " world"]).build();
    let clear = |scenario: TestScenario, _| scenario.send(Command::UserCommand(UserCommand::ClearContext));
    let scenario = run_interrupted_scenario(first_turn, clear).await?;
    assert_queued_text_discarded(scenario, &["again"]);
    Ok(())
}

struct Scenario {
    messages: Vec<AgentEvent>,
    contexts: Vec<Context>,
}

/// Drives a two-turn conversation where one or more user messages are queued
/// while the first turn's LLM stream is deliberately paused mid-flight.
async fn run_queued_scenario(first_stop_reason: Option<StopReason>, queued: &[&str]) -> Scenario {
    let first_turn = match first_stop_reason {
        Some(stop_reason) => llm_response().text(&["hello", " world"]).build_with_stop_reason(stop_reason),
        None => llm_response().text(&["hello", " world"]).build(),
    };
    let turns = vec![first_turn, llm_response().text(&["next turn"]).build()];

    let release = Arc::new(Notify::new());
    let mut scenario = TestScenario::new().user_text("original prompt").wait_for(|m| is_partial_text(m, "hello"));
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

async fn run_interrupted_scenario(
    first_turn: Vec<LlmResponse>,
    interrupt: impl FnOnce(TestScenario, Arc<Notify>) -> TestScenario,
) -> TestResult<Scenario> {
    let turns = vec![first_turn, llm_response().text(&["again reply"]).build()];
    let release = Arc::new(Notify::new());
    let scenario =
        TestScenario::new().user_text("original prompt").user_text("beep").wait_for(|m| is_partial_text(m, "hello"));

    let result = test_agent()
        .without_mcp()
        .llm_responses(&turns)
        .pause_turn_after(0, 1, Arc::clone(&release))
        .scenario(interrupt(scenario, release).wait_for_turn_end().user_text("again").wait_for_turn_end())
        .run_with_context()
        .await?;

    Ok(Scenario {
        messages: result.messages,
        contexts: result.captured_contexts.lock().expect("captured contexts lock poisoned").clone(),
    })
}

fn assert_queued_text_discarded(Scenario { messages, contexts }: Scenario, final_user_texts: &[&str]) {
    let trace = AgentTrace::from_events(messages);
    let first_end = trace.position(|m| matches!(m, AgentEvent::Turn(TurnEvent::Ended { .. })));
    let inserted = trace.positions(is_user_message_inserted).into_iter().filter(|&index| index < first_end).count();
    let discarded = trace.positions(|m| matches!(m, AgentEvent::Turn(TurnEvent::UserMessageDiscarded { .. })));

    assert_eq!(inserted, 1, "only the prompt that started the turn is inserted: {:?}", trace.events());
    assert_eq!(discarded.len(), 1, "the queued text is discarded once: {:?}", trace.events());
    assert!(discarded[0] < first_end, "the queued text is discarded before the turn ends: {:?}", trace.events());
    assert_eq!(contexts.last().map(user_texts).unwrap_or_default(), final_user_texts);
}

fn is_user_message_inserted(m: &AgentEvent) -> bool {
    matches!(m, AgentEvent::Turn(TurnEvent::UserMessageInserted { .. }))
}

fn is_partial_text(m: &AgentEvent, chunk: &str) -> bool {
    matches!(m, AgentEvent::Message(MessageEvent::Text { chunk: c, is_complete: false, .. }) if c == chunk)
}

fn is_complete_text(m: &AgentEvent, chunk: &str) -> bool {
    matches!(m, AgentEvent::Message(MessageEvent::Text { chunk: c, is_complete: true, .. }) if c == chunk)
}

fn complete_text(messages: &[AgentEvent], index: usize) -> Option<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentEvent::Message(MessageEvent::Text { chunk, is_complete: true, .. }) => Some(chunk.clone()),
            _ => None,
        })
        .nth(index)
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
