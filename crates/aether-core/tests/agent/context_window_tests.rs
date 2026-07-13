use aether_core::events::{AgentCommand, AgentEvent, Command, ContextEvent, ContextUsage};
use aether_core::testing::{TestAgentStep, test_agent};
use llm::ModelSettings;
use llm::testing::{FakeLlmProvider, llm_response};

fn context_usage(events: &[AgentEvent]) -> ContextUsage {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Context(ContextEvent::UsageUpdated { usage }) => Some(usage.clone()),
            _ => None,
        })
        .expect("agent should emit context usage")
}

#[tokio::test]
async fn context_window_override_supplies_unknown_provider_limit() {
    let events = test_agent()
        .llm_responses(&[llm_response("msg").usage(100_000, 10).build()])
        .without_mcp()
        .provider_context_window(None)
        .context_window_override(200_000)
        .user_text("hello")
        .run()
        .await
        .unwrap();

    let usage = context_usage(&events);
    assert_eq!(usage.context_limit, Some(200_000));
    assert_eq!(usage.usage_ratio, Some(0.5));
    assert_eq!(usage.input_tokens, 100_000);
}

#[tokio::test]
async fn context_window_override_beats_provider_limit() {
    let events = test_agent()
        .llm_responses(&[llm_response("msg").usage(100_000, 10).build()])
        .without_mcp()
        .provider_context_window(Some(128_000))
        .context_window_override(200_000)
        .user_text("hello")
        .run()
        .await
        .unwrap();

    let usage = context_usage(&events);
    assert_eq!(usage.context_limit, Some(200_000));
    assert_eq!(usage.usage_ratio, Some(0.5));
}

/// Requires sequential messaging (SwitchModel after initial setup).
#[tokio::test]
async fn context_window_override_survives_model_switch() {
    let events = test_agent()
        .without_mcp()
        .provider_context_window(Some(128_000))
        .context_window_override(200_000)
        .scenario(vec![
            TestAgentStep::send(Command::AgentCommand(AgentCommand::SwitchModel(Box::new(
                FakeLlmProvider::new(vec![]).with_display_name("new fake").with_context_window(Some(32_000)),
            )))),
            TestAgentStep::wait_for(|event| matches!(event, AgentEvent::Context(ContextEvent::UsageUpdated { .. }))),
        ])
        .run()
        .await
        .unwrap();

    let usage = context_usage(&events);
    assert_eq!(usage.context_limit, Some(200_000));
    assert_eq!(usage.usage_ratio, Some(0.0));
}

#[tokio::test]
async fn spawn_applies_model_settings_to_context() {
    let settings = ModelSettings { temperature: Some(0.0), max_tokens: Some(64), ..Default::default() };

    let result = test_agent()
        .llm_responses(&[llm_response("msg").usage(10, 10).build()])
        .model_settings(settings.clone())
        .user_text("hello")
        .run_with_context()
        .await
        .unwrap();

    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts[0].model_settings(), &settings);
}
