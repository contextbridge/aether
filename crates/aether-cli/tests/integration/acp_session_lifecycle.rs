use aether_cli::acp::testing::AcpTestHarness;
use aether_core::core::agent;
use agent_client_protocol::schema::v1::{
    CloseSessionRequest, ContentBlock, ListSessionsRequest, PromptRequest, ResumeSessionRequest, SessionId,
    SessionUpdate, StopReason, TextContent,
};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn list_sessions_paginates_sorted_results() {
    with_harness(|harness| async move {
        for index in 0..51 {
            harness
                .append_stored_session(&format!("session-{index:02}"), &format!("2026-05-{:02}T00:00:00Z", index + 1));
        }

        let first = list(&harness, ListSessionsRequest::new()).await;
        assert_eq!(first.sessions.len(), 50);
        assert_eq!(first.sessions[0].session_id.0.as_ref(), "session-50");
        let cursor = first.next_cursor.expect("first page has a cursor");

        let second = list(&harness, ListSessionsRequest::new().cursor(cursor)).await;
        assert_eq!(second.sessions.len(), 1);
        assert_eq!(second.sessions[0].session_id.0.as_ref(), "session-00");
        assert!(second.next_cursor.is_none());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn close_removes_idle_session_but_preserves_persisted_log() {
    with_harness(|harness| async move {
        let fake = harness.insert_agent_switching_session().await;
        harness.append_stored_session("agent-switching-session", "2026-05-01T00:00:00Z");
        harness.append_stored_prompt("agent-switching-session", "persisted prompt");

        close(&harness, CloseSessionRequest::new(fake.session_id().clone())).await.expect("close succeeds");

        let stored =
            harness.client_cx.send_request(ListSessionsRequest::new()).block_task().await.expect("list succeeds");
        assert_eq!(stored.sessions.len(), 1);
        assert_eq!(stored.sessions[0].session_id.0.as_ref(), "agent-switching-session");

        let prompt = harness
            .client_cx
            .send_request(PromptRequest::new(
                fake.session_id().clone(),
                vec![ContentBlock::Text(TextContent::new("after close"))],
            ))
            .block_task()
            .await;
        assert!(prompt.is_err(), "a closed session must no longer accept prompts");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn close_cancels_prompt_before_returning() {
    with_harness(|mut harness| async move {
        let release = Arc::new(Notify::new());
        let llm = FakeLlmProvider::new(vec![vec![
            LlmResponse::start("message"),
            LlmResponse::text("hello"),
            LlmResponse::done(),
        ]])
        .pause_turn_after(0, 1, Arc::clone(&release));
        let (agent_tx, agent_rx, agent_handle) = agent(llm).spawn().await.expect("agent spawns");
        let session_id = SessionId::new("prompting-session");
        harness.insert_stub_session(agent_tx, agent_rx, agent_handle, session_id.clone(), "fake:fake").await;

        let prompt = harness
            .client_cx
            .send_request(PromptRequest::new(session_id.clone(), vec![ContentBlock::Text(TextContent::new("hi"))]))
            .block_task();
        tokio::pin!(prompt);

        loop {
            tokio::select! {
                biased;
                notification = harness.peer.next_session_notification() => {
                    if let SessionUpdate::AgentMessageChunk(chunk) = notification.update
                        && let ContentBlock::Text(text) = chunk.content
                        && text.text.contains("hello")
                    {
                        break;
                    }
                }
                _ = &mut prompt => panic!("prompt completed before it could be closed"),
            }
        }

        let close = close(&harness, CloseSessionRequest::new(session_id));
        tokio::pin!(close);
        drop(release);
        let (prompt, close) = tokio::join!(&mut prompt, &mut close);
        assert_eq!(prompt.expect("prompt succeeds").stop_reason, StopReason::Cancelled);
        close.expect("close succeeds");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn resume_and_close_reject_unknown_sessions() {
    with_harness(|harness| async move {
        let resume = harness.client_cx.send_request(ResumeSessionRequest::new("missing", "/tmp")).block_task().await;
        assert!(resume.is_err());

        let close = close(&harness, CloseSessionRequest::new("missing")).await;
        assert!(close.is_err());
    })
    .await;
}

async fn with_harness<F, Fut>(body: F)
where
    F: FnOnce(AcpTestHarness) -> Fut,
    Fut: Future<Output = ()>,
{
    LocalSet::new()
        .run_until(async move {
            let harness = AcpTestHarness::start().await;
            body(harness).await;
        })
        .await;
}

async fn list(
    harness: &AcpTestHarness,
    request: ListSessionsRequest,
) -> agent_client_protocol::schema::v1::ListSessionsResponse {
    harness.client_cx.send_request(request).block_task().await.expect("list succeeds")
}

async fn close(
    harness: &AcpTestHarness,
    request: CloseSessionRequest,
) -> Result<agent_client_protocol::schema::v1::CloseSessionResponse, agent_client_protocol::Error> {
    harness.client_cx.send_request(request).block_task().await
}
