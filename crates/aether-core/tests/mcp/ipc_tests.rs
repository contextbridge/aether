use aether_core::mcp::{DeferredToolGateway, mcp};
use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, fake_mcp};
use mcp_utils::ServiceExt;
use mcp_utils::client::{DeferredToolRules, ToolExposure};
use mcp_utils::tool_gateway::{LIST_SERVERS_TOOL, ServerDescription, UnixSocketPath};
use rmcp::{RoleClient, model::CallToolRequestParams, service::RunningService};
use serde_json::{Map, json};
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn ipc_discovers_and_executes_deferred_tools_over_mcp() {
    let workspace = tempfile::tempdir().unwrap();
    let gateway = DeferredToolGateway::bind().unwrap();
    let endpoint = gateway.endpoint();
    let builder = mcp(workspace.path())
        .with_servers(vec![fake_mcp("fake", FakeMcpServer::new()).with_exposure(ToolExposure::deferred_all())]);
    let mut spawn = builder.spawn().await.unwrap();
    let gateway = gateway.start(spawn.command_client());
    let socket_path = endpoint.socket_path().to_path_buf();
    spawn.block_until_ready().await.unwrap();
    let client = connect(&endpoint).await;

    let servers = list_servers(&client).await;
    assert_eq!(servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(), ["fake"]);
    let tools = client.peer().list_all_tools().await.unwrap();
    assert!(tools.iter().any(|tool| tool.name == "fake__add_numbers"));

    let arguments = Map::from_iter([("a".into(), json!(20)), ("b".into(), json!(22))]);
    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("fake__add_numbers").with_arguments(arguments))
        .await
        .unwrap();
    assert_eq!(result.structured_content, Some(json!({"sum": 42})));

    drop(client);
    drop(gateway);
    drop(spawn);
    assert!(!socket_path.exists());
    assert!(tokio::net::UnixStream::connect(&socket_path).await.is_err());
}

#[tokio::test]
async fn tools_added_after_discovery_are_available_to_gateway() {
    let server = FakeMcpServer::new();
    let state = server.state();
    let gateway = DeferredToolGateway::bind().unwrap();
    let endpoint = gateway.endpoint();
    let builder =
        mcp("/workspace").with_servers(vec![fake_mcp("fake", server).with_exposure(ToolExposure::deferred_all())]);
    let mut spawn = builder.spawn().await.unwrap();
    let _gateway = gateway.start(spawn.command_client());
    spawn.block_until_ready().await.unwrap();
    let client = connect(&endpoint).await;

    state.add_tool(FakeTool::new("added_later").responds(FakeToolResponse::text("available")));

    let tools = client.peer().list_all_tools().await.unwrap();
    assert!(tools.iter().any(|tool| tool.name == "fake__added_later"));
    let result = client.peer().call_tool(CallToolRequestParams::new("fake__added_later")).await.unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "available");
}

#[tokio::test]
async fn selective_policy_partitions_model_visible_and_deferred_routes() {
    let exposure = ToolExposure::Deferred(DeferredToolRules::new(&[], &["add_*"]));
    let gateway = DeferredToolGateway::bind().unwrap();
    let endpoint = gateway.endpoint();
    let builder = mcp("/workspace").with_servers(vec![fake_mcp("fake", FakeMcpServer::new()).with_exposure(exposure)]);
    let mut spawn = builder.spawn().await.unwrap();
    let _gateway = gateway.start(spawn.command_client());
    let snapshot = spawn.block_until_ready().await.unwrap();
    let client = connect(&endpoint).await;

    assert!(snapshot.tool_definitions.iter().any(|tool| tool.name == "fake__add_numbers"));
    assert!(!snapshot.tool_definitions.iter().any(|tool| tool.name == "fake__divide_numbers"));
    let tools = client.peer().list_all_tools().await.unwrap();
    assert!(!tools.iter().any(|tool| tool.name == "fake__add_numbers"));
    assert!(tools.iter().any(|tool| tool.name == "fake__divide_numbers"));
}

#[tokio::test]
async fn ipc_runtime_directory_is_private() {
    let gateway = DeferredToolGateway::bind().unwrap();
    let endpoint = gateway.endpoint();
    let spawn = mcp("/workspace")
        .with_servers(vec![fake_mcp("fake", FakeMcpServer::new()).with_exposure(ToolExposure::deferred_all())])
        .spawn()
        .await
        .unwrap();
    let _gateway = gateway.start(spawn.command_client());
    let mode = std::fs::metadata(endpoint.socket_path().parent().unwrap()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    assert!(endpoint.socket_path().as_os_str().len() < 104);
}

async fn connect(endpoint: &UnixSocketPath) -> RunningService<RoleClient, ()> {
    ().serve(tokio::net::UnixStream::connect(endpoint.socket_path()).await.unwrap()).await.unwrap()
}

async fn list_servers(client: &RunningService<RoleClient, ()>) -> Vec<ServerDescription> {
    let result = client.peer().call_tool(CallToolRequestParams::new(LIST_SERVERS_TOOL)).await.unwrap();
    serde_json::from_value(result.structured_content.unwrap()).unwrap()
}
