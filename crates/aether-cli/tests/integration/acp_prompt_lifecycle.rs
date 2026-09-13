use aether_cli::acp::testing::AcpTestHarness;
use aether_core::core::agent;
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, ContentBlock, PromptRequest, SessionId, SessionUpdate, StateUpdate, StopReason,
};
use llm::{LlmResponse, testing::FakeLlmProvider};
use std::sync::Arc;
use tokio::{sync::Notify, task::LocalSet};

#[tokio::test(flavor = "current_thread")]
async fn cancel_sent_before_acceptance_is_not_lost() {
    LocalSet::new()
        .run_until(async {
            let (tx, rx, handle) = agent(FakeLlmProvider::new(vec![vec![
                LlmResponse::start("reply"),
                LlmResponse::text("reply"),
                LlmResponse::done(),
            ]]))
            .spawn()
            .await
            .unwrap();
            let mut harness = AcpTestHarness::start().await;
            let id = SessionId::new("early-cancel");
            harness.insert_stub_session(tx, rx, handle, id.clone(), "fake:fake").await;
            let prompt = harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["hi".into()])).block_task();
            harness.client_cx.send_notification(CancelSessionNotification::new(id.clone())).unwrap();
            prompt.await.unwrap();
            harness.expect_idle(&id, StopReason::Cancelled).await;
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
async fn unknown_session_is_rejected_before_acceptance() {
    LocalSet::new()
        .run_until(async {
            let harness = AcpTestHarness::start().await;
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
            let provider = FakeLlmProvider::new(vec![vec![
                LlmResponse::start("reply"),
                LlmResponse::text("hello"),
                LlmResponse::done(),
            ]])
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
            assert_eq!(serde_json::to_value(response).unwrap(), serde_json::json!({}));
            loop {
                match harness.peer.next_session_notification().await.update {
                    SessionUpdate::UserMessage(_) => break,
                    SessionUpdate::AvailableCommandsUpdate(_) | SessionUpdate::ConfigOptionUpdate(_) => {}
                    update => panic!("unexpected update before user acknowledgement: {update:?}"),
                }
            }
            assert!(matches!(
                harness.peer.next_session_notification().await.update,
                SessionUpdate::StateUpdate(StateUpdate::Running(_))
            ));
            let second =
                harness.client_cx.send_request(PromptRequest::new(id.clone(), vec!["again".into()])).block_task().await;
            assert!(second.is_err());
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
