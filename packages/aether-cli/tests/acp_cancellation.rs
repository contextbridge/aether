use aether_cli::acp::testing::AcpTestHarness;
use aether_core::context::ext::SessionEvent;
use aether_core::core::agent;
use aether_core::events::AgentMessage;
use agent_client_protocol::schema::{
    CancelNotification, ContentBlock, PromptRequest, SessionId, SessionUpdate, StopReason, TextContent,
};
use llm::LlmResponse;
use llm::testing::FakeLlmProvider;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn cancel_mid_stream_interrupts_prompt() {
    LocalSet::new()
        .run_until(async {
            let release = Arc::new(Notify::new());
            let llm = FakeLlmProvider::new(vec![vec![
                LlmResponse::start("msg_1"),
                LlmResponse::text("hello"),
                LlmResponse::text(" world"),
                LlmResponse::done(),
            ]])
            // Simulate the agent streaming LLM response "forever"
            .pause_turn_after(0, 1, Arc::clone(&release));

            let (agent_tx, agent_rx, agent_handle) = agent(llm).spawn().await.expect("agent spawns");
            let mut harness = AcpTestHarness::start().await;
            let session_id = SessionId::new("test-session");
            harness.insert_stub_session(agent_tx, agent_rx, agent_handle, session_id.clone(), "fake:fake").await;

            let prompt_fut = harness
                .client_cx
                .send_request(PromptRequest::new(session_id.clone(), vec![ContentBlock::Text(TextContent::new("hi"))]))
                .block_task();

            tokio::pin!(prompt_fut);

            loop {
                tokio::select! {
                    biased;
                    notif = harness.peer.next_session_notification() => {
                        if let SessionUpdate::AgentMessageChunk(chunk) = &notif.update
                            && let ContentBlock::Text(t) = &chunk.content
                            && t.text.contains("hello")
                        {
                            break;
                        }
                    }
                    _ = &mut prompt_fut => panic!("prompt completed before any text chunk arrived"),
                }
            }

            // Cancellation should work when the LLM is busy streaming
            harness
                .client_cx
                .send_notification(CancelNotification::new(session_id))
                .expect("cancel notification queues");

            drop(release);
            let response = prompt_fut.await.expect("prompt request returned ok");
            assert_eq!(response.stop_reason, StopReason::Cancelled);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn re_registering_a_session_id_cancels_the_displaced_actor() {
    LocalSet::new()
        .run_until(async {
            let release = Arc::new(Notify::new());
            let first_llm = FakeLlmProvider::new(vec![vec![
                LlmResponse::start("msg_1"),
                LlmResponse::text("from first actor"),
                LlmResponse::done(),
            ]])
            .pause_turn_after(0, 1, Arc::clone(&release));
            let second_llm = FakeLlmProvider::new(vec![]);

            let (first_tx, first_rx, first_handle) = agent(first_llm).spawn().await.expect("first agent spawns");
            let (second_tx, second_rx, second_handle) = agent(second_llm).spawn().await.expect("second agent spawns");

            let mut harness = AcpTestHarness::start().await;
            let session_id = SessionId::new("forked-session");
            harness.append_stored_session("forked-session", "2026-06-11T00:00:00Z");
            harness.insert_stub_session(first_tx, first_rx, first_handle, session_id.clone(), "fake:fake").await;

            let prompt_fut = harness
                .client_cx
                .send_request(PromptRequest::new(session_id.clone(), vec![ContentBlock::Text(TextContent::new("hi"))]))
                .block_task();
            tokio::pin!(prompt_fut);

            loop {
                tokio::select! {
                    biased;
                    notif = harness.peer.next_session_notification() => {
                        if let SessionUpdate::AgentMessageChunk(chunk) = &notif.update
                            && let ContentBlock::Text(t) = &chunk.content
                            && t.text.contains("from first actor")
                        {
                            break;
                        }
                    }
                    _ = &mut prompt_fut => panic!("prompt completed before any text chunk arrived"),
                }
            }

            // Loading a session that is already live (what a fork does) replaces
            // the registered handle; the displaced actor must stop, not keep
            // streaming into the same session log.
            harness.insert_stub_session(second_tx, second_rx, second_handle, session_id, "fake:fake").await;

            release.notify_one();
            let response = prompt_fut.await.expect("prompt request returned ok");
            assert_eq!(response.stop_reason, StopReason::Cancelled);

            let appended_agent_text = harness
                .stored_events("forked-session")
                .iter()
                .any(|event| matches!(event, SessionEvent::Agent(AgentMessage::Text { .. })));
            assert!(!appended_agent_text, "displaced actor must not keep appending to the session log");
        })
        .await;
}
