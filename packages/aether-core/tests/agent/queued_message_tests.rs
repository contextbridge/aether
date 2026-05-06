use std::sync::Arc;

use aether_core::core::agent;
use aether_core::events::{AgentMessage, UserMessage};
use aether_core::mcp::McpSpawnResult;
use aether_core::mcp::mcp;
use aether_core::testing::{AddNumbersRequest, FakeMcpServer, fake_mcp};
use llm::testing::{FakeLlmProvider, llm_response};
use llm::{ChatMessage, ContentBlock, Context, LlmResponse, StopReason};
use tokio::sync::{Notify, mpsc};

#[tokio::test]
async fn queued_text_does_not_cancel_active_stream_and_drains_into_single_turn() {
    let Scenario { messages, contexts } = scenario(None, &["beep", "boop", "zap"]).await;

    assert!(!messages.iter().any(|m| matches!(m, AgentMessage::Cancelled { .. })),);
    assert_eq!(complete_text(&messages, "msg_1").as_deref(), Some("hello world"));
    assert_eq!(complete_text(&messages, "msg_2").as_deref(), Some("next turn"));
    assert_eq!(contexts.len(), 2);

    let drained = "beep\nboop\nzap";
    assert_eq!(user_texts(&contexts[1]), vec!["original prompt", drained]);
    assert!(assistant_index(&contexts[1], "hello world") < user_index(&contexts[1], drained),);
}

#[tokio::test]
async fn queued_text_suppresses_intermediate_done_between_turns() {
    let Scenario { messages, .. } = scenario(None, &["beep", "boop", "zap"]).await;
    let first_complete = messages
        .iter()
        .position(|m| is_complete_text(m, "msg_1", "hello world"))
        .expect("Expected complete text for first turn");

    let second_stream =
        messages.iter().position(|m| is_partial_text_for(m, "msg_2")).expect("Expected streamed text for second turn");

    assert!(!messages[first_complete + 1..second_stream].iter().any(|m| matches!(m, AgentMessage::Done)),);
    assert_eq!(messages.iter().filter(|m| matches!(m, AgentMessage::Done)).count(), 1);
}

#[tokio::test]
async fn user_message_during_tool_execution_is_queued() {
    let request_json = AddNumbersRequest::new(2, 3).json().expect("serialize tool request");
    let turns = vec![
        llm_response("msg_1").tool_call("call_1", "test__add_numbers", &[&request_json]).build(),
        vec![LlmResponse::start("msg_2"), LlmResponse::text("done"), LlmResponse::done()],
    ];

    // Pause turn 1 right after ToolRequestStart (chunk index 1). At that point the
    // agent has populated `active_requests` and emitted `ToolCall`, so `is_busy()`
    // returns true even though no real tool work has begun.
    let release = Arc::new(Notify::new());
    let llm = FakeLlmProvider::new(turns).pause_turn_after(0, 1, Arc::clone(&release));
    let captured = llm.captured_contexts();

    let McpSpawnResult { tool_definitions, command_tx: mcp_tx, .. } =
        mcp().with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await.expect("MCP should spawn");

    let (tx, mut rx, _handle) = agent(llm).tools(mcp_tx, tool_definitions).spawn().await.expect("Agent should spawn");
    tx.send(UserMessage::text("add 2 and 3")).await.expect("Initial prompt should send");

    drain_until(&mut rx, |m| matches!(m, AgentMessage::ToolCall { request, .. } if request.id == "call_1")).await;
    tx.send(UserMessage::text("now add 10 and 20")).await.expect("Queued message should send");
    release.notify_one();

    drain_until(&mut rx, |m| matches!(m, AgentMessage::Done)).await;
    drop(tx);

    let contexts = captured.lock().expect("captured contexts lock poisoned").clone();
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
    let Scenario { messages, contexts } = scenario(Some(StopReason::Length), &["beep"]).await;

    assert!(
        !messages.iter().any(|m| matches!(m, AgentMessage::AutoContinue { .. })),
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
    messages: Vec<AgentMessage>,
    contexts: Vec<Context>,
}

async fn scenario(first_stop_reason: Option<StopReason>, queued: &[&str]) -> Scenario {
    let first_done = first_stop_reason.map_or_else(LlmResponse::done, LlmResponse::done_with_stop_reason);
    let turns = vec![
        vec![LlmResponse::start("msg_1"), LlmResponse::text("hello"), LlmResponse::text(" world"), first_done],
        vec![LlmResponse::start("msg_2"), LlmResponse::text("next turn"), LlmResponse::done()],
    ];

    let release = Arc::new(Notify::new());
    let llm = FakeLlmProvider::new(turns).pause_turn_after(0, 1, Arc::clone(&release));
    let captured = llm.captured_contexts();

    let (tx, mut rx, _handle) = agent(llm).spawn().await.expect("Agent should spawn");

    tx.send(UserMessage::text("original prompt")).await.expect("Initial prompt should send");
    let mut messages = drain_until(&mut rx, |m| is_partial_text(m, "msg_1", "hello")).await;

    for text in queued {
        tx.send(UserMessage::text(text)).await.expect("Queued message should send");
    }
    release.notify_one();

    messages.extend(drain_until(&mut rx, |m| matches!(m, AgentMessage::Done)).await);
    drop(tx);

    let contexts = captured.lock().expect("captured contexts lock poisoned").clone();
    Scenario { messages, contexts }
}

async fn drain_until(rx: &mut mpsc::Receiver<AgentMessage>, stop: impl Fn(&AgentMessage) -> bool) -> Vec<AgentMessage> {
    let mut collected = Vec::new();
    loop {
        let m = rx.recv().await.expect("Channel should stay open");
        let done = stop(&m);
        collected.push(m);
        if done {
            break;
        }
    }
    collected
}

fn is_partial_text(m: &AgentMessage, id: &str, chunk: &str) -> bool {
    matches!(m, AgentMessage::Text { message_id, chunk: c, is_complete: false, .. } if message_id == id && c == chunk)
}

fn is_partial_text_for(m: &AgentMessage, id: &str) -> bool {
    matches!(m, AgentMessage::Text { message_id, is_complete: false, .. } if message_id == id)
}

fn is_complete_text(m: &AgentMessage, id: &str, chunk: &str) -> bool {
    matches!(m, AgentMessage::Text { message_id, chunk: c, is_complete: true, .. } if message_id == id && c == chunk)
}

fn complete_text(messages: &[AgentMessage], id: &str) -> Option<String> {
    messages.iter().find_map(|m| match m {
        AgentMessage::Text { message_id, chunk, is_complete: true, .. } if message_id == id => Some(chunk.clone()),
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
