use aether_core::mcp::{
    DeferredServer, DeferredTool, DeferredToolsClient, McpBuilder, McpCatalog, ToolExposure, ToolFilter, ToolMatcher,
    mcp,
};
use aether_core::testing::ready_catalog;
use futures::StreamExt;
use mcp_utils::client::{ToolCallError, ToolCallEvent};
use mcp_utils::gateway::ServerSpec;
use mcp_utils::testing::{FakeMcpServer, FakeTask, FakeTool, FakeToolResponse, args, completed_task_payload, task};
use rmcp::model::{CallToolResult, DetailedTask, ErrorCode, TaskStatus};
use rmcp::service::ServiceError;
use serde_json::{Map, json};
use std::time::Duration;

#[tokio::test]
async fn no_deferred_servers_do_not_create_a_gateway() {
    let mcp =
        ready_catalog(mcp("/workspace").with_servers(vec![ServerSpec::in_process("math", FakeMcpServer::math())]))
            .await;

    assert!(mcp.deferred_tools_socket().is_none());
}

#[tokio::test]
async fn gateway_discovers_and_calls_only_deferred_tools() {
    let exposure =
        ToolExposure::Deferred(ToolFilter { allow: vec![ToolMatcher::name("divide_numbers")], deny: Vec::new() });
    let mcp =
        ready_catalog(mcp("/workspace").with_server(ServerSpec::in_process("math", FakeMcpServer::math()), exposure))
            .await;
    let client = connect(&mcp).await;

    assert_eq!(
        client.servers().await.unwrap(),
        [DeferredServer {
            name: "math".to_string(),
            description: "A fake MCP server for testing".to_string(),
            tools: vec![DeferredTool {
                name: "divide_numbers".to_string(),
                description: Some("Divides two numbers".to_string())
            }],
        }]
    );
    let divide = client.tool("math", "divide_numbers").await.unwrap().expect("divide_numbers is deferred");
    assert_eq!(divide.description.as_deref(), Some("Divides two numbers"));
    assert!(client.tool("math", "add_numbers").await.unwrap().is_none());

    let result = call(&client, "math", "divide_numbers", json!({"a": 8, "b": 2})).await.unwrap();
    assert_eq!(result.structured_content, Some(json!({"quotient": 4})));

    let error = call(&client, "math", "add_numbers", json!({"a": 1, "b": 2}))
        .await
        .expect_err("model-visible tools are rejected by the deferred route");
    assert!(error.to_string().contains("exposed directly; call 'math__add_numbers'"));
}

#[tokio::test]
async fn gateway_discovery_and_execution_share_the_tool_filter() {
    let server = FakeMcpServer::new().with_tool(FakeTool::new("secret").responds(FakeToolResponse::text("hidden")));
    let filter = ToolFilter { allow: Vec::new(), deny: vec![ToolMatcher::name("math__secret")] };
    let mcp = ready_catalog(deferred("math", server).with_tool_filter(filter)).await;
    let client = connect(&mcp).await;

    assert!(client.tool("math", "secret").await.unwrap().is_none());
    let error = call(&client, "math", "secret", json!({})).await.expect_err("filtered tools cannot be executed");
    assert!(matches!(
        error,
        ToolCallError::Call(ServiceError::McpError(error))
            if error.code == ErrorCode::INVALID_PARAMS && error.message.contains("Tool not found")
    ));
}

#[tokio::test]
async fn gateway_reduces_mcp_task_completion_to_the_final_result() {
    let payload = completed_task_payload(CallToolResult::structured(json!({"done": true})));
    let completed = DetailedTask::new(task("gateway-task", TaskStatus::Completed), payload);
    let mcp =
        ready_catalog(deferred("math", FakeTask::new("gateway-task").states([completed]).server("background"))).await;
    let client = connect(&mcp).await;

    let events = client.call("math", "background", Map::new(), TIMEOUT).collect::<Vec<_>>().await;

    assert!(
        !events.iter().any(|event| matches!(event, ToolCallEvent::TaskCreated(_))),
        "the gateway resolves MCP Tasks itself"
    );
    assert!(matches!(
        events.last(),
        Some(ToolCallEvent::Done(Ok(result))) if result.structured_content == Some(json!({"done": true}))
    ));
}

