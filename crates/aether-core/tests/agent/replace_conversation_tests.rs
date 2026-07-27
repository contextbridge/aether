use aether_core::core::Prompt;
use aether_core::events::{AgentEvent, ContextEvent};
use aether_core::testing::{TestScenario, test_agent};
use llm::testing::llm_response;
use llm::types::IsoString;
use llm::{AssistantReasoning, ChatMessage, ContentBlock};

#[tokio::test]
async fn replace_conversation_preserves_system_prompt_for_next_request() {
    let result = test_agent()
        .system_prompt(Prompt::text("original system"))
        .llm_responses(&[llm_response("msg").build()])
        .scenario(
            TestScenario::new()
                .replace_conversation(vec![
                    ChatMessage::user("old user"),
                    ChatMessage::Assistant {
                        content: "old assistant".to_string(),
                        reasoning: AssistantReasoning::default(),
                        timestamp: IsoString::now(),
                        tool_calls: vec![],
                    },
                ])
                .user_text("new user")
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await
        .unwrap();

    let contexts = result.captured_contexts.lock().unwrap();
    let messages = contexts.last().expect("provider should receive a context").messages();
    assert!(matches!(messages[0], ChatMessage::System { ref content, .. } if content == "original system"));
    assert!(
        matches!(messages[1], ChatMessage::User { ref content, .. } if content == &vec![ContentBlock::text("old user")])
    );
    assert!(matches!(messages[2], ChatMessage::Assistant { ref content, .. } if content == "old assistant"));
    assert!(
        matches!(messages[3], ChatMessage::User { ref content, .. } if content == &vec![ContentBlock::text("new user")])
    );
}

/// Token usage from a completed turn survives a subsequent conversation replacement.
#[tokio::test]
async fn replace_conversation_preserves_token_usage() {
    let events = test_agent()
        .llm_responses(&[llm_response("msg").usage(800, 10).build()])
        .provider_context_window(Some(1000))
        .scenario(
            TestScenario::new()
                .user_text("first user")
                .wait_for_turn_end()
                .replace_conversation(vec![ChatMessage::user("replacement user")])
                .wait_for(|event| matches!(event, AgentEvent::Context(ContextEvent::UsageUpdated { .. }))),
        )
        .run()
        .await
        .unwrap();

    let usage = events
        .iter()
        .rev()
        .find_map(|event| match event {
            AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => Some(usage),
            _ => None,
        })
        .expect("expected context usage update after conversation replacement");
    assert_eq!(usage.input_tokens, 800);
    assert_eq!(usage.usage_ratio, Some(0.8));
}
