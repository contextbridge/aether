use aether_core::{
    core::AgentDeps,
    events::{AgentCommand, Command},
    mcp::{ServerFactory, mcp},
    testing::{FakeMcpServer, FakeTool},
};
use futures::FutureExt;
use mcp_utils::client::{InMemoryServerSpec, McpClientEvent, McpServer, McpTransport};
use rmcp::RoleServer;
use rmcp::model::{ClientCapabilities, UrlElicitationCapability};
use rmcp::service::DynService;
use tokio::sync::mpsc;

#[tokio::test]
async fn session_synchronization_does_not_keep_agent_input_open() {
    let session = mcp("/workspace").spawn().await.unwrap();
    let (agent_tx, agent_rx) = mpsc::channel(32);
    let (runtime, _) = session.connect_agent(agent_tx.clone()).await.split();

    assert_eq!(agent_rx.sender_strong_count(), 1);
    assert_eq!(agent_rx.sender_weak_count(), 1);

    drop(agent_tx);
    assert_eq!(agent_rx.sender_strong_count(), 0);
    drop(runtime);
}

#[tokio::test]
async fn spawned_mcp_client_advertises_the_configured_elicitation_support() {
    let server = FakeMcpServer::new();
    let state = server.state();
    let factory_server = server.clone();
    let factory: ServerFactory = Box::new(move |_, _| {
        let server = factory_server.clone();
        async move { Box::new(server) as Box<dyn DynService<RoleServer>> }.boxed()
    });
    let configured = McpServer::new(
        "capability-capture",
        McpTransport::InMemory {
            spec: InMemoryServerSpec { factory: "capability-factory".to_string(), args: Vec::new(), input: None },
        },
        mcp_utils::client::ToolExposure::ModelVisible,
    );
    let mut url_only = ClientCapabilities::builder().enable_elicitation().build();
    url_only.elicitation.as_mut().unwrap().url = Some(UrlElicitationCapability::default());
    let mut session = mcp("/workspace")
        .register_in_memory_server("capability-factory", factory)
        .with_servers(vec![configured])
        .with_agent_deps(AgentDeps::default().with_mcp_client_capabilities(url_only))
        .spawn()
        .await
        .unwrap();

    session.block_until_ready().await.expect("MCP bootstrap completes");

    let capabilities = state.client_capabilities().expect("client capabilities were discovered");
    let elicitation = capabilities.elicitation.expect("elicitation is advertised");
    assert!(elicitation.form.is_none());
    assert!(elicitation.url.is_some());
}

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
        mcp_utils::client::ToolExposure::ModelVisible,
    );
    let session = mcp("/workspace")
        .register_in_memory_server("dynamic-factory", factory)
        .with_servers(vec![configured])
        .spawn()
        .await
        .unwrap();
    let handle = session.handle().clone();
    let (agent_tx, mut agent_rx) = mpsc::channel(32);
    let (runtime, mut host_events) = session.connect_agent(agent_tx.clone()).await.split();

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
            McpClientEvent::Elicitation(_)
            | McpClientEvent::ElicitationComplete { .. }
            | McpClientEvent::AuthenticationFailed { .. } => {}
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
