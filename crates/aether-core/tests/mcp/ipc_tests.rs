use aether_core::testing::McpTestBuilder;
use futures::StreamExt;
use mcp_utils::client::{ClientOptions, McpClient, ToolCallEvent, ToolCallOptions, Transport};
use mcp_utils::gateway::{LIST_SERVERS_TOOL, ToolExposure, ToolFilter, ToolMatcher};
use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse};
use rmcp::model::{CallToolResult, CreateTaskResult, DetailedTask, Task, TaskPayload, TaskStatus};
use serde_json::{Map, Value, json};
use std::path::Path;

#[tokio::test]
async fn no_deferred_servers_do_not_create_a_gateway() {
    let mcp_test = McpTestBuilder::new().server("math", FakeMcpServer::new()).build().await;

    assert!(mcp_test.deferred_tools_socket().is_none());
}

#[tokio::test]
async fn gateway_discovers_and_calls_only_deferred_tools() {
    let exposure =
        ToolExposure::Deferred(ToolFilter { allow: vec![ToolMatcher::name("divide_numbers")], deny: Vec::new() });
    let mcp_test = McpTestBuilder::new().server_with_exposure("math", FakeMcpServer::new(), exposure).build().await;
    let client = connect(mcp_test.deferred_tools_socket().expect("deferred gateway exists")).await;

    let servers = call(&client, LIST_SERVERS_TOOL, json!({})).await.unwrap();
    assert_eq!(
        servers.structured_content,
        Some(json!([{"name": "math", "description": "A fake MCP server for testing"}]))
    );

    let tools = client.list_tools().await.unwrap();
    let names = tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>();
    assert!(names.contains(&"math__divide_numbers"));
    assert!(!names.contains(&"math__add_numbers"));
    let divide = tools.iter().find(|tool| tool.name == "math__divide_numbers").unwrap();
    assert_eq!(divide.description.as_deref(), Some("Divides two numbers"));

    let result = call(&client, "math__divide_numbers", json!({"a": 8, "b": 2})).await.unwrap();
    assert_eq!(result.structured_content, Some(json!({"quotient": 4})));

    let error = call(&client, "math__add_numbers", json!({"a": 1, "b": 2}))
        .await
        .expect_err("model-visible tools are rejected by the deferred route");
    assert!(error.to_string().contains("exposed directly") || error.to_string().contains("Tool not found"));
}

#[tokio::test]
async fn gateway_discovery_and_execution_share_the_tool_filter() {
    let server = FakeMcpServer::new().with_tool(FakeTool::new("secret").responds(FakeToolResponse::text("hidden")));
    let filter = ToolFilter { allow: Vec::new(), deny: vec![ToolMatcher::name("math__secret")] };
    let mcp_test = McpTestBuilder::new().deferred_server("math", server).tool_filter(filter).build().await;
    let client = connect(mcp_test.deferred_tools_socket().expect("deferred gateway exists")).await;

    let tools = client.list_tools().await.unwrap();
    assert!(!tools.iter().any(|tool| tool.name == "math__secret"));
    let error = call(&client, "math__secret", json!({})).await.expect_err("filtered tools cannot be executed");
    assert!(error.to_string().contains("Tool not found"));
}

#[tokio::test]
async fn gateway_reduces_mcp_task_completion_to_the_final_result() {
    let now = chrono::Utc::now().to_rfc3339();
    let working = Task::new("gateway-task", TaskStatus::Working, now.clone(), now.clone()).with_poll_interval_ms(1);
    let completed = Task::new("gateway-task", TaskStatus::Completed, now.clone(), now);
    let final_result = CallToolResult::structured(json!({"done": true}));
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("background").responds(FakeToolResponse::task(CreateTaskResult::new(working))))
        .with_task(
            "gateway-task",
            [DetailedTask::new(
                completed,
                TaskPayload::Completed {
                    result: serde_json::from_value(serde_json::to_value(final_result).unwrap()).unwrap(),
                },
            )],
        );
    let mcp_test = McpTestBuilder::new().deferred_server("math", server).build().await;
    let client = connect(mcp_test.deferred_tools_socket().expect("deferred gateway exists")).await;

    let events = client.call_tool("math__background", Map::new(), ToolCallOptions::default()).collect::<Vec<_>>().await;

    assert!(
        !events.iter().any(|event| matches!(event, ToolCallEvent::TaskCreated(_))),
        "the gateway resolves MCP Tasks itself"
    );
    assert!(matches!(
        events.last(),
        Some(ToolCallEvent::Done { task: None, result: Ok(result) }) if result.structured_content == Some(json!({"done": true}))
    ));
}

#[tokio::test]
async fn gateway_socket_is_removed_with_the_runtime() {
    let mcp_test = McpTestBuilder::new().deferred_server("math", FakeMcpServer::new()).build().await;
    let endpoint = mcp_test.deferred_tools_socket().expect("deferred gateway exists").to_path_buf();
    let directory = endpoint.parent().unwrap().to_path_buf();
    assert!(endpoint.exists());

    drop(mcp_test);

    assert!(!endpoint.exists());
    assert!(!directory.exists());
}

async fn connect(socket: &Path) -> McpClient {
    McpClient::connect("aether", Transport::Unix(socket.to_path_buf()), &ClientOptions::default()).await.unwrap()
}

async fn call(
    client: &McpClient,
    tool: &str,
    arguments: Value,
) -> Result<CallToolResult, mcp_utils::client::ToolCallError> {
    let arguments = arguments.as_object().cloned().expect("arguments are an object");
    client.call_tool(tool, arguments, ToolCallOptions::default()).result().await
}
