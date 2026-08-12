use aether_core::events::TurnEvent;
use std::error::Error;
use std::time::Duration;

use aether_core::core::RetryConfig;
use aether_core::events::{AgentEvent, TurnOutcome};
use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, test_agent};
use llm::{LlmError, LlmResponse};
use rmcp::model::{CreateTaskResult, DetailedTask, Task, TaskPayload, TaskStatus};

fn fast_retry(max_attempts: u32) -> RetryConfig {
    RetryConfig { max_attempts, base_delay: Duration::from_millis(1), max_delay: Duration::from_millis(5) }
}

fn retry_attempts(messages: &[AgentEvent]) -> Vec<u32> {
    messages
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Turn(turn @ TurnEvent::RetryScheduled { .. }) => turn.retry_info().map(|retry| retry.attempt),
            _ => None,
        })
        .collect()
}

fn has_failed_turn(messages: &[AgentEvent]) -> bool {
    messages.iter().any(|m| matches!(m, AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } })))
}

#[tokio::test(start_paused = true)]
async fn deferred_event_after_retry_clears_pending_tool_and_cancels_task() -> Result<(), Box<dyn Error>> {
    let arguments = serde_json::json!({}).to_string();
    let mut interrupted = llm::testing::llm_response("msg_1")
        .tool_call("deferred-call", "tasks__deferred", &[&arguments])
        .build()
        .into_iter()
        .map(Ok)
        .collect::<Vec<_>>();
    interrupted.pop();
    interrupted.push(Err(LlmError::StreamInterrupted("retry after tool call".into())));
    let attempts = vec![
        interrupted,
        vec![Ok(LlmResponse::start("msg_2")), Ok(LlmResponse::text("recovered")), Ok(LlmResponse::done())],
    ];

    let now = chrono::Utc::now().to_rfc3339();
    let task = Task::new("stale-task", TaskStatus::Working, now.clone(), now).with_poll_interval_ms(10);
    let server = FakeMcpServer::new()
        .with_tool(
            FakeTool::new("deferred")
                .responds(FakeToolResponse::task(CreateTaskResult::new(task.clone())).delay(Duration::from_millis(10))),
        )
        .with_task("stale-task", [DetailedTask::new(task, TaskPayload::Working)]);
    let server_state = server.state();

    let result = test_agent()
        .fake_mcp_server("tasks", server)
        .retry_config(fast_retry(1))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    assert!(
        matches!(result.messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)),
        "stale deferred result must not block iteration completion: {:?}",
        result.messages,
    );
    assert!(
        !result.messages.iter().any(|event| matches!(event, AgentEvent::Tool(aether_core::events::ToolEvent::TaskCreated { request, .. }) if request.id == "deferred-call")),
        "stale deferred event should not be surfaced after retry: {:?}",
        result.messages,
    );
    assert_eq!(server_state.task_cancel_ids(), ["stale-task"]);

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn retries_then_succeeds_on_third_attempt() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> = vec![
        vec![Err(LlmError::StreamInterrupted("boom 1".into()))],
        vec![Err(LlmError::ServerError { status: Some(503), message: "boom 2".into() })],
        vec![Ok(LlmResponse::start("msg_3")), Ok(LlmResponse::text("ok")), Ok(LlmResponse::done())],
    ];

    let result = test_agent()
        .retry_config(fast_retry(5))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    let attempts_seen = retry_attempts(&result.messages);
    assert_eq!(attempts_seen, vec![1, 2], "attempt counter should increment per retry: {:?}", result.messages);

    assert!(
        matches!(result.messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)),
        "expected the turn to complete, got {:?}",
        result.messages.last()
    );

    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured.len(), 3, "should have called LLM 3 times (2 failures + 1 success)");

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn exhausts_retries_then_emits_error() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> =
        (0..6).map(|i| vec![Err(LlmError::ServerError { status: Some(503), message: format!("boom {i}") })]).collect();

    let result = test_agent()
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    let retry_count = retry_attempts(&result.messages).len();
    assert_eq!(retry_count, 3, "should retry exactly max_attempts times before giving up");

    assert!(
        has_failed_turn(&result.messages),
        "expected a failed turn after exhausting retries: {:?}",
        result.messages
    );

    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured.len(), 4, "should call LLM max_attempts + 1 times (1 initial + 3 retries)");

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn non_retryable_error_surfaces_immediately() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> =
        vec![vec![Err(LlmError::ApiError("HTTP 400 bad request".into()))]];

    let result = test_agent()
        .retry_config(fast_retry(5))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    let retry_count = retry_attempts(&result.messages).len();
    assert_eq!(retry_count, 0, "non-retryable errors must not trigger retry");

    assert!(has_failed_turn(&result.messages), "expected a failed turn for non-retryable failure");

    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured.len(), 1, "should call LLM exactly once");

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn retry_disabled_surfaces_retryable_error_immediately() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> =
        vec![vec![Err(LlmError::ServerError { status: Some(503), message: "would be retryable".into() })]];

    let result = test_agent()
        .retry_config(RetryConfig::disabled())
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    let retry_count = retry_attempts(&result.messages).len();
    assert_eq!(retry_count, 0, "RetryConfig::disabled() must skip all retries");

    assert!(has_failed_turn(&result.messages), "expected a failed turn when retry is disabled");

    Ok(())
}

