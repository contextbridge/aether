use mcp_utils::client::{McpError, McpManager};
use mcp_utils::testing::{FakeMcpServer, fake_mcp};

#[tokio::test]
async fn server_names_containing_the_namespace_delimiter_are_rejected() {
    let mut manager = McpManager::new();

    let result = manager.add_mcps(vec![fake_mcp("my__server", FakeMcpServer::new())]).await;

    assert!(matches!(result, Err(McpError::InvalidServerName(name)) if name == "my__server"));
}
