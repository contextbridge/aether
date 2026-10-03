use crate::TestResult;
use aether_cli::acp::testing::{AcpTestHarness, FakeBackgroundTask};
use aether_core::core::agent;
use aether_sessions::{SessionEvent, UserEvent};
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, CloseSessionRequest, ContentBlock, MessageId, PromptRequest, PromptResponse, SessionId,
    SessionUpdate, StateUpdate, StopReason,
};
use agent_client_protocol::{ErrorCode, SentRequest};
use llm::LlmResponse;
use llm::testing::{FakeLlmProvider, llm_response};
use std::sync::Arc;
use tokio::{sync::Notify, task::LocalSet};

#[tokio::test(flavor = "current_thread")]
async fn cancel_during_mcp_prompt_expansion_does_not_wait_for_the_server() {
    AcpTestHarness::run(|mut harness| async move {
        let (started, _release) = harness.pause_prompt_expansion();
        let session = harness.insert_agent_switching_session().await;
        let id = session.session_id().clone();
        let prompt = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["/plan".into()])).block_task();
        started.notified().await;
        harness.client_cx.send_notification(CancelSessionNotification::new(id.clone())).unwrap();
        let error = prompt.await.expect_err("a prompt cancelled before insertion errors");
        assert_eq!(error.code, ErrorCode::RequestCancelled);
        session.planner().assert_never_ran();
        harness.shutdown().await;
        assert_eq!(harness.live_runtime_count(), 0);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_during_prompt_expansion_reports_no_turn() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let (started, _release) = harness.pause_prompt_expansion();
        let session = harness.insert_agent_switching_session().await;
        let id = session.session_id().clone();
        let prompt = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["/plan".into()])).block_task();
        started.notified().await;
        harness.client_cx.send_notification(CancelSessionNotification::new(id.clone()))?;
        prompt.await.expect_err("a prompt cancelled before insertion errors");

        harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["hello".into()])).block_task().await?;
        let updates = harness.updates_until(&id, is_running).await;
        assert!(!updates.iter().any(is_idle), "no idle is reported for a turn that never ran: {updates:?}");
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_sent_before_acceptance_is_not_lost() {
    LocalSet::new()
        .run_until(async {
            let provider =
                FakeLlmProvider::new(vec![vec![LlmResponse::Start, LlmResponse::text("reply"), LlmResponse::done()]])
                    .pause_turn_after(0, 1, Arc::new(Notify::new()));
            let (tx, rx, handle) = agent(provider).spawn().await.unwrap();
            let mut harness = AcpTestHarness::start().await;
            let id = SessionId::new("early-cancel");
            harness.insert_stub_session(tx, rx, handle, id.clone(), "fake:fake").await;
            let prompt = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["hi".into()])).block_task();
            harness.client_cx.send_notification(CancelSessionNotification::new(id.clone())).unwrap();
            match prompt.await {
                Ok(_) => harness.expect_idle(&id, StopReason::Cancelled).await,
                Err(error) => assert_eq!(error.code, ErrorCode::RequestCancelled),
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn provider_failure_after_acceptance_reports_error_and_idle() {
    LocalSet::new().run_until(async {
        let (tx, rx, handle) = agent(FakeLlmProvider::new(vec![vec![LlmResponse::Error { message: "provider failed".into() }]]))
            .spawn().await.unwrap();
        let mut harness = AcpTestHarness::start().await;
        let id = SessionId::new("failure");
        harness.insert_stub_session(tx, rx, handle, id.clone(), "fake:fake").await;
        harness.client_cx.send_request(PromptRequest::new(id, vec!["hi".into()])).block_task().await.unwrap();
        let mut error_seen = false;
        loop {
            match harness.peer.next_session_notification().await.update {
                SessionUpdate::AgentMessageChunk(chunk) => {
                    if let ContentBlock::Text(text) = chunk.content {
                        error_seen |= text.text.contains("provider failed");
                    }
                },
                SessionUpdate::AgentMessage(message) => {
                    if let Some(content) = message.content.value() {
                        error_seen |= content.iter().any(|block| matches!(block, ContentBlock::Text(text) if text.text.contains("provider failed")));
                    }
                },
                SessionUpdate::StateUpdate(StateUpdate::Idle(idle)) => {
                    assert!(error_seen, "error must precede idle");
                    assert_eq!(idle.stop_reason, Some(StopReason::EndTurn));
                    break;
                },
                _ => {},
            }
        }
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn failed_turn_is_reported_once_under_a_stable_id_and_kept_out_of_resumed_history() {
    LocalSet::new()
        .run_until(async {
            let provider = FakeLlmProvider::new(vec![vec![LlmResponse::Error { message: "provider failed".into() }]]);
            let (tx, rx, handle) = agent(provider).spawn().await.unwrap();
            let mut harness = AcpTestHarness::start().await;
            let id = SessionId::new("failed-turn");
            harness.append_stored_session(&id.0, "2026-05-01T00:00:00Z");
            harness.insert_stub_session(tx, rx, handle, id.clone(), "fake:fake").await;

            harness
                .client_cx
                .send_request(PromptRequest::new(id.clone(), vec!["hi".into()]))
                .block_task()
                .await
                .unwrap();
            let live = error_message_ids(&mut harness, &id).await;
            assert_eq!(live.len(), 1, "live turn reports the failure once");

            harness.client_cx.send_request(CloseSessionRequest::new(id.clone())).block_task().await.unwrap();
            harness.resume_with_replay(&id).await;
            harness
                .client_cx
                .send_request(PromptRequest::new(id.clone(), vec!["again".into()]))
                .block_task()
                .await
                .unwrap();
            let replayed = error_message_ids(&mut harness, &id).await;
            assert_eq!(replayed, live, "replay reports the failure once, under its live id");

            harness.resume_agent().assert_saw_exactly(&["hi", "again"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_session_is_rejected_before_acceptance() {
    AcpTestHarness::run(|harness| async move {
        assert!(
            harness
                .client_cx
                .send_request(PromptRequest::new("missing", vec!["hi".into()]))
                .block_task()
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn acceptance_precedes_streaming_and_idle_completes_the_turn() {
    LocalSet::new()
        .run_until(async {
            let release = Arc::new(Notify::new());
            let provider =
                FakeLlmProvider::new(vec![vec![LlmResponse::Start, LlmResponse::text("hello"), LlmResponse::done()]])
                    .pause_turn_after(0, 1, release.clone());
            let (tx, rx, handle) = agent(provider).spawn().await.unwrap();
            let mut harness = AcpTestHarness::start().await;
            let id = SessionId::new("lifecycle");
            harness.insert_stub_session(tx, rx, handle, id.clone(), "fake:fake").await;
            harness.client_cx.send_notification(CancelSessionNotification::new(id.clone())).unwrap();
            let response = harness
                .client_cx
                .send_request(PromptRequest::new(id.clone(), vec!["hi".into()]))
                .block_task()
                .await
                .unwrap();
            assert_eq!(serde_json::to_value(&response).unwrap(), serde_json::json!({"messageId": response.message_id}));
            let mut lifecycle = Vec::new();
            while lifecycle.len() < 2 {
                match harness.peer.next_session_notification().await.update {
                    update if is_running(&update) => lifecycle.push("running"),
                    SessionUpdate::UserMessage(message) => {
                        assert_eq!(message.message_id, response.message_id);
                        lifecycle.push("user_message");
                    }
                    SessionUpdate::AvailableCommandsUpdate(_) | SessionUpdate::ConfigOptionUpdate(_) => {}
                    update => panic!("unexpected update before user acknowledgement: {update:?}"),
                }
            }
            assert_eq!(lifecycle, ["running", "user_message"]);
            release.notify_one();
            let mut streamed_message = None;
            loop {
                match harness.peer.next_session_notification().await.update {
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        assert!(matches!(chunk.content, ContentBlock::Text(text) if text.text == "hello"));
                        streamed_message = Some(chunk.message_id);
                    }
                    SessionUpdate::AgentMessage(message) => assert_eq!(Some(message.message_id), streamed_message),
                    SessionUpdate::StateUpdate(StateUpdate::Idle(idle)) => {
                        assert!(streamed_message.is_some(), "streaming must precede idle");
                        assert_eq!(idle.stop_reason, Some(StopReason::EndTurn));
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn background_task_outcome_turn_is_framed_by_running_and_idle() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let task = FakeBackgroundTask::new();
        let id = SessionId::new("task-outcome");
        harness.insert_background_task_session(background_task_provider(), &task, id.clone()).await;
        start_background_task(&mut harness, &id).await?;

        task.complete("task output");
        let updates = harness.updates_until(&id, |update| matches!(update, SessionUpdate::AgentMessage(_))).await;
        assert!(updates.iter().any(is_running), "task outcome turn must report running before replying: {updates:?}");
        harness.expect_idle(&id, StopReason::EndTurn).await;
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_during_background_task_outcome_turn_is_inserted_after_its_reply() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let task = FakeBackgroundTask::new();
        let release = Arc::new(Notify::new());
        let provider = background_task_provider().pause_turn_after(2, 1, release.clone());
        let id = SessionId::new("task-outcome-prompt");
        harness.insert_background_task_session(provider, &task, id.clone()).await;
        start_background_task(&mut harness, &id).await?;

        task.complete("task output");
        let updates = harness.updates_until(&id, |update| matches!(update, SessionUpdate::AgentMessageChunk(_))).await;
        assert!(updates.iter().any(is_running), "task outcome turn must report running before replying: {updates:?}");
        let mut delivered = harness.watch_agent_prompts();
        let prompt = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["second".into()])).block_task();
        delivered.changed().await?;
        release.notify_one();
        let prompt = prompt.await?;

        let updates = harness.updates_until(&id, |update| is_agent_message(update, "prompt handled")).await;
        assert_inserted_within_turn_after(&updates, "task handled", &prompt.message_id);
        harness.expect_idle(&id, StopReason::EndTurn).await;
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_prepared_during_background_task_outcome_turn_is_inserted_after_its_reply() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let (expansion_started, release_expansion) = harness.pause_prompt_expansion();
        let task = FakeBackgroundTask::new();
        let release = Arc::new(Notify::new());
        let provider = background_task_provider().pause_turn_after(2, 1, release.clone());
        let id = SessionId::new("task-outcome-preparing");
        harness.insert_background_task_session(provider, &task, id.clone()).await;
        start_background_task(&mut harness, &id).await?;
        let prompt = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["/plan".into()])).block_task();
        expansion_started.notified().await;

        task.complete("task output");
        let updates = harness.updates_until(&id, |update| matches!(update, SessionUpdate::AgentMessageChunk(_))).await;
        assert!(updates.iter().any(is_running), "task outcome turn must report running before replying: {updates:?}");
        let mut delivered = harness.watch_agent_prompts();
        release_expansion.notify_one();
        delivered.changed().await?;
        release.notify_one();
        let prompt = prompt.await?;

        let updates = harness.updates_until(&id, |update| is_agent_message(update, "prompt handled")).await;
        assert_inserted_within_turn_after(&updates, "task handled", &prompt.message_id);
        harness.expect_idle(&id, StopReason::EndTurn).await;
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_sent_while_running_is_inserted_after_the_current_reply() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let id = SessionId::new("queued-prompt");
        let (second, updates) = queue_prompt_behind_running_reply(&mut harness, &id).await?;

        assert_inserted_within_turn_after(&updates, "first reply", &second.message_id);
        harness.expect_idle(&id, StopReason::EndTurn).await;
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn queued_prompt_is_resumed_where_it_was_inserted() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let id = SessionId::new("queued-prompt-resume");
        harness.append_stored_session(&id.0, "2026-05-01T00:00:00Z");
        queue_prompt_behind_running_reply(&mut harness, &id).await?;
        harness.expect_idle(&id, StopReason::EndTurn).await;

        harness.client_cx.send_request(CloseSessionRequest::new(id.clone())).block_task().await?;
        harness.resume(&id).await;
        harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["third".into()])).block_task().await?;
        harness.expect_idle(&id, StopReason::EndTurn).await;

        harness.resume_agent().assert_saw_exactly(&["first", "first reply", "second", "second reply", "third"]);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn prompts_sent_together_are_inserted_in_submission_order() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let (expansion_started, release_expansion) = harness.pause_prompt_expansion();
        let session = harness.insert_agent_switching_session().await;
        let id = session.session_id().clone();
        let slow = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["/plan".into()])).block_task();
        expansion_started.notified().await;
        let fast =
            harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["right after".into()])).block_task();
        release_expansion.notify_one();

        let slow = slow.await?;
        let fast = fast.await?;
        let updates = harness.updates_until(&id, |update| is_user_message(update, &fast.message_id)).await;
        assert!(
            updates.iter().any(|update| is_user_message(update, &slow.message_id)),
            "the slow prompt is inserted before the fast one: {updates:?}"
        );
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_rejects_a_prompt_queued_behind_the_running_reply() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let id = SessionId::new("cancelled-queue");
        harness.append_stored_session(&id.0, "2026-05-01T00:00:00Z");
        let (_release, second) = queue_prompt_behind_paused_reply(&mut harness, &id, &[]).await?;
        harness.client_cx.send_notification(CancelSessionNotification::new(id.clone()))?;

        let error = second.block_task().await.expect_err("a discarded prompt is rejected");
        assert_eq!(error.code, ErrorCode::RequestCancelled);
        let updates = harness.updates_until(&id, is_idle).await;
        assert!(
            !updates.iter().any(|update| matches!(update, SessionUpdate::UserMessage(_))),
            "a discarded prompt is never shown as inserted: {updates:?}"
        );
        assert!(
            matches!(updates.last(), Some(SessionUpdate::StateUpdate(StateUpdate::Idle(idle))) if idle.stop_reason == Some(StopReason::Cancelled))
        );
        assert_eq!(stored_prompts(&harness, &id), ["first"]);
        Ok(())
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_sent_after_cancel_starts_a_new_turn() -> TestResult {
    AcpTestHarness::run(|mut harness| async move {
        let id = SessionId::new("prompt-after-cancel");
        let (_release, second) = queue_prompt_behind_paused_reply(&mut harness, &id, &["third reply"]).await?;
        harness.client_cx.send_notification(CancelSessionNotification::new(id.clone()))?;
        let third = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["third".into()]));

        let error = second.block_task().await.expect_err("a discarded prompt is rejected");
        assert_eq!(error.code, ErrorCode::RequestCancelled);
        let third = third.block_task().await?;
        let updates = harness.updates_until(&id, |update| is_agent_message(update, "third reply")).await;
        assert!(updates.iter().any(|update| is_user_message(update, &third.message_id)), "{updates:?}");
        harness.expect_idle(&id, StopReason::EndTurn).await;
        Ok(())
    })
    .await
}

fn background_task_provider() -> FakeLlmProvider {
    FakeLlmProvider::new(vec![
        llm_response().tool_call("start-call", FakeBackgroundTask::TOOL, &["{}"]).build(),
        llm_response().text(&["started"]).build(),
        llm_response().text(&["task handled"]).build(),
        llm_response().text(&["prompt handled"]).build(),
    ])
}

async fn queue_prompt_behind_running_reply(
    harness: &mut AcpTestHarness,
    id: &SessionId,
) -> TestResult<(PromptResponse, Vec<SessionUpdate>)> {
    let (release, second) = queue_prompt_behind_paused_reply(harness, id, &["second reply"]).await?;
    release.notify_one();
    let second = second.block_task().await?;
    let updates = harness.updates_until(id, |update| is_agent_message(update, "second reply")).await;
    Ok((second, updates))
}

async fn queue_prompt_behind_paused_reply(
    harness: &mut AcpTestHarness,
    id: &SessionId,
    later_replies: &[&str],
) -> TestResult<(Arc<Notify>, SentRequest<PromptResponse>)> {
    let release = Arc::new(Notify::new());
    let mut turns = vec![llm_response().text(&["first", " reply"]).build()];
    turns.extend(later_replies.iter().map(|reply| llm_response().text(&[reply]).build()));
    let provider = FakeLlmProvider::new(turns).pause_turn_after(0, 1, release.clone());
    let (tx, rx, handle) = agent(provider).spawn().await?;
    harness.insert_stub_session(tx, rx, handle, id.clone(), "fake:fake").await;
    harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["first".into()])).block_task().await?;
    harness.updates_until(id, |update| matches!(update, SessionUpdate::AgentMessageChunk(_))).await;

    let mut delivered = harness.watch_agent_prompts();
    let second = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["second".into()]));
    delivered.changed().await?;
    Ok((release, second))
}

fn assert_inserted_within_turn_after(updates: &[SessionUpdate], reply: &str, prompt: &MessageId) {
    assert!(
        position(updates, |update| is_agent_message(update, reply)) < position(updates, |u| is_user_message(u, prompt)),
        "the prompt is inserted after the reply it queued behind: {updates:?}"
    );
    assert!(
        !updates.iter().any(|update| matches!(update, SessionUpdate::StateUpdate(_))),
        "the prompt joins the turn already running: {updates:?}"
    );
}

fn position(updates: &[SessionUpdate], predicate: impl Fn(&SessionUpdate) -> bool) -> usize {
    updates.iter().position(predicate).unwrap_or_else(|| panic!("expected a matching update in {updates:?}"))
}

fn is_agent_message(update: &SessionUpdate, text: &str) -> bool {
    matches!(update, SessionUpdate::AgentMessage(message) if message.content.value().into_iter().flatten().any(
        |block| matches!(block, ContentBlock::Text(block) if block.text == text),
    ))
}

fn is_user_message(update: &SessionUpdate, id: &MessageId) -> bool {
    matches!(update, SessionUpdate::UserMessage(message) if message.message_id == *id)
}

async fn start_background_task(harness: &mut AcpTestHarness, id: &SessionId) -> TestResult {
    harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["start a task".into()])).block_task().await?;
    harness.expect_idle(id, StopReason::EndTurn).await;
    Ok(())
}

fn is_running(update: &SessionUpdate) -> bool {
    matches!(update, SessionUpdate::StateUpdate(StateUpdate::Running(_)))
}

fn is_idle(update: &SessionUpdate) -> bool {
    matches!(update, SessionUpdate::StateUpdate(StateUpdate::Idle(_)))
}

fn stored_prompts(harness: &AcpTestHarness, id: &SessionId) -> Vec<String> {
    harness
        .stored_events(id)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::User(UserEvent::Message { content, .. }) => Some(llm::ContentBlock::join_text(&content)),
            _ => None,
        })
        .collect()
}

async fn error_message_ids(harness: &mut AcpTestHarness, id: &SessionId) -> Vec<MessageId> {
    let mut ids = Vec::new();
    loop {
        let notification = harness.peer.next_session_notification().await;
        if notification.session_id != *id {
            continue;
        }
        match notification.update {
            SessionUpdate::AgentMessage(message)
                if message.content.value().into_iter().flatten().any(
                    |block| matches!(block, ContentBlock::Text(text) if text.text.contains("provider failed")),
                ) =>
            {
                ids.push(message.message_id);
            }
            SessionUpdate::StateUpdate(StateUpdate::Idle(_)) => return ids,
            _ => {}
        }
    }
}
