use aether_core::events::{ToolEvent, TurnEvent};
use std::error::Error;
use std::time::Duration;

use aether_core::core::RetryConfig;
use aether_core::events::{AgentEvent, LlmCallOutcome, LlmCallPurpose, TurnOutcome};
use aether_core::testing::{AddNumbersRequest, test_agent};
use llm::testing::llm_response;
use llm::{LlmError, LlmResponse, StopReason};

#[tokio::test]
async fn tool_call_turn_emits_full_trace() -> Result<(), Box<dyn Error>> {
    let tool_request = AddNumbersRequest::new(3, 5);
    let llm_responses = [
        llm_response("m1").tool_call("call_1", "test__add_numbers", &[&tool_request.json()?]).build(),
        llm_response("m2").text(&["The sum is 8"]).build(),
    ];

    let trace = test_agent().llm_responses(&llm_responses).user_text("3+5 = ?").run_trace().await?;

    let events = trace.events();
    assert!(
        matches!(events.first(), Some(AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools })) if !tools.is_empty()),
        "trace opens with the tool definitions: {events:?}"
    );
    assert!(
        matches!(events.get(1), Some(AgentEvent::Turn(TurnEvent::Started { .. }))),
        "turn starts after tool definitions: {events:?}"
    );
    assert!(matches!(events.last(), Some(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }))));

    let call_starts = trace.positions(|e| matches!(e, AgentEvent::Turn(TurnEvent::LlmCallStarted { .. })));
    let call_ends = trace.positions(|e| matches!(e, AgentEvent::Turn(TurnEvent::LlmCallEnded { .. })));
    assert_eq!(call_starts.len(), 2);
    assert_eq!(call_ends.len(), 2);
    for index in &call_starts {
        assert!(
            matches!(
                &events[*index],
                AgentEvent::Turn(TurnEvent::LlmCallStarted { purpose: LlmCallPurpose::Chat, attempt: 0, .. })
            ),
            "chat calls start with attempt 0: {:?}",
            events[*index]
        );
    }
    for index in &call_ends {
        assert!(
            matches!(
                &events[*index],
                AgentEvent::Turn(TurnEvent::LlmCallEnded {
                    purpose: LlmCallPurpose::Chat,
                    outcome: LlmCallOutcome::Completed { usage: None, .. },
                })
            ),
            "chat calls complete without usage when none is reported: {:?}",
            events[*index]
        );
    }

    let tool_call = trace.position(|e| matches!(e, AgentEvent::Tool(ToolEvent::Call { .. })));
    let tool_exec = trace.position(
        |e| matches!(e, AgentEvent::Tool(ToolEvent::ExecutionStarted { tool_id, tool_name }) if tool_id == "call_1" && tool_name == "test__add_numbers"),
    );
    let tool_result = trace.position(|e| matches!(e, AgentEvent::Tool(ToolEvent::Result { .. })));
    let turn_ended = trace.position(|e| matches!(e, AgentEvent::Turn(TurnEvent::Ended { .. })));

    assert!(call_starts[0] < tool_call);
    assert!(tool_call < tool_exec);
    assert!(tool_exec < tool_result);
    assert!(tool_result < call_starts[1]);
    assert!(call_ends[1] < turn_ended);

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn retried_call_traces_each_attempt() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> = vec![
        vec![Err(LlmError::ServerError { status: Some(503), message: "boom".into() })],
        vec![Ok(LlmResponse::start("m2")), Ok(LlmResponse::text("ok")), Ok(LlmResponse::done())],
    ];

    let trace =
        test_agent().retry_config(fast_retry(3)).llm_result_responses(&attempts).user_text("go").run_trace().await?;

    trace.assert_names(&[
        "tool_definitions",
        "turn_started",
        "call_started:Chat:0",
        "call_ended:Chat:failed_will_retry",
        "retry_scheduled:Chat:1",
        "call_started:Chat:1",
        "call_ended:Chat:completed",
        "turn_ended:completed",
    ]);

    let retry_scheduled = trace
        .events()
        .iter()
        .find(|event| matches!(event, AgentEvent::Turn(TurnEvent::RetryScheduled { attempt: 1, .. })))
        .expect("retry schedule traced");
    assert!(
        matches!(retry_scheduled, AgentEvent::Turn(TurnEvent::RetryScheduled { delay_ms, .. }) if *delay_ms > 0),
        "retries carry their backoff delay: {retry_scheduled:?}"
    );

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn exhausted_retries_fail_the_turn() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> =
        (0..3).map(|i| vec![Err(LlmError::ServerError { status: Some(503), message: format!("boom {i}") })]).collect();

    let trace =
        test_agent().retry_config(fast_retry(1)).llm_result_responses(&attempts).user_text("go").run_trace().await?;

    trace.assert_names(&[
        "tool_definitions",
        "turn_started",
        "call_started:Chat:0",
        "call_ended:Chat:failed_will_retry",
        "retry_scheduled:Chat:1",
        "call_started:Chat:1",
        "call_ended:Chat:failed_terminal",
        "turn_ended:failed",
    ]);

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancel_during_retry_wait_traces_cancelled_turn_without_starting_call() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> = vec![
        vec![Err(LlmError::ServerError { status: Some(503), message: "boom".into() })],
        vec![Ok(LlmResponse::start("m2")), Ok(LlmResponse::text("never seen")), Ok(LlmResponse::done())],
    ];
    let retry = RetryConfig { max_attempts: 5, base_delay: Duration::from_mins(1), max_delay: Duration::from_mins(1) };

    let trace = test_agent()
        .retry_config(retry)
        .llm_result_responses(&attempts)
        .cancel_when(|event| matches!(event, AgentEvent::Turn(TurnEvent::RetryScheduled { attempt: 1, .. })))
        .user_text("go")
        .run_trace()
        .await?;

    trace.assert_names(&[
        "tool_definitions",
        "turn_started",
        "call_started:Chat:0",
        "call_ended:Chat:failed_will_retry",
        "retry_scheduled:Chat:1",
        "turn_ended:cancelled",
    ]);

    Ok(())
}

#[tokio::test]
async fn usage_triggered_compaction_runs_before_the_next_chat_call() -> Result<(), Box<dyn Error>> {
    let responses = [
        llm_response("m1").text(&["hi"]).usage(90_000, 10).build_with_stop_reason(StopReason::Length),
        llm_response("summary").text(&["summary"]).usage(50, 5).build(),
        llm_response("m2").text(&["done"]).build(),
    ];

    let trace = test_agent().context_window(100_000).llm_responses(&responses).user_text("go").run_trace().await?;

    trace.assert_names(&[
        "tool_definitions",
        "turn_started",
        "call_started:Chat:0",
        "call_ended:Chat:completed",
        "call_started:Compaction:0",
        "call_ended:Compaction:completed",
        "call_started:Chat:0",
        "call_ended:Chat:completed",
        "turn_ended:completed",
    ]);

    let compaction_usage = trace.call_usage(LlmCallPurpose::Compaction).expect("compaction usage traced");
    assert_eq!(compaction_usage.input_tokens, 50);
    let chat_usage = trace.call_usage(LlmCallPurpose::Chat).expect("chat usage traced");
    assert_eq!(chat_usage.input_tokens, 90_000);
    assert_eq!(chat_usage.output_tokens, 10);

    Ok(())
}

fn fast_retry(max_attempts: u32) -> RetryConfig {
    RetryConfig { max_attempts, base_delay: Duration::from_millis(1), max_delay: Duration::from_millis(5) }
}
