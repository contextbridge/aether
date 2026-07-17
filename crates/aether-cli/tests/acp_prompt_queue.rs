use aether_cli::acp::testing::AcpTestHarness;
use aether_core::core::agent;
use agent_client_protocol::schema::{ContentBlock, PromptRequest, SessionId, SessionUpdate, StopReason, TextContent};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn prompt_submitted_while_busy_runs_on_the_next_agent_iteration() {
    LocalSet::new()
        .run_until(async {
            let release = Arc::new(Notify::new());
            let llm = FakeLlmProvider::new(vec![
                vec![LlmResponse::start("msg_1"), LlmResponse::text("first reply"), LlmResponse::done()],
                vec![LlmResponse::start("msg_2"), LlmResponse::text("second reply"), LlmResponse::done()],
            ])
            .pause_turn_after(0, 1, Arc::clone(&release));

            let (agent_tx, agent_rx, agent_handle) = agent(llm).spawn().await.expect("agent spawns");
            let mut harness = AcpTestHarness::start().await;
            let session_id = SessionId::new("queued-prompt-session");
            harness.insert_stub_session(agent_tx, agent_rx, agent_handle, session_id.clone(), "fake:fake").await;

            let client_cx = harness.client_cx.clone();
            let first = send_prompt(client_cx.clone(), session_id.clone(), "first".to_string());
            tokio::pin!(first);

            loop {
                tokio::select! {
                    biased;
                    notification = harness.peer.next_session_notification() => {
                        if let SessionUpdate::AgentMessageChunk(chunk) = &notification.update
                            && let ContentBlock::Text(text) = &chunk.content
                            && text.text.contains("first reply")
                        {
                            break;
                        }
                    }
                    _ = &mut first => panic!("first prompt completed before its reply started"),
                }
            }

            let second =
                tokio::task::spawn_local(send_prompt(client_cx.clone(), session_id.clone(), "second".to_string()));
            tokio::task::yield_now().await;
            release.notify_waiters();

            assert_eq!(first.await.expect("first prompt succeeds").stop_reason, StopReason::EndTurn);
            assert_eq!(
                second.await.expect("queued prompt task completes").expect("queued prompt succeeds").stop_reason,
                StopReason::EndTurn
            );
        })
        .await;
}

async fn send_prompt(
    client_cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>,
    session_id: SessionId,
    text: String,
) -> Result<agent_client_protocol::schema::PromptResponse, agent_client_protocol::Error> {
    client_cx
        .send_request(PromptRequest::new(session_id, vec![ContentBlock::Text(TextContent::new(text))]))
        .block_task()
        .await
}
