use aether_cli::acp::testing::AcpTestHarness;
use aether_core::core::agent;
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, ContentBlock, PromptRequest, SessionId, SessionUpdate, StopReason,
};
use llm::{LlmResponse, testing::FakeLlmProvider};
use std::sync::Arc;
use tokio::{sync::Notify, task::LocalSet};

#[tokio::test(flavor = "current_thread")]
async fn cancel_mid_stream_interrupts_prompt() {
    LocalSet::new()
        .run_until(async {
            let release = Arc::new(Notify::new());
            let provider = FakeLlmProvider::new(vec![vec![
                LlmResponse::start("msg_1"),
                LlmResponse::text("hello"),
                LlmResponse::text(" world"),
                LlmResponse::done(),
            ]])
            .pause_turn_after(0, 1, release);
            let (tx, rx, handle) = agent(provider).spawn().await.unwrap();
            let mut harness = AcpTestHarness::start().await;
            let id = SessionId::new("test-session");
            harness.insert_stub_session(tx, rx, handle, id.clone(), "fake:fake").await;
            let response = harness
                .client_cx
                .send_request(PromptRequest::new(id.clone(), vec!["hi".into()]))
                .block_task()
                .await
                .unwrap();
            assert_eq!(serde_json::to_value(response).unwrap(), serde_json::json!({}));
            loop {
                if let SessionUpdate::AgentMessageChunk(chunk) = harness.peer.next_session_notification().await.update
                    && let ContentBlock::Text(text) = chunk.content
                    && text.text.contains("hello")
                {
                    break;
                }
            }
            harness.client_cx.send_notification(CancelSessionNotification::new(id.clone())).unwrap();
            harness.expect_idle(&id, StopReason::Cancelled).await;
        })
        .await;
}
