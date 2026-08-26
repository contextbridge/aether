use aether_cli::acp::testing::AcpTestHarness;
use aether_core::core::agent;
use agent_client_protocol::Error;
use agent_client_protocol::schema::v1::{
    CloseSessionRequest, CloseSessionResponse, ContentBlock, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, PromptRequest, ResumeSessionRequest, SessionId, SessionUpdate, StopReason, TextContent,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
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
async fn malformed_and_stale_list_cursors_are_rejected() {
    with_harness(|harness| async move {
        let malformed =
            harness.client_cx.send_request(ListSessionsRequest::new().cursor("not-base64")).block_task().await;
        assert!(malformed.is_err());

        harness.append_stored_session("present", "2026-05-01T00:00:00Z");
        let stale = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "created_at": "2026-05-01T00:00:00Z",
                "session_id": "deleted"
            })
            .to_string(),
        );
        let result = harness.client_cx.send_request(ListSessionsRequest::new().cursor(stale)).block_task().await;
        assert!(result.is_err());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_list_cursor_returns_an_empty_page() {
    with_harness(|harness| async move {
        harness.append_stored_session("last", "2026-05-01T00:00:00Z");
        let cursor = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "created_at": "2026-05-01T00:00:00Z",
                "session_id": "last"
            })
            .to_string(),
        );
        let response = list(&harness, ListSessionsRequest::new().cursor(cursor)).await;
        assert!(response.sessions.is_empty());
        assert!(response.next_cursor.is_none());
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
async fn load_session_replays_persisted_transcript_over_the_server_connection() {
    with_harness(|mut harness| async move {
        let session_id = "load-session";
        harness.append_stored_session(session_id, "2026-05-01T00:00:00Z");
        harness.append_stored_prompt(session_id, "prior user");
        harness.append_stored_agent_turn(session_id, "prior assistant");

        harness
            .client_cx
            .send_request(LoadSessionRequest::new(session_id, "/tmp"))
            .block_task()
            .await
            .expect("load succeeds");

        let first = harness.peer.next_session_notification().await;
        let second = harness.peer.next_session_notification().await;
        assert!(matches!(first.update, SessionUpdate::UserMessageChunk(_)));
        assert!(matches!(second.update, SessionUpdate::AgentMessageChunk(_)));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn resume_restores_transcript_without_replay_and_replaces_active_session() {
    with_harness(|mut harness| async move {
        let active = harness.insert_agent_switching_session().await;
        let session_id = active.session_id().clone();
        harness.append_stored_session(session_id.0.as_ref(), "2026-05-01T00:00:00Z");
        harness.append_stored_prompt(session_id.0.as_ref(), "prior user");
        harness.append_stored_agent_turn(session_id.0.as_ref(), "prior assistant");
        harness.expect_available_commands(&["plan"], &[]).await;

        let resume = harness.client_cx.send_request(ResumeSessionRequest::new(session_id.clone(), "/tmp")).block_task();
        tokio::pin!(resume);
        tokio::select! {
            response = &mut resume => {
                response.expect("resume succeeds");
            }
            notification = harness.peer.next_session_notification() => {
                panic!("resume replayed historical notification: {notification:?}");
            }
        }

        let prompt = harness
            .client_cx
            .send_request(PromptRequest::new(session_id, vec![ContentBlock::Text(TextContent::new("next prompt"))]))
            .block_task()
            .await
            .expect("prompt on resumed session succeeds");
        assert_eq!(prompt.stop_reason, StopReason::EndTurn);
        harness.resume_agent().assert_saw(&["prior user", "prior assistant", "next prompt"]);
        active.planner().assert_never_ran();
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

async fn list(harness: &AcpTestHarness, request: ListSessionsRequest) -> ListSessionsResponse {
    harness.client_cx.send_request(request).block_task().await.expect("list succeeds")
}

async fn close(harness: &AcpTestHarness, request: CloseSessionRequest) -> Result<CloseSessionResponse, Error> {
    harness.client_cx.send_request(request).block_task().await
}
