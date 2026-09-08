use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, McpTestBuilder};
use mcp_utils::ServiceExt;
use mcp_utils::client::{DeferredToolRules, ToolExposure, ToolFilter, ToolMatcher};
use mcp_utils::tool_gateway::{LIST_SERVERS_TOOL, connect};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CreateTaskResult, DetailedTask, Task, TaskPayload,
    TaskStatus,
};
use serde_json::json;

#[tokio::test]
async fn no_deferred_servers_do_not_create_a_gateway() {
    let mcp_test = McpTestBuilder::new().server("math", FakeMcpServer::new()).build().await;

    assert!(mcp_test.gateway_endpoint().is_none());
}

#[tokio::test]
async fn gateway_discovers_and_calls_only_deferred_tools() {
    let exposure = ToolExposure::Deferred(DeferredToolRules::new(&["divide_numbers"], &[]));
    let mcp_test = McpTestBuilder::new().server_with_exposure("math", FakeMcpServer::new(), exposure).build().await;
    let client =
        ().serve(connect(mcp_test.gateway_endpoint().expect("deferred gateway exists")).await.unwrap()).await.unwrap();

    let servers = client.call_tool_once(CallToolRequestParams::new(LIST_SERVERS_TOOL)).await.unwrap();
    let CallToolResponse::Complete(servers) = servers else { panic!("server discovery is complete") };
    assert_eq!(
        servers.structured_content,
        Some(json!([{"name": "math", "description": "A fake MCP server for testing"}]))
    );

    let tools = client.list_tools(None).await.unwrap().tools;
    let names = tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>();
    assert!(names.contains(&"math__divide_numbers"));
    assert!(!names.contains(&"math__add_numbers"));
    let divide = tools.iter().find(|tool| tool.name == "math__divide_numbers").unwrap();
    assert_eq!(divide.description.as_deref(), Some("Divides two numbers"));

    let result = client
        .call_tool_once(
            CallToolRequestParams::new("math__divide_numbers")
                .with_arguments(json!({"a": 8, "b": 2}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let CallToolResponse::Complete(result) = result else {
        panic!("gateway returns a complete result");
    };
    assert_eq!(result.structured_content, Some(json!({"quotient": 4})));

    let error = client
        .call_tool_once(
            CallToolRequestParams::new("math__add_numbers")
                .with_arguments(json!({"a": 1, "b": 2}).as_object().unwrap().clone()),
        )
        .await
        .expect_err("model-visible tools are rejected by the deferred route");
    assert!(error.to_string().contains("exposed directly") || error.to_string().contains("Tool not found"));
}

#[tokio::test]
async fn gateway_discovery_and_execution_share_the_tool_filter() {
    let server = FakeMcpServer::new().with_tool(FakeTool::new("secret").responds(FakeToolResponse::text("hidden")));
    let filter = ToolFilter { allow: Vec::new(), deny: vec![ToolMatcher::name("math__secret")] };
    let mcp_test = McpTestBuilder::new().deferred_server("math", server).tool_filter(filter).build().await;
    let client =
        ().serve(connect(mcp_test.gateway_endpoint().expect("deferred gateway exists")).await.unwrap()).await.unwrap();

    let tools = client.list_tools(None).await.unwrap().tools;
    assert!(!tools.iter().any(|tool| tool.name == "math__secret"));
    let error = client
        .call_tool_once(CallToolRequestParams::new("math__secret"))
        .await
        .expect_err("filtered tools cannot be executed");
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
    let client =
        ().serve(connect(mcp_test.gateway_endpoint().expect("deferred gateway exists")).await.unwrap()).await.unwrap();

    let response = client.call_tool_once(CallToolRequestParams::new("math__background")).await.unwrap();

    let CallToolResponse::Complete(result) = response else { panic!("gateway resolves MCP Tasks") };
    assert_eq!(result.structured_content, Some(json!({"done": true})));
}

#[tokio::test]
async fn gateway_socket_is_removed_with_the_runtime() {
    let mcp_test = McpTestBuilder::new().deferred_server("math", FakeMcpServer::new()).build().await;
    let endpoint = mcp_test.gateway_endpoint().expect("deferred gateway exists").to_path_buf();
    let directory = endpoint.parent().unwrap().to_path_buf();
    assert!(endpoint.exists());

    drop(mcp_test);

    assert!(!endpoint.exists());
    assert!(!directory.exists());
}
