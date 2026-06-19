use std::error::Error;

use aether_core::{
    events::{AgentMessage, Command, UserCommand},
    testing::test_agent,
};
use llm::{ChatMessage, LlmError, LlmResponse};

#[tokio::test]
async fn test_api_error_mid_stream_does_not_add_empty_assistant_message() -> Result<(), Box<dyn Error>> {
    // First call: Start → Err → Done (simulates HTTP 522 mid-stream)
    let error_response: Vec<Result<LlmResponse, LlmError>> = vec![
        Ok(LlmResponse::start("msg_1")),
        Err(LlmError::ApiError("HTTP 522: connection timed out".into())),
        Ok(LlmResponse::done()),
    ];

    // Second call: normal success (triggered by second user message)
    let success_response: Vec<Result<LlmResponse, LlmError>> =
        vec![Ok(LlmResponse::start("msg_2")), Ok(LlmResponse::text("Hello!")), Ok(LlmResponse::done())];

    // Only send the first user message to avoid race conditions.
    // After the error + Done cycle, we manually inspect captured contexts.
    let result = test_agent()
        .llm_result_responses(&[error_response, success_response])
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("first message")],
        })])
        .run_with_context()
        .await?;

    // The agent should NOT emit a Text{is_complete: true} with empty content.
    // That would mean an empty assistant message was added to context.
    let has_empty_complete_text = result.messages.iter().any(|m| {
        matches!(
            m,
            AgentMessage::Text {
                chunk,
                is_complete: true,
                ..
            } if chunk.is_empty()
        )
    });

    assert!(
        !has_empty_complete_text,
        "Agent must not emit a completed Text message with empty content after an API error. Messages: {:?}",
        result.messages
    );

    // Should end with Done
    assert!(
        matches!(result.messages.last(), Some(AgentMessage::Done)),
        "Expected Done message, got: {:?}",
        result.messages.last()
    );

    // Only one LLM call should have been made (the errored one)
    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 1, "Expected exactly one LLM call (the errored one)");

    // That context should only contain the user message — no empty assistant message
    let has_empty_assistant = contexts[0].messages().iter().any(|msg| match msg {
        ChatMessage::Assistant { content, tool_calls, .. } => content.is_empty() && tool_calls.is_empty(),
        _ => false,
    });

    assert!(
        !has_empty_assistant,
        "Context must not contain an empty assistant message. Messages: {:?}",
        contexts[0].messages()
    );

    Ok(())
}