#[tokio::test]
async fn the_socket_relays_upstream_progress() {
    let ci = FakeMcpServer::new()
        .with_tool(FakeTool::new("build").responds(FakeToolResponse::text("built").progress_message(1.0, "compiling")));
    let mcp = ready_catalog(deferred("ci", ci)).await;
    let client = connect(&mcp).await;

    let events = client.call("ci", "build", Map::new(), TIMEOUT).collect::<Vec<_>>().await;

    assert!(events.iter().any(|event| matches!(
        event,
        ToolCallEvent::Progress(progress) if progress.message.as_deref() == Some("compiling")
    )));
    assert!(matches!(events.last(), Some(ToolCallEvent::Done(Ok(_)))));
}

#[tokio::test]
async fn listing_servers_names_only_servers_with_deferred_tools_and_their_deferred_tools() {
    let partial = ToolExposure::Deferred(ToolFilter { allow: vec![ToolMatcher::name("add_*")], deny: vec![] });
    let mcp = ready_catalog(
        deferred("lazy", FakeMcpServer::math())
            .with_server(ServerSpec::in_process("partial", FakeMcpServer::math()), partial)
            .with_servers(vec![ServerSpec::in_process("direct", FakeMcpServer::math())]),
    )
    .await;
    let client = connect(&mcp).await;

    let servers = client.servers().await.unwrap();

    let tools = |server: &DeferredServer| server.tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>();
    assert_eq!(servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(), ["lazy", "partial"]);
    assert_eq!(tools(&servers[0]), ["add_numbers", "divide_numbers"]);
    assert_eq!(tools(&servers[1]), ["add_numbers"]);
    assert_eq!(servers[1].tools[0].description.as_deref(), Some("Adds two numbers together"));
}

#[tokio::test]
async fn disconnecting_from_the_socket_cancels_the_server_task() {
    let server = FakeTask::new("task-1").server("deferred");
    let state = server.state();
    let mcp = ready_catalog(deferred("lazy", server)).await;
    let client = connect(&mcp).await;
    let in_flight = tokio::spawn(client.call("lazy", "deferred", Map::new(), TIMEOUT).result());
    state.wait_until(|state| !state.task_get_ids().is_empty()).await;

    client.close().await;

    assert!(in_flight.await.unwrap().is_err());
    state.wait_until(|state| state.task_cancel_ids() == ["task-1"]).await;
}

#[tokio::test]
async fn the_socket_is_removed_once_the_session_is_dropped() {
    let mcp = ready_catalog(deferred("math", FakeMcpServer::math())).await;
    let endpoint = mcp.deferred_tools_socket().expect("deferred gateway exists").to_path_buf();
    let directory = endpoint.parent().unwrap().to_path_buf();
    let mut changes = mcp.changes();
    assert!(endpoint.exists());

    drop(mcp);
    while changes.next().await.is_some() {}

    assert!(!endpoint.exists());
    assert!(!directory.exists());
}

const TIMEOUT: Duration = Duration::from_mins(1);

fn deferred(name: &str, server: FakeMcpServer) -> McpBuilder {
    mcp("/workspace").with_server(ServerSpec::in_process(name, server), ToolExposure::deferred_all())
}

async fn connect(mcp: &McpCatalog) -> DeferredToolsClient {
    let socket = mcp.deferred_tools_socket().expect("deferred gateway exists").to_path_buf();
    DeferredToolsClient::connect(socket).await.unwrap()
}

async fn call(
    client: &DeferredToolsClient,
    server: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<CallToolResult, ToolCallError> {
    client.call(server, tool, args(arguments), TIMEOUT).result().await
}
