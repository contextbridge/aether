use std::error::Error;
use std::time::Duration;

use aether_core::core::RetryConfig;
use aether_core::events::{AgentMessage, Command, UserCommand};
use aether_core::testing::test_agent;
use llm::{LlmError, LlmResponse};

fn fast_retry(max_attempts: u32) -> RetryConfig {
    RetryConfig { max_attempts, base_delay: Duration::from_millis(1), max_delay: Duration::from_millis(5) }
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
        .user_messages(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("go")] })])
        .run_with_context()
        .await?;

    let retry_events: Vec<_> = result.messages.iter().filter(|m| matches!(m, AgentMessage::Retrying { .. })).collect();
    assert_eq!(retry_events.len(), 2, "expected 2 Retrying events, got {:?}", result.messages);

    let attempts_seen: Vec<u32> = retry_events
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Retrying { attempt, .. } => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(attempts_seen, vec![1, 2], "attempt counter should increment per retry");

    assert!(
        matches!(result.messages.last(), Some(AgentMessage::Done)),
        "expected final Done, got {:?}",
        result.messages.last()
    );

    let has_error = result.messages.iter().any(|m| matches!(m, AgentMessage::Error { .. }));
    assert!(!has_error, "no terminal Error after successful retry");

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
        .user_messages(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("go")] })])
        .run_with_context()
        .await?;

    let retry_count = result.messages.iter().filter(|m| matches!(m, AgentMessage::Retrying { .. })).count();
    assert_eq!(retry_count, 3, "should retry exactly max_attempts times before giving up");

    let has_error = result.messages.iter().any(|m| matches!(m, AgentMessage::Error { .. }));
    assert!(has_error, "expected terminal Error after exhausting retries: {:?}", result.messages);

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
        .user_messages(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("go")] })])
        .run_with_context()
        .await?;

    let retry_count = result.messages.iter().filter(|m| matches!(m, AgentMessage::Retrying { .. })).count();
    assert_eq!(retry_count, 0, "non-retryable errors must not trigger retry");

    let has_error = result.messages.iter().any(|m| matches!(m, AgentMessage::Error { .. }));
    assert!(has_error, "expected terminal Error for non-retryable failure");

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
        .user_messages(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("go")] })])
        .run_with_context()
        .await?;

    let retry_count = result.messages.iter().filter(|m| matches!(m, AgentMessage::Retrying { .. })).count();
    assert_eq!(retry_count, 0, "RetryConfig::disabled() must skip all retries");

    let has_error = result.messages.iter().any(|m| matches!(m, AgentMessage::Error { .. }));
    assert!(has_error, "expected terminal Error when retry is disabled");

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
        .user_messages(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("go")] })])
        .run_with_context()
        .await?;

    let retry_count = result.messages.iter().filter(|m| matches!(m, AgentMessage::Retrying { .. })).count();
    assert_eq!(retry_count, 3, "mid-stream interrupts must respect max_attempts; got {retry_count} retries");

    let has_error = result.messages.iter().any(|m| matches!(m, AgentMessage::Error { .. }));
    assert!(has_error, "expected terminal Error after exhausting retries on mid-stream interrupts");

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
        .user_messages(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("go")] })])
        .run_with_context()
        .await?;

    let retry_count = result.messages.iter().filter(|m| matches!(m, AgentMessage::Retrying { .. })).count();
    assert_eq!(retry_count, 1);
    assert!(matches!(result.messages.last(), Some(AgentMessage::Done)));

    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancel_during_retry_wait_aborts_pending_retry() -> Result<(), Box<dyn Error>> {
    use aether_core::core::agent;
    use llm::testing::FakeLlmProvider;

    let attempts: Vec<Vec<Result<LlmResponse, LlmError>>> = vec![
        vec![Err(LlmError::ServerError { status: Some(503), message: "boom".into() })],
        vec![Ok(LlmResponse::start("msg_2")), Ok(LlmResponse::text("should not see this")), Ok(LlmResponse::done())],
    ];

    let llm = FakeLlmProvider::from_results(attempts);
    let captured = llm.captured_contexts();

    // Long retry delay; with virtual time it never elapses unless we advance.
    let retry = RetryConfig { max_attempts: 5, base_delay: Duration::from_mins(1), max_delay: Duration::from_mins(1) };

    let (tx, mut rx, _handle) = agent(llm).retry(retry).spawn().await?;

    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("go")] })).await?;

    loop {
        match rx.recv().await {
            Some(AgentMessage::Retrying { .. }) => break,
            Some(_) => {}
            None => panic!("channel closed before Retrying"),
        }
    }

    tx.send(Command::UserCommand(UserCommand::Cancel)).await?;

    let mut messages = Vec::new();
    while let Some(msg) = rx.recv().await {
        let is_done = matches!(msg, AgentMessage::Done);
        messages.push(msg);
        if is_done {
            break;
        }
    }

    let has_cancelled = messages.iter().any(|m| matches!(m, AgentMessage::Cancelled { .. }));
    assert!(has_cancelled, "expected Cancelled event, got {messages:?}");

    // The retry should never have fired — only the original failed call counts.
    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 1, "retry must not fire after cancel; expected 1 LLM call");

    Ok(())
}
