use aether_core::context::CompactionConfig;
use aether_core::core::{AgentBuilder, Prompt, agent};
use aether_core::events::{
    AgentEvent, Command, CompactionOutcome, ContextEvent, LlmCallOutcome, MessageEvent, TurnEvent, TurnOutcome,
    UserCommand,
};
use aether_core::testing::{AgentTrace, FakeAgentObserver, TestResult, TestScenario, drain_until, test_agent};
use llm::testing::llm_response;
use llm::{ChatMessage, Context, LlmResponseStream, ProviderError, StreamingModelProvider};

#[tokio::test]
async fn test_api_error_mid_stream_does_not_add_empty_assistant_message() -> TestResult<()> {
    // First call: Start → Err → Done (simulates HTTP 522 mid-stream)
    let error_response = llm_response().build_with_error(ProviderError::api("HTTP 522: connection timed out"));

    // Second call: normal success (triggered by second user message)
    let success_response = llm_response().text(&["Hello!"]).build_results();

    // Only send the first user message to avoid race conditions.
    // After the error + Done cycle, we manually inspect captured contexts.
    let result = test_agent()
        .llm_result_responses(&[error_response, success_response])
        .user_text("first message")
        .run_with_context()
        .await?;

    // The agent should NOT emit a Text{is_complete: true} with empty content.
    // That would mean an empty assistant message was added to context.
    let has_empty_complete_text = result.messages.iter().any(|m| {
        matches!(
            m,
            AgentEvent::Message(MessageEvent::Text {
                chunk,
                is_complete: true,
                ..
            }) if chunk.is_empty()
        )
    });

    assert!(
        !has_empty_complete_text,
        "Agent must not emit a completed Text message with empty content after an API error. Messages: {:?}",
        result.messages
    );

    // Should end with Done
    assert!(
        matches!(result.messages.last(), Some(AgentEvent::Turn(TurnEvent::Ended { .. }))),
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

#[tokio::test]
async fn observer_panic_does_not_abort_the_turn() -> TestResult<()> {
    let observer =
        FakeAgentObserver::new().with_event_panic(|event| matches!(event, AgentEvent::Turn(TurnEvent::Started { .. })));
    let events = observer.events();

    assert_observer_panic_isolated(observer).await?;
    assert!(matches!(events.lock().unwrap().last(), Some(AgentEvent::Turn(TurnEvent::Started { .. }))));
    Ok(())
}

#[tokio::test]
async fn observer_panic_on_turn_end_still_delivers_turn_end() -> TestResult<()> {
    let observer =
        FakeAgentObserver::new().with_event_panic(|event| matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. })));
    let events = observer.events();

    assert_observer_panic_isolated(observer).await?;

    let trace = AgentTrace::from_observer_events(&events);
    assert_eq!(trace.positions(|event| event.turn_outcome().is_some()).len(), 1);
    assert!(matches!(trace.events().last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)));
    Ok(())
}

#[tokio::test]
async fn observer_panic_while_idle_keeps_agent_running() -> TestResult<()> {
    let observer =
        FakeAgentObserver::new().with_event_panic(|event| matches!(event, AgentEvent::Context(ContextEvent::Cleared)));
    let events = observer.events();

    assert_observer_panic_isolated(observer).await?;
    assert!(matches!(events.lock().unwrap().last(), Some(AgentEvent::Context(ContextEvent::Cleared))));
    Ok(())
}

#[tokio::test]
async fn system_prompt_observer_panic_does_not_abort_the_turn() -> TestResult<()> {
    let observer = FakeAgentObserver::new().with_system_prompt_panic();
    let prompts = observer.system_prompts();

    assert_observer_panic_isolated(observer).await?;
    assert_eq!(*prompts.lock().unwrap(), vec!["You are a test agent."]);
    Ok(())
}

#[tokio::test]
async fn tool_trace_observer_panic_does_not_abort_the_turn() -> TestResult<()> {
    assert_observer_panic_isolated(FakeAgentObserver::new().with_tool_trace_context_panic()).await
}