/// Regression test for a bug where `IterationState::on_llm_start` reset the
/// retry counter on every successful `Start` frame. That made any failure
/// occurring *after* the first byte of a stream (the case `StreamInterrupted`
/// was added for) effectively unbounded — each retry's `Start` zeroed the
/// counter, so the budget never accumulated.
///
/// With the fix, mid-stream interrupts must consume the same retry budget as
/// pre-`Start` failures.
#[tokio::test(start_paused = true)]
async fn mid_stream_interrupts_consume_retry_budget() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> = (0..6)
        .map(|i| {
            let id = format!("m{i}");
            vec![
                Ok(LlmResponse::start(&id)),
                Ok(LlmResponse::text("partial")),
                Err(LlmError::StreamInterrupted(format!("boom {i}"))),
            ]
        })
        .collect();

    let result = test_agent()
        .retry_config(fast_retry(3))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    let retry_count = retry_attempts(&result.messages).len();
    assert_eq!(retry_count, 3, "mid-stream interrupts must respect max_attempts; got {retry_count} retries");

    assert!(
        has_failed_turn(&result.messages),
        "expected a failed turn after exhausting retries on mid-stream interrupts"
    );

    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(
        captured.len(),
        4,
        "should call LLM exactly max_attempts + 1 times (1 initial + 3 retries), got {}",
        captured.len()
    );

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn rate_limited_error_is_retried() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> = vec![
        vec![Err(LlmError::RateLimited("slow down".into()))],
        vec![Ok(LlmResponse::start("msg_2")), Ok(LlmResponse::text("ok")), Ok(LlmResponse::done())],
    ];

    let result = test_agent()
        .retry_config(fast_retry(5))
        .llm_result_responses(&attempts)
        .user_text("go")
        .run_with_context()
        .await?;

    let retry_count = retry_attempts(&result.messages).len();
    assert_eq!(retry_count, 1);
    assert!(matches!(result.messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)));

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancel_during_retry_wait_aborts_pending_retry() -> Result<(), Box<dyn Error>> {
    use aether_core::testing::TestScenario;

    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> = vec![
        vec![Err(LlmError::ServerError { status: Some(503), message: "boom".into() })],
        vec![Ok(LlmResponse::start("msg_2")), Ok(LlmResponse::text("should not see this")), Ok(LlmResponse::done())],
    ];

    // Long retry delay; with virtual time it never elapses unless we advance.
    let retry = RetryConfig { max_attempts: 5, base_delay: Duration::from_mins(1), max_delay: Duration::from_mins(1) };

    let result = test_agent()
        .retry_config(retry)
        .llm_result_responses(&attempts)
        .scenario(TestScenario::new().user_text("go").wait_for_retry(1).cancel().wait_for_turn_end())
        .run_with_context()
        .await?;

    let messages = &result.messages;

    let retry_started = messages
        .iter()
        .any(|message| matches!(message, AgentEvent::Turn(TurnEvent::LlmCallStarted { attempt: 1, .. })));
    assert!(!retry_started, "cancelled backoff must not emit a call start: {messages:?}");

    let has_cancelled = messages.iter().any(|m| matches!(m.turn_outcome(), Some(TurnOutcome::Cancelled)));
    assert!(has_cancelled, "expected the turn to end as cancelled, got {messages:?}");

    // The retry should never have fired — only the original failed call counts.
    let captured = result.captured_contexts.lock().unwrap();
    assert_eq!(captured.len(), 1, "retry must not fire after cancel; expected 1 LLM call");

    Ok(())
}
