use acp_utils::notifications::McpRequest;
use aether_cli::acp::testing::AcpTestHarness;
use agent_client_protocol::schema::v2::ElicitationMode;
use mcp_utils::testing::{FakeAuthServer, FakeMcpServer, complete_authorization};
use utils::mcp_status::{McpServerAuthCapability, McpServerStatus};

#[tokio::test(flavor = "current_thread")]
async fn signing_in_to_an_mcp_server_runs_through_the_client_and_reports_completion() {
    AcpTestHarness::run(|mut harness| async move {
        let auth = FakeAuthServer::start(FakeMcpServer::math()).await;
        harness.add_mcp_server(auth.spec("linear"));
        harness
            .peer
            .queue_elicitation_response(serde_json::from_value(serde_json::json!({"action": "accept"})).unwrap());
        let session = harness.insert_agent_switching_session().await;
        let needs_oauth = wait_for_linear(&mut harness, |status| *status == McpServerStatus::NeedsOAuth).await;
        assert_eq!(needs_oauth, McpServerAuthCapability::OAuth);

        let session_id = session.session_id().0.to_string();
        harness
            .client_cx
            .send_notification(McpRequest::Authenticate { session_id, server_name: "linear".to_string() })
            .unwrap();

        let request = harness.peer.next_elicitation_request().await;
        let ElicitationMode::Url(prompt) = request.mode else { panic!("sign-in asks the client to open a URL") };
        complete_authorization(&prompt.url).await;
        let completion = harness.peer.next_elicitation_completion().await;
        assert_eq!(completion.elicitation_id, prompt.elicitation_id);
        wait_for_linear(&mut harness, |status| *status == McpServerStatus::Connected { tool_count: 2 }).await;
        harness.shutdown().await;
    })
    .await;
}

async fn wait_for_linear(
    harness: &mut AcpTestHarness,
    reached: impl Fn(&McpServerStatus) -> bool,
) -> McpServerAuthCapability {
    loop {
        let statuses = harness.next_mcp_server_statuses().await;
        if let Some(linear) = statuses.iter().find(|entry| entry.name == "linear" && reached(&entry.status)) {
            return linear.auth_capability;
        }
    }
}