#[tokio::test]
async fn agent_panic_ends_turn_as_failed_with_panic_message() -> TestResult<()> {
    let trace = run_panicking_agent(agent(PanickingLlm)).await?;

    trace.assert_names(&[
        "turn_started",
        "call_started:Chat:0",
        "call_ended:Chat:failed_terminal",
        "turn_ended:failed",
    ]);
    assert!(matches!(
        &trace.events()[trace.position(|event| matches!(event, AgentEvent::Turn(TurnEvent::LlmCallEnded { .. })))],
        AgentEvent::Turn(TurnEvent::LlmCallEnded {
            outcome: LlmCallOutcome::Failed { error, will_retry: false, .. }, ..
        }) if error.contains("llm exploded")
    ));
    Ok(())
}

#[tokio::test]
async fn compaction_panic_closes_call_and_compaction_before_failing_the_turn() -> TestResult<()> {
    let trace = run_panicking_agent(
        agent(PanickingLlm)
            .context_window(Some(100))
            .compaction(CompactionConfig::with_threshold(0.85))
            .messages(vec![ChatMessage::user("x".repeat(400))]),
    )
    .await?;

    trace.assert_names(&[
        "turn_started",
        "compaction_started",
        "call_started:Compaction:0",
        "call_ended:Compaction:failed_terminal",
        "compaction_ended:failed",
        "turn_ended:failed",
    ]);
    let lifecycle: Vec<_> = trace
        .events()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Context(context) => Some(context),
            _ => None,
        })
        .collect();
    assert!(
        matches!(
            lifecycle.as_slice(),
            [ContextEvent::CompactionStarted { compaction_id: started, .. },
             ContextEvent::CompactionEnded { compaction_id: ended, outcome: CompactionOutcome::Failed { error } }]
            if started == ended && error.contains("llm exploded")
        ),
        "got: {lifecycle:?}"
    );
    Ok(())
}

struct PanickingLlm;

impl StreamingModelProvider for PanickingLlm {
    fn stream_response(&self, _context: &Context) -> LlmResponseStream {
        panic!("llm exploded");
    }

    fn display_name(&self) -> String {
        "panicking".to_string()
    }

    fn context_window(&self) -> Option<u32> {
        None
    }
}

async fn assert_observer_panic_isolated(observer: FakeAgentObserver) -> TestResult<()> {
    let observer_events = observer.events();
    let trace = test_agent()
        .system_prompt(Prompt::text("You are a test agent."))
        .llm_responses(&[
            llm_response().tool_call("call_1", "test__add_numbers", &[r#"{"a":1,"b":2}"#]).build(),
            llm_response().text(&["Hello!"]).build(),
            llm_response().text(&["Again!"]).build(),
        ])
        .observer(Box::new(observer))
        .scenario(
            TestScenario::new()
                .send(Command::UserCommand(UserCommand::ClearContext))
                .user_text("hi")
                .wait_for_turn_end()
                .user_text("again")
                .wait_for_turn_end(),
        )
        .run_trace()
        .await?;

    let outcomes: Vec<_> = trace.events().iter().filter_map(AgentEvent::turn_outcome).collect();
    assert_eq!(outcomes, vec![&TurnOutcome::Completed, &TurnOutcome::Completed]);
    for text in ["Hello!", "Again!"] {
        assert!(trace.events().iter().any(|event| event.content().as_deref() == Some(text)));
    }
    let observer_events = observer_events.lock().unwrap();
    assert!(observer_events.len() < trace.events().len(), "Expected panicking observer to be removed");
    assert!(trace.events().starts_with(&observer_events));
    Ok(())
}

async fn run_panicking_agent(builder: AgentBuilder) -> TestResult<AgentTrace> {
    let observer = FakeAgentObserver::new();
    let events = observer.events();
    let (tx, mut rx, handle) = builder.observer(Box::new(observer)).spawn().await?;
    tx.send(Command::text("hi")).await?;
    let messages = drain_until(&mut rx, |event| event.turn_outcome().is_some()).await;
    drop(tx);
    handle.await_completion().await;

    let trace = AgentTrace::from_observer_events(&events);
    assert_eq!(trace.events(), messages);
    assert!(matches!(
        trace.events().last().and_then(AgentEvent::turn_outcome),
        Some(TurnOutcome::Failed { error, .. }) if error.contains("llm exploded")
    ));
    Ok(trace)
}
