use aether_core::agent_spec::AgentSpec;
use aether_core::core::{AgentBuilder, AgentDeps, Command};
use aether_core::testing::{FakeObserverFactory, ObservedAgent, TestResult, drain_until};
use llm::testing::{FakeLlmProvider, llm_response};
use std::sync::Arc;

#[tokio::test]
async fn observers_use_the_conversation_id_regardless_of_provider_affinity() -> TestResult<()> {
    let deps = AgentDeps::default().with_conversation_id("conversation-42");
    let (observed, affinity) = observe_turn(deps).await?;
    assert_eq!(affinity, "provider-key");
    assert_eq!(observed.conversation_id.as_deref(), Some("conversation-42"));
    Ok(())
}

#[tokio::test]
async fn observers_do_not_substitute_provider_affinity_for_a_missing_conversation() -> TestResult<()> {
    let (observed, affinity) = observe_turn(AgentDeps::default()).await?;
    assert_eq!(affinity, "provider-key");
    assert_eq!(observed.conversation_id, None);
    Ok(())
}

async fn observe_turn(mut deps: AgentDeps) -> TestResult<(ObservedAgent, String)> {
    let factory = Arc::new(FakeObserverFactory::new());
    let agents = factory.agents();
    deps.observer_factory = Some(factory);
    let spec = AgentSpec { name: "planner".to_string(), ..AgentSpec::default() };
    let llm = FakeLlmProvider::with_single_response(llm_response().text(&["done"]).build());
    let contexts = llm.captured_contexts();
    let builder = AgentBuilder::from_spec_with_llm(&spec, &deps, Arc::new(llm));
    let (tx, mut rx, _agent) = builder.session_affinity_key("provider-key").spawn().await?;
    tx.send(Command::text("hello")).await?;
    drain_until(&mut rx, |event| event.turn_outcome().is_some()).await;

    let agents = agents.lock().unwrap();
    let [observed] = agents.as_slice() else { panic!("expected one observer, got {agents:?}") };
    assert_eq!(observed.agent_name.as_deref(), Some("planner"));
    let affinity = contexts.lock().unwrap()[0].session_affinity_key().expect("provider session affinity").to_owned();
    Ok((observed.clone(), affinity))
}
