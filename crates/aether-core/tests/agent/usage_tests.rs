use aether_core::context::CompactionConfig;
use aether_core::core::RetryConfig;
use aether_core::events::{AgentEvent, LlmCallOutcome, SubAgentProgressPayload, ToolEvent, TurnEvent, TurnOutcome};
use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, TestScenario, test_agent};
use llm::alloyed::AlloyedModelProvider;
use llm::testing::{FakeLlmProvider, llm_response, priced_model, session_usage_event};
use llm::{
    ChatMessage, LlmCallPurpose, LlmError, LlmModel, LlmResponse, ProviderError, SessionUsageEvent, TokenUsage,
    UsageSource, Usd,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

#[tokio::test]
async fn provider_usage_emits_session_usage_before_call_end() {
    let events = test_agent()
        .llm_responses(&[llm_response("msg").usage(11, 7).build()])
        .without_mcp()
        .user_text("hello")
        .run()
        .await
        .unwrap();

    let usage_index = position(&events, |event| matches!(event, AgentEvent::SessionUsage(_)));
    let call_end_index = position(&events, |event| matches!(event, AgentEvent::Turn(TurnEvent::LlmCallEnded { .. })));
    assert!(usage_index < call_end_index);

    let usage = session_usage(&events)[0];
    assert_eq!(usage.sequence, 1);
    assert_eq!(usage.purpose, LlmCallPurpose::Chat);
    assert_eq!(usage.tokens.input_tokens.get(), 11);
    assert_eq!(usage.totals.tokens.output_tokens.get(), 7);
    assert!(usage.estimated_cost.is_none());
    assert_eq!(usage.totals.unpriced_calls, 1);
    assert_eq!(usage.totals.estimated_usd, Usd::ZERO);
}

#[tokio::test]
async fn priced_model_costs_each_call_and_keeps_totals_priced() {
    let events = test_agent()
        .model(priced_model())
        .llm_responses(&[llm_response("a").usage(1_000, 500).build(), llm_response("b").usage(2_000, 100).build()])
        .without_mcp()
        .scenario(TestScenario::new().user_text("one").wait_for_turn_end().user_text("two").wait_for_turn_end())
        .run()
        .await
        .unwrap();

    let usage = session_usage(&events);
    assert_eq!(usage.len(), 2);
    let first_cost = usage[0].estimated_cost.expect("catalog model is priced").total_usd;
    let second_cost = usage[1].estimated_cost.expect("catalog model is priced").total_usd;
    assert!(first_cost.get() > 0.0 && second_cost.get() > 0.0);
    assert!(usage[1].totals.is_fully_priced());
    assert!((usage[1].totals.estimated_usd.get() - (first_cost + second_cost).get()).abs() < 1e-12);
    assert_eq!(usage[1].sequence, 2);
    assert_eq!(usage[1].totals.tokens.input_tokens.get(), 3_000);
    assert!(usage[1].model.provider.is_some() && usage[1].model.model_id.is_some());
}

#[tokio::test]
async fn usage_received_before_a_stream_error_survives_the_failed_turn() {
    let events = test_agent()
        .llm_result_responses(&[vec![
            Ok(LlmResponse::start("msg")),
            Ok(LlmResponse::Usage { tokens: TokenUsage::new(9, 1) }),
            Err(LlmError::from(ProviderError::api("HTTP 500".to_string()))),
            Ok(LlmResponse::done()),
        ]])
        .without_mcp()
        .user_text("hello")
        .run()
        .await
        .unwrap();

    let usage_index = position(&events, |event| matches!(event, AgentEvent::SessionUsage(_)));
    let failed_index = position(&events, |event| {
        matches!(event, AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } }))
    });
    assert!(usage_index < failed_index);
    assert_eq!(session_usage(&events)[0].tokens.input_tokens.get(), 9);
}

