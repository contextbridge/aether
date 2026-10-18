use aether_cli::acp::testing::AcpTestHarness;
use agent_client_protocol::schema::v2::{CancelSessionNotification, ContentBlock, SessionUpdate, StopReason};
use llm::testing::{FakeLlmProvider, llm_response};
use std::sync::Arc;
use tokio::sync::Notify;

#[tokio::test(flavor = "current_thread")]
async fn cancel_mid_stream_interrupts_prompt() {
    AcpTestHarness::run(|mut harness| async move {
        let release = Arc::new(Notify::new());
        let provider = FakeLlmProvider::new(vec![llm_response().text(&["hello", " world"]).build()])
            .pause_turn_after(0, 1, release);
        let id = harness.session("test-session", provider).insert().await;
        let response = harness.prompt(&id, "hi").await.unwrap();
        assert_eq!(serde_json::to_value(&response).unwrap(), serde_json::json!({"messageId": response.message_id}));
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
