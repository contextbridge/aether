use aether_core::events::TurnEvent;
use mcp_utils::testing::{FakeTask, Gate};
use std::error::Error;
use std::time::Duration;

use aether_core::core::RetryConfig;
use aether_core::events::{AgentEvent, LlmCallOutcome, TurnOutcome};
use aether_core::testing::{TestAgentResult, TestResult, TestScenario, fast_retry, test_agent};
use llm::testing::{failed_call, llm_response};
use llm::{LlmError, LlmResponse, ProviderError};

#[tokio::test(start_paused = true)]
async fn deferred_event_after_retry_clears_pending_tool_and_cancels_task() -> Result<(), Box<dyn Error>> {
    let arguments = serde_json::json!({}).to_string();
    let attempts = vec![
        llm_response()
            .tool_call("deferred-call", "tasks__deferred", &[&arguments])
            .build_interrupted(ProviderError::stream_interrupted("retry after tool call")),
        llm_response().text(&["recovered"]).build_results(),
    ];

    let created = Gate::new();
    let server = FakeTask::new("stale-task").gated(created.clone()).server("deferred");
    let server_state = server.state();

    let result = test_agent()
        .mcp_server("tasks", server)
        .retry_config(fast_retry(1))
        .llm_result_responses(&attempts)
        .scenario(
            TestScenario::new().user_text("go").wait_for_retry(1).perform(move || created.open()).wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    assert!(completed(&result), "stale deferred result must not block iteration completion: {:?}", result.messages);
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
    let attempts = vec![
        failed_call(
            ProviderError::server("boom 1")
                .with_http_status(200)
                .with_code(Some("server_error".to_string()))
                .with_request_id(Some("req-1".to_string())),
        ),
        failed_call(ProviderError::server("boom 2").with_http_status(503)),
        llm_response().text(&["ok"]).build_results(),
    ];

    let result = run_with_retries(fast_retry(5), &attempts).await?;

    assert_eq!(
        retry_attempts(&result),
        vec![1, 2],
        "attempt counter should increment per retry: {:?}",
        result.messages
    );
    assert!(completed(&result), "expected the turn to complete, got {:?}", result.messages.last());
    assert_eq!(result.contexts().len(), 3, "should have called LLM 3 times (2 failures + 1 success)");

    let first_failed = failed_calls(&result)[0];
    assert!(
        matches!(
            first_failed,
            LlmCallOutcome::Failed {
                will_retry: true,
                http_status: Some(200),
                provider_request_id: Some(request_id),
                provider_error_code: Some(code),
                ..
            } if request_id == "req-1" && code == "server_error"
        ),
        "first failure should schedule a retry and carry provider details: {first_failed:?}"
    );

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn exhausts_retries_then_emits_error() -> Result<(), Box<dyn Error>> {
    let attempts: Vec<_> =
        (0..6).map(|i| failed_call(ProviderError::server(format!("boom {i}")).with_http_status(503))).collect();

    let result = run_with_retries(fast_retry(3), &attempts).await?;

    assert_eq!(retry_attempts(&result).len(), 3, "should retry exactly max_attempts times before giving up");
    assert!(has_failed_turn(&result), "expected a failed turn after exhausting retries: {:?}", result.messages);
    assert_eq!(result.contexts().len(), 4, "should call LLM max_attempts + 1 times (1 initial + 3 retries)");

    let last_failed = failed_calls(&result).pop().expect("expected failed llm_call_ended events");
    assert!(
        matches!(last_failed, LlmCallOutcome::Failed { will_retry: false, http_status: Some(503), .. }),
        "exhausted budget must report will_retry=false: {last_failed:?}"
    );

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn non_retryable_error_surfaces_immediately() -> Result<(), Box<dyn Error>> {
    let attempts = vec![failed_call(ProviderError::api("HTTP 400 bad request"))];

    let result = run_with_retries(fast_retry(5), &attempts).await?;

    assert_eq!(retry_attempts(&result).len(), 0, "non-retryable errors must not trigger retry");
    assert!(has_failed_turn(&result), "expected a failed turn for non-retryable failure");
    assert_eq!(result.contexts().len(), 1, "should call LLM exactly once");

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn retry_disabled_surfaces_retryable_error_immediately() -> Result<(), Box<dyn Error>> {
    let attempts = vec![failed_call(ProviderError::server("would be retryable").with_http_status(503))];

    let result = run_with_retries(RetryConfig::disabled(), &attempts).await?;

    assert_eq!(retry_attempts(&result).len(), 0, "RetryConfig::disabled() must skip all retries");
    assert!(has_failed_turn(&result), "expected a failed turn when retry is disabled");

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
    let attempts: Vec<_> = (0..6)
        .map(|i| {
            llm_response().text(&["partial"]).build_interrupted(ProviderError::stream_interrupted(format!("boom {i}")))
        })
        .collect();

    let result = run_with_retries(fast_retry(3), &attempts).await?;

    assert_eq!(retry_attempts(&result).len(), 3, "mid-stream interrupts must respect max_attempts");
    assert!(has_failed_turn(&result), "expected a failed turn after exhausting retries on mid-stream interrupts");
    assert_eq!(result.contexts().len(), 4, "should call LLM exactly max_attempts + 1 times (1 initial + 3 retries)");

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn rate_limited_error_is_retried() -> Result<(), Box<dyn Error>> {
    let attempts =
        vec![failed_call(ProviderError::rate_limit("slow down")), llm_response().text(&["ok"]).build_results()];

    let result = run_with_retries(fast_retry(5), &attempts).await?;

    assert_eq!(retry_attempts(&result).len(), 1);
    assert!(completed(&result));

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancel_during_retry_wait_aborts_pending_retry() -> Result<(), Box<dyn Error>> {
    let attempts = vec![
        failed_call(ProviderError::server("boom").with_http_status(503)),
        llm_response().text(&["should not see this"]).build_results(),
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
    assert_eq!(result.contexts().len(), 1, "retry must not fire after cancel; expected 1 LLM call");

    Ok(())
}

async fn run_with_retries(
    retry: RetryConfig,
    attempts: &[Vec<Result<LlmResponse, LlmError>>],
) -> TestResult<TestAgentResult> {
    test_agent().retry_config(retry).llm_result_responses(attempts).user_text("go").run_with_context().await
}

fn retry_attempts(result: &TestAgentResult) -> Vec<u32> {
    result
        .messages
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Turn(turn @ TurnEvent::RetryScheduled { .. }) => turn.retry_info().map(|retry| retry.attempt),
            _ => None,
        })
        .collect()
}

fn failed_calls(result: &TestAgentResult) -> Vec<&LlmCallOutcome> {
    result
        .messages
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Turn(TurnEvent::LlmCallEnded { outcome: outcome @ LlmCallOutcome::Failed { .. }, .. }) => {
                Some(outcome)
            }
            _ => None,
        })
        .collect()
}

fn completed(result: &TestAgentResult) -> bool {
    matches!(result.messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed))
}

fn has_failed_turn(result: &TestAgentResult) -> bool {
    result.messages.iter().any(|m| matches!(m.turn_outcome(), Some(TurnOutcome::Failed { .. })))
}