#[tokio::test]
async fn usage_received_before_cancellation_survives_the_cancelled_turn() {
    let release = Arc::new(Notify::new());
    let events = test_agent()
        .llm_responses(&[llm_response("msg").usage(5, 3).text(&["never delivered"]).build()])
        .without_mcp()
        .pause_turn_after(0, 2, release)
        .scenario(
            TestScenario::new()
                .user_text("hello")
                .wait_for(|event| matches!(event, AgentEvent::SessionUsage(_)))
                .cancel()
                .wait_for_turn_end(),
        )
        .run()
        .await
        .unwrap();

    let usage_index = position(&events, |event| matches!(event, AgentEvent::SessionUsage(_)));
    let cancelled_index = position(&events, |event| {
        matches!(event, AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Cancelled }))
    });
    assert!(usage_index < cancelled_index);
    assert_eq!(session_usage(&events).len(), 1);
}

#[tokio::test]
async fn retried_attempts_without_usage_add_nothing() {
    let mut interrupted = vec![Ok(LlmResponse::start("msg_1"))];
    interrupted.push(Err(LlmError::from(ProviderError::stream_interrupted("retry".to_string()))));
    let recovered = llm_response("msg_2").usage(3, 2).text(&["ok"]).build().into_iter().map(Ok).collect::<Vec<_>>();

    let events = test_agent()
        .llm_result_responses(&[interrupted, recovered])
        .retry_config(RetryConfig {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        })
        .without_mcp()
        .user_text("hello")
        .run()
        .await
        .unwrap();

    let usage = session_usage(&events);
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].sequence, 1);
    assert_eq!(usage[0].totals.tokens.input_tokens.get(), 3);
}

#[tokio::test]
async fn compaction_usage_is_recorded_once_before_the_compaction_call_ends() {
    let events = test_agent()
        .llm_responses(&[
            llm_response("sum").usage(50, 10).text(&["summary"]).build(),
            llm_response("msg").usage(20, 5).text(&["hello"]).build(),
        ])
        .without_mcp()
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![ChatMessage::user("x".repeat(400))])
        .user_text("go")
        .run()
        .await
        .unwrap();

    let usage = session_usage(&events);
    assert_eq!(usage.len(), 2);
    assert_eq!(usage[0].purpose, LlmCallPurpose::Compaction);
    assert_eq!(usage[0].tokens.input_tokens.get(), 50);
    assert_eq!(usage[1].purpose, LlmCallPurpose::Chat);
    assert_eq!(usage[1].totals.tokens.input_tokens.get(), 70);
    assert_eq!(usage[1].totals.tokens.output_tokens.get(), 15);

    let compaction_usage_index = position(
        &events,
        |event| matches!(event, AgentEvent::SessionUsage(usage) if usage.purpose == LlmCallPurpose::Compaction),
    );
    let compaction_end_index = position(&events, |event| {
        matches!(
            event,
            AgentEvent::Turn(TurnEvent::LlmCallEnded {
                purpose: LlmCallPurpose::Compaction,
                outcome: LlmCallOutcome::Completed { .. }
            })
        )
    });
    assert!(compaction_usage_index < compaction_end_index);
}

