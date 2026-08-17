use aether_core::events::{AgentCommand, Command};
use aether_core::mcp::mcp;
use aether_core::testing::{FakeMcpServer, fake_mcp};
use mcp_utils::client::{McpClientEvent, McpConnectionDetails};
use std::collections::BTreeMap;
use tokio::sync::mpsc;

#[tokio::test]
async fn connected_agent_receives_catalog_updates_while_host_receives_only_host_events() {
    let session = mcp("/workspace").with_servers(vec![fake_mcp("fake", FakeMcpServer::new())]).spawn().await.unwrap();
    let (agent_tx, mut agent_rx) = mpsc::channel(10);
    let initial_snapshot = McpConnectionDetails {
        instructions: BTreeMap::new(),
        tool_definitions: Vec::new(),
        server_statuses: Vec::new(),
    };

    let (runtime, mut host_event_rx) = session.connect_agent(agent_tx, initial_snapshot);

    let mut tools_updated = false;
    let mut instructions_updated = false;
    while !tools_updated || !instructions_updated {
        match agent_rx.recv().await.unwrap() {
            Command::AgentCommand(AgentCommand::UpdateTools(tools)) => {
                tools_updated = tools.iter().any(|tool| tool.name == "fake__add_numbers");
            }
            Command::AgentCommand(AgentCommand::UpdateMcpInstructions { server, body }) => {
                instructions_updated = server == "fake" && body.as_deref() == Some("A fake MCP server for testing");
            }
            _ => {}
        }
    }

    while let Some(event) = host_event_rx.recv().await {
        assert!(!matches!(
            event,
            McpClientEvent::ToolDefinitionsChanged(_) | McpClientEvent::ServerInstructionsUpdated { .. }
        ));
        if matches!(event, McpClientEvent::ConnectionReady(_)) {
            break;
        }
    }

    let snapshot = runtime.latest_snapshot().unwrap();
    assert!(snapshot.tool_definitions.iter().any(|tool| tool.name == "fake__add_numbers"));
    assert_eq!(snapshot.instructions.get("fake").map(String::as_str), Some("A fake MCP server for testing"));
}
