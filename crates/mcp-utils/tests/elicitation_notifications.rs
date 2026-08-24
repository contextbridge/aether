use mcp_utils::client::{McpClient, McpClientEvent, client_capabilities};
use mcp_utils::testing::connect;
use rmcp::ServerHandler;
use rmcp::model::{ClientInfo, CustomNotification, Implementation, ServerNotification};
use tokio::sync::mpsc;

#[derive(Clone)]
struct CompletionServer;

impl ServerHandler for CompletionServer {}

#[tokio::test]
async fn custom_completion_notification_includes_the_source_server() {
    let (event_tx, mut event_rx) = mpsc::channel(1);
    let client = McpClient::new(
        ClientInfo::new(client_capabilities(), Implementation::new("test-client", "1.0.0")),
        "linear".to_string(),
        event_tx,
    );
    let (server, client) = connect(CompletionServer, client).await.expect("connect MCP peers");

    server
        .send_notification(ServerNotification::CustomNotification(CustomNotification::new(
            "notifications/elicitation/complete",
            Some(serde_json::json!({ "elicitationId": "oauth-1" })),
        )))
        .await
        .expect("send completion notification");

    let event = event_rx.recv().await.expect("receive completion event");
    assert!(matches!(
        event,
        McpClientEvent::ElicitationComplete { server_name, elicitation_id }
            if server_name == "linear" && elicitation_id == "oauth-1"
    ));

    client.cancel().await.expect("close MCP client");
}
