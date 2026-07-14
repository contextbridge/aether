use aether_core::events::{AgentEvent, MessageEvent, TurnEvent};
use aether_core::testing::{TestScenario, test_agent};
use llm::testing::llm_response;

/// After cancelling, a new prompt should produce a normal response.
/// Regression test: the agent's `cancelled` flag was never reset, so all
/// LLM events after the first cancel were silently dropped.
#[tokio::test]
async fn test_prompt_after_cancel_produces_response() {
    let events = test_agent()
        .llm_responses(&[
            llm_response("msg_1").text(&["Hello", " world", " this", " is", " a", " long", " response"]).build(),
            llm_response("msg_2").text(&["Second response"]).build(),
        ])
        .scenario(
            TestScenario::new()
                .user_text("first question")
                .cancel()
                .wait_for_turn_end()
                .user_text("second question")
                .wait_for_turn_end(),
        )
        .run()
        .await
        .unwrap();

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::Turn(TurnEvent::Ended { outcome: aether_core::events::TurnOutcome::Cancelled })
        )),
        "expected the first turn to be cancelled"
    );

    let first_ended = events.iter().position(|e| matches!(e, AgentEvent::Turn(TurnEvent::Ended { .. }))).unwrap();
    let second_turn = &events[first_ended + 1..];

    assert!(
        second_turn.iter().any(|e| matches!(e, AgentEvent::Message(MessageEvent::Text { chunk, is_complete: false, .. }) if chunk == "Second response")),
        "expected streamed text from the second prompt"
    );
    assert!(
        second_turn.iter().any(|e| matches!(e, AgentEvent::Turn(TurnEvent::Ended { .. }))),
        "expected TurnEnded from the second prompt"
    );
}
