use std::time::Duration;

use aether_core::core::{Prompt, agent};
use aether_core::events::{AgentMessage, Command, UserCommand};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;
use llm::{ChatMessage, ContentBlock};
use tokio::sync::mpsc;

#[tokio::test]
async fn test_clear_context_resets_history_and_preserves_system_prompt() {
    let llm_responses = vec![
        vec![LlmResponse::start("msg_1"), LlmResponse::text("First response"), LlmResponse::done()],
        vec![LlmResponse::start("msg_2"), LlmResponse::text("Second response"), LlmResponse::done()],
    ];

    let llm = FakeLlmProvider::new(llm_responses);
    let captured_contexts = llm.captured_contexts();

    let (tx, mut rx, _handle) = agent(llm).system_prompt(Prompt::text("You are a test agent.")).spawn().await.unwrap();

    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("first question")] }))
        .await
        .unwrap();
    drain_until_done(&mut rx).await;

    tx.send(Command::UserCommand(UserCommand::ClearContext)).await.unwrap();
    let cleared = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("Timed out waiting for ContextCleared")
        .expect("Channel closed before ContextCleared");
    assert!(matches!(cleared, AgentMessage::ContextCleared), "Expected ContextCleared, got: {cleared:?}");

    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("second question")] }))
        .await
        .unwrap();
    drain_until_done(&mut rx).await;

    let contexts = captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2, "expected two LLM requests");

    let second = &contexts[1];
    let messages = second.messages();

    assert!(
        matches!(messages.first(), Some(ChatMessage::System { .. })),
        "system prompt should be preserved after clear"
    );

    let has_first_question = messages.iter().any(|m| {
        matches!(
            m,
            ChatMessage::User { content, .. } if *content == vec![ContentBlock::text("first question")]
        )
    });
    assert!(!has_first_question, "first turn user text should be removed from cleared context");

    let has_second_question = messages.iter().any(|m| {
        matches!(
            m,
            ChatMessage::User { content, .. } if *content == vec![ContentBlock::text("second question")]
        )
    });
    assert!(has_second_question, "new prompt should be present after clear");
}

async fn drain_until_done(rx: &mut mpsc::Receiver<AgentMessage>) {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Timed out waiting for Done")
            .expect("Channel closed before Done");
        if matches!(msg, AgentMessage::Done) {
            break;
        }
    }
}
