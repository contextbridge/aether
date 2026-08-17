use aether_core::{
    events::{AgentCommand, Command},
    mcp::{ServerFactory, mcp},
    testing::{FakeMcpServer, FakeTool},
};
use futures::FutureExt;
use mcp_utils::client::{InMemoryServerSpec, McpClientEvent, McpServer, McpTransport};
use rmcp::{RoleServer, service::DynService};
use tokio::sync::mpsc;

#[tokio::test]
async fn session_synchronizes_agent_while_forwarding_host_events() {
    let server = FakeMcpServer::new();
    let state = server.state();
    let factory_server = server.clone();
    let factory: ServerFactory = Box::new(move |_, _| {
        let server = factory_server.clone();
        async move { Box::new(server) as Box<dyn DynService<RoleServer>> }.boxed()
    });
    let configured = McpServer::new(
        "dynamic",
        McpTransport::InMemory {
            spec: InMemoryServerSpec { factory: "dynamic-factory".to_string(), args: Vec::new(), input: None },
        },
        mcp_utils::client::ToolExposure::Direct,
    );
    let session = mcp("/workspace")
        .register_in_memory_server("dynamic-factory", factory)
        .with_servers(vec![configured])
        .spawn()
        .await
        .unwrap();
    let handle = session.handle().clone();
    let (agent_tx, mut agent_rx) = mpsc::channel(32);
    let (runtime, mut host_events) = session.connect_agent(agent_tx).await.split();

    let mut received_initial_tools = false;
    let mut received_initial_instructions = false;
    while !received_initial_tools || !received_initial_instructions {
        match agent_rx.recv().await.expect("agent synchronization remains connected") {
            Command::AgentCommand(AgentCommand::UpdateTools(tools)) => {
                received_initial_tools = tools.iter().any(|tool| tool.name == "dynamic__add_numbers");
            }
            Command::AgentCommand(AgentCommand::UpdateMcpInstructions { server, body }) if server == "dynamic" => {
                received_initial_instructions = body.as_deref() == Some("A fake MCP server for testing");
            }
            _ => {}
        }
    }

    let mut received_status = false;
    loop {
        match host_events.recv().await.expect("host event stream remains connected") {
            McpClientEvent::ServerStatusesChanged(_) => received_status = true,
            McpClientEvent::ConnectionReady(_) => break,
            McpClientEvent::Elicitation(_) | McpClientEvent::AuthenticationFailed { .. } => {}
        }
    }
    assert!(received_status);

    let mut snapshots = handle.subscribe();
    snapshots.borrow_and_update();
    state.add_tool_and_notify(FakeTool::new("added_later")).await;
    snapshots.changed().await.expect("tool refresh publishes a snapshot");
    loop {
        if let Command::AgentCommand(AgentCommand::UpdateTools(tools)) =
            agent_rx.recv().await.expect("agent synchronization remains connected")
            && tools.iter().any(|tool| tool.name == "dynamic__added_later")
        {
            break;
        }
    }

    snapshots.borrow_and_update();
    state.clear_tools_and_notify().await;
    snapshots.changed().await.expect("tool removal publishes a snapshot");
    let mut received_empty_tools = false;
    let mut received_removed_instructions = false;
    while !received_empty_tools || !received_removed_instructions {
        match agent_rx.recv().await.expect("agent synchronization remains connected") {
            Command::AgentCommand(AgentCommand::UpdateTools(tools)) => received_empty_tools = tools.is_empty(),
            Command::AgentCommand(AgentCommand::UpdateMcpInstructions { server, body }) if server == "dynamic" => {
                received_removed_instructions = body.is_none();
            }
            _ => {}
        }
    }

    drop(runtime);
}