#[tokio::test]
async fn sub_agent_usage_is_folded_into_the_parent_totals_before_the_tool_result() {
    let mut child_usage = session_usage_event(9, TokenUsage::new(8, 4));
    child_usage.source = UsageSource::new("explorer");
    let payload = SubAgentProgressPayload {
        task_id: "task_0".to_string(),
        agent_name: "explorer".to_string(),
        event: AgentEvent::SessionUsage(child_usage.clone()),
    };
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("spawn")
            .responds(FakeToolResponse::text("done").progress_message(0.0, serde_json::to_string(&payload).unwrap())),
    );
    let arguments = serde_json::json!({}).to_string();

    let events = test_agent()
        .fake_mcp_server("agents", server)
        .llm_responses(&[
            llm_response("msg_1").tool_call("spawn-call", "agents__spawn", &[&arguments]).build(),
            llm_response("msg_2").usage(1, 1).text(&["all done"]).build(),
        ])
        .user_text("delegate")
        .run()
        .await
        .unwrap();

    let usage = session_usage(&events);
    assert_eq!(usage.len(), 2, "child usage must reach the parent stream exactly once: {events:?}");
    let (folded, own) = (usage[0], usage[1]);
    assert_eq!(folded.sequence, 1, "the parent re-sequences child samples");
    assert_eq!(folded.source.agent_id, child_usage.source.agent_id);
    assert_eq!(folded.source.agent_name, "explorer");
    assert_eq!(folded.source.parent_agent_id.as_deref(), Some(own.source.agent_id.as_str()));
    assert_eq!(folded.source.task_id.as_deref(), Some("task_0"));
    assert_eq!(folded.tokens, child_usage.tokens);
    assert_eq!(folded.totals.tokens, child_usage.tokens);
    assert_eq!(own.sequence, 2);
    assert_eq!(own.totals.tokens.input_tokens.get(), 9);
    assert_eq!(own.totals.tokens.output_tokens.get(), 5);
    assert_eq!(own.totals.unpriced_calls, 2);

    let folded_index = position(&events, |event| matches!(event, AgentEvent::SessionUsage(_)));
    let typed_progress_index = position(
        &events,
        |event| matches!(event, AgentEvent::Tool(ToolEvent::SubAgentProgress { request, payload: seen }) if request.id == "spawn-call" && **seen == payload),
    );
    let result_index = position(
        &events,
        |event| matches!(event, AgentEvent::Tool(ToolEvent::Result { result, .. }) if result.id == "spawn-call"),
    );
    assert!(folded_index < typed_progress_index && typed_progress_index < result_index);
    assert!(
        !events.iter().any(
            |event| matches!(event, AgentEvent::Tool(ToolEvent::Progress { request, .. }) if request.id == "spawn-call")
        ),
        "sub-agent payloads must not also surface as untyped progress"
    );
}

#[tokio::test]
async fn alloyed_usage_is_attributed_to_the_member_that_served_the_call() {
    let [first, second] = two_distinct_models();
    let alloy = AlloyedModelProvider::new(vec![
        Box::new(
            FakeLlmProvider::new(vec![
                llm_response("a").usage(1, 1).text(&["a"]).build(),
                llm_response("c").usage(3, 3).text(&["c"]).build(),
            ])
            .with_model(first.clone()),
        ),
        Box::new(
            FakeLlmProvider::new(vec![llm_response("b").usage(2, 2).text(&["b"]).build()]).with_model(second.clone()),
        ),
    ]);

    let events = test_agent()
        .without_mcp()
        .scenario(
            TestScenario::new()
                .switch_model(alloy)
                .user_text("one")
                .wait_for_turn_end()
                .user_text("two")
                .wait_for_turn_end()
                .user_text("three")
                .wait_for_turn_end(),
        )
        .run()
        .await
        .unwrap();

    let served = session_usage(&events).iter().map(|usage| usage.model.model_id.clone().unwrap()).collect::<Vec<_>>();
    let expected = [&first, &second, &first].map(|model| model.model_id().into_owned());
    assert_eq!(served, expected);

    let started = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Turn(TurnEvent::LlmCallStarted { model, .. }) => model.model_id.clone(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(started, served, "usage must name the same model as the call that produced it");
}

fn two_distinct_models() -> [LlmModel; 2] {
    let models = LlmModel::all();
    let first = models[0].clone();
    let second = models
        .iter()
        .find(|model| model.model_id() != first.model_id())
        .cloned()
        .expect("catalog has two distinct models");
    [first, second]
}

fn session_usage(events: &[AgentEvent]) -> Vec<&SessionUsageEvent> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::SessionUsage(usage) => Some(usage),
            _ => None,
        })
        .collect()
}

fn position(events: &[AgentEvent], predicate: impl Fn(&AgentEvent) -> bool) -> usize {
    events.iter().position(predicate).unwrap_or_else(|| panic!("expected event not found in {events:?}"))
}
