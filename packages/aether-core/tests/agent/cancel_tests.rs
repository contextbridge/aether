use std::time::Duration;

use aether_core::core::agent;
use aether_core::events::{AgentMessage, Command, UserCommand};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;

/// After cancelling, a new prompt should produce a normal response.
/// Regression test: the agent's `cancelled` flag was never reset, so all
/// LLM events after the first cancel were silently dropped.
#[tokio::test]
async fn test_prompt_after_cancel_produces_response() {
    let llm_responses = vec![
        // First prompt response (will be cancelled mid-stream)
        vec![
            LlmResponse::start("msg_1"),
            LlmResponse::text("Hello"),
            LlmResponse::text(" world"),
            LlmResponse::text(" this"),
            LlmResponse::text(" is"),
            LlmResponse::text(" a"),
            LlmResponse::text(" long"),
            LlmResponse::text(" response"),
            LlmResponse::done(),
        ],
        // Second prompt response (should be delivered normally)
        vec![LlmResponse::start("msg_2"), LlmResponse::text("Second response"), LlmResponse::done()],
    ];

    let llm = FakeLlmProvider::new(llm_responses);
    let (tx, mut rx, _handle) = agent(llm).spawn().await.unwrap();

    // Send first prompt
    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("first question")] }))
        .await
        .unwrap();

    // Send cancel immediately
    tx.send(Command::UserCommand(UserCommand::Cancel)).await.unwrap();

    // Drain until Done (from the cancel)
    loop {
        match rx.recv().await {
            Some(AgentMessage::Done) => break,
            Some(_) => {}
            None => panic!("Channel closed before Done"),
        }
    }

    // Send second prompt
    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("second question")] }))
        .await
        .unwrap();

    // Collect messages from the second prompt, with a timeout to catch the hang
    let mut got_text = false;
    let got_done;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(AgentMessage::Text { is_complete: false, .. })) => {
                got_text = true;
            }
            Ok(Some(AgentMessage::Done)) => {
                got_done = true;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("Channel closed before second Done"),
            Err(elapsed) => panic!("Timed out waiting for second prompt response — agent is stuck: {elapsed}"),
        }
    }

    assert!(got_text, "Expected text from the second prompt");
    assert!(got_done, "Expected Done from the second prompt");
}
