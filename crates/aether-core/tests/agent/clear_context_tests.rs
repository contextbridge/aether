use aether_core::core::Prompt;
use aether_core::events::{AgentEvent, Command, ContextEvent, UserCommand};
use aether_core::testing::{TestScenario, test_agent};
use llm::{ChatMessage, ContentBlock, LlmResponse};

#[tokio::test]
async fn test_clear_context_resets_history_and_preserves_system_prompt() -> Result<(), Box<dyn std::error::Error>> {
    let result = test_agent()
        .without_mcp()
        .system_prompt(Prompt::text("You are a test agent."))
        .llm_responses(&[
            vec![LlmResponse::start("msg_1"), LlmResponse::text("First response"), LlmResponse::done()],
            vec![LlmResponse::start("msg_2"), LlmResponse::text("Second response"), LlmResponse::done()],
        ])
        .scenario(
            TestScenario::new()
                .user_text("first question")
                .wait_for_turn_end()
                .send(Command::UserCommand(UserCommand::ClearContext))
                .wait_for(|event| matches!(event, AgentEvent::Context(ContextEvent::Cleared)))
                .user_text("second question")
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
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

    Ok(())
}
