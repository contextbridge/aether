use aether_core::events::{AgentEvent, ContextEvent, ModelEvent};
use aether_core::testing::{TestScenario, test_agent};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;

#[tokio::test]
async fn test_switch_model_emits_model_switched() -> Result<(), Box<dyn std::error::Error>> {
    // The switched-to provider will produce this response
    let new_provider = FakeLlmProvider::new(vec![vec![
        LlmResponse::start("after-switch"),
        LlmResponse::text("Switched!"),
        LlmResponse::done(),
    ]]);

    // Initial LLM produces a response, then we switch
    let events = test_agent()
        .without_mcp()
        .llm_responses(&[vec![LlmResponse::start("msg-1"), LlmResponse::text("Hello"), LlmResponse::done()]])
        .scenario(
            TestScenario::new()
                .user_text("hi")
                .wait_for_turn_end()
                .switch_model(new_provider)
                .user_text("after switch")
                .wait_for_turn_end(),
        )
        .run()
        .await?;

    // Should have ModelSwitched with display name strings
    let switched = events.iter().find(|m| matches!(m, AgentEvent::Model(ModelEvent::Switched { .. })));
    assert!(switched.is_some(), "Expected ModelSwitched message, got: {events:?}");
    if let Some(AgentEvent::Model(ModelEvent::Switched { previous, new })) = switched {
        // FakeLlmProvider::display_name() returns "Fake LLM"
        assert_eq!(previous, "Fake LLM");
        assert_eq!(new, "Fake LLM");
    }
    Ok(())
}

#[tokio::test]
async fn test_switch_model_unknown_context_limit_resets_context_meter() -> Result<(), Box<dyn std::error::Error>> {
    let unknown_limit_provider = FakeLlmProvider::new(vec![vec![
        LlmResponse::start("after-switch"),
        LlmResponse::text("Switched!"),
        LlmResponse::done(),
    ]])
    .with_context_window(None);

    let events = test_agent()
        .without_mcp()
        .provider_context_window(Some(200_000))
        .llm_responses(&[vec![
            LlmResponse::start("msg-1"),
            LlmResponse::usage(1000, 50),
            LlmResponse::text("Hello"),
            LlmResponse::done(),
        ]])
        .scenario(
            TestScenario::new().user_text("hi").wait_for_turn_end().switch_model(unknown_limit_provider).wait_for(
                |event| {
                    matches!(
                        event,
                        AgentEvent::Context(ContextEvent::UsageUpdated { usage })
                            if usage.usage_ratio.is_none() && usage.context_limit.is_none() && usage.input_tokens.is_zero()
                    )
                },
            ),
        )
        .run()
        .await?;

    assert!(
        events.iter().any(|m| matches!(m, AgentEvent::Model(ModelEvent::Switched { .. }))),
        "Expected ModelSwitched message, got: {events:?}"
    );
    assert!(
        events.iter().any(|m| {
            matches!(
                m,
                AgentEvent::Context(ContextEvent::UsageUpdated { usage })
                    if usage.usage_ratio.is_none() && usage.context_limit.is_none() && usage.input_tokens.is_zero()
            )
        }),
        "Expected context usage reset for unknown context limit, got: {events:?}"
    );
    Ok(())
}
