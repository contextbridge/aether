use aether_core::core::agent;
use aether_core::events::{AgentCommand, AgentEvent, Command, UserCommand};
use aether_core::events::{ContextEvent, ModelEvent, TurnEvent};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;

#[tokio::test]
async fn test_switch_model_emits_model_switched() {
    // The switched-to provider will produce this response
    let switch_responses =
        vec![vec![LlmResponse::start("after-switch"), LlmResponse::text("Switched!"), LlmResponse::done()]];
    let new_provider = FakeLlmProvider::new(switch_responses);

    // Initial LLM produces a response, then we switch
    let initial_responses = vec![vec![LlmResponse::start("msg-1"), LlmResponse::text("Hello"), LlmResponse::done()]];
    let llm = FakeLlmProvider::new(initial_responses);

    let (tx, mut rx, _handle) = agent(llm).spawn().await.unwrap();

    // Send initial message
    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("hi")] })).await.unwrap();

    // Wait for initial response to complete
    let mut got_initial_done = false;
    while let Some(msg) = rx.recv().await {
        if matches!(msg, AgentEvent::Turn(TurnEvent::Ended { .. })) {
            got_initial_done = true;
            break;
        }
    }
    assert!(got_initial_done, "Expected Done after initial message");

    // Switch models by sending a ready-to-use provider
    tx.send(Command::AgentCommand(AgentCommand::SwitchModel(Box::new(new_provider)))).await.unwrap();

    // Send a follow-up message to exercise the new provider
    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("after switch")] }))
        .await
        .unwrap();
    drop(tx);

    // Collect remaining messages
    let mut messages = Vec::new();
    while let Some(msg) = rx.recv().await {
        messages.push(msg);
    }

    // Should have ModelSwitched with display name strings
    let switched = messages.iter().find(|m| matches!(m, AgentEvent::Model(ModelEvent::Switched { .. })));
    assert!(switched.is_some(), "Expected ModelSwitched message, got: {messages:?}");
    if let Some(AgentEvent::Model(ModelEvent::Switched { previous, new })) = switched {
        // FakeLlmProvider::display_name() returns "Fake LLM"
        assert_eq!(previous, "Fake LLM");
        assert_eq!(new, "Fake LLM");
    }
}

#[tokio::test]
async fn test_switch_model_unknown_context_limit_resets_context_meter() {
    let initial_provider = FakeLlmProvider::from_results(vec![vec![
        Ok(LlmResponse::start("msg-1")),
        Ok(LlmResponse::usage(1000, 50)),
        Ok(LlmResponse::text("Hello")),
        Ok(LlmResponse::done()),
    ]])
    .with_context_window(Some(200_000));

    let unknown_limit_provider = FakeLlmProvider::from_results(vec![vec![
        Ok(LlmResponse::start("after-switch")),
        Ok(LlmResponse::text("Switched!")),
        Ok(LlmResponse::done()),
    ]])
    .with_context_window(None);

    let (tx, mut rx, _handle) = agent(initial_provider).spawn().await.unwrap();

    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("hi")] })).await.unwrap();
    while let Some(msg) = rx.recv().await {
        if matches!(msg, AgentEvent::Turn(TurnEvent::Ended { .. })) {
            break;
        }
    }

    tx.send(Command::AgentCommand(AgentCommand::SwitchModel(Box::new(unknown_limit_provider)))).await.unwrap();
    drop(tx);

    let mut messages = Vec::new();
    while let Some(msg) = rx.recv().await {
        messages.push(msg);
    }

    assert!(
        messages.iter().any(|m| matches!(m, AgentEvent::Model(ModelEvent::Switched { .. }))),
        "Expected ModelSwitched message, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| {
            matches!(
                m,
                AgentEvent::Context(ContextEvent::UsageUpdated { usage })
                    if usage.usage_ratio.is_none() && usage.context_limit.is_none() && usage.input_tokens == 0
            )
        }),
        "Expected context usage reset for unknown context limit, got: {messages:?}"
    );
}
