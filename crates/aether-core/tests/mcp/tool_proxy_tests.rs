use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, McpTest, McpTestBuilder, fake_mcp};
use mcp_utils::client::{McpClientEvent, McpConnectionDetails, McpManager, ToolExposure, ToolProxyRules};
use tokio::sync::mpsc;

async fn math_proxy() -> McpTest {
    McpTestBuilder::new().proxy_server("math", FakeMcpServer::new()).build().await
}

async fn call_proxy_tool(
    test: &McpTest,
    server: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    test.call("proxy", "call_tool", serde_json::json!({ "server": server, "tool": tool, "arguments": arguments }))
        .await
        .result
        .map(|result| result.result)
        .map_err(|error| error.error)
}

fn proxy_instructions(snapshot: &McpConnectionDetails) -> &str {
    snapshot.instructions.get("proxy").expect("Expected proxy instructions")
}

fn extract_tool_dir(instructions: &str) -> Option<String> {
    let start = instructions.find('`')? + 1;
    let end = instructions[start..].find('`')? + start;
    Some(instructions[start..end].to_string())
}

#[tokio::test]
async fn test_tool_proxy_exposes_only_call_tool() {
    let test = math_proxy().await;
    let snapshot = test.snapshot();

    assert_eq!(snapshot.tool_definitions.len(), 1);
    assert_eq!(snapshot.tool_definitions[0].name, "proxy__call_tool");
    assert!(snapshot.tool_definitions[0].description.contains("Execute a tool on a nested MCP server"));
}

#[tokio::test]
async fn test_tool_proxy_instructions_mention_tool_directory() {
    let test = math_proxy().await;
    let proxy_instr = proxy_instructions(test.snapshot());

    assert!(proxy_instr.contains("tool-proxy"), "Instructions should mention tool-proxy directory: {proxy_instr}");
    assert!(proxy_instr.contains("call_tool"), "Instructions should mention call_tool: {proxy_instr}");
    assert!(proxy_instr.contains("## Connected Servers"));
    assert!(proxy_instr.contains("**math**"));
}

#[tokio::test]
async fn test_tool_proxy_does_not_expose_nested_server_tools() {
    let test = math_proxy().await;
    let snapshot = test.snapshot();

    assert_eq!(snapshot.tool_definitions.len(), 1);
    assert_eq!(snapshot.tool_definitions[0].name, "proxy__call_tool");
}

#[tokio::test]
async fn test_tool_proxy_does_not_leak_nested_instructions() {
    let test = math_proxy().await;
    let snapshot = test.snapshot();

    assert!(!snapshot.instructions.contains_key("math"));
    assert!(snapshot.instructions.contains_key("proxy"));
}

#[tokio::test]
async fn test_tool_proxy_writes_tool_files_to_disk() {
    let test = math_proxy().await;
    let tool_dir = extract_tool_dir(proxy_instructions(test.snapshot())).expect("tool directory is listed");
    let tool_dir = std::path::Path::new(&tool_dir);

    let math_dir = tool_dir.join("math");
    assert!(math_dir.join("add_numbers.json").exists());
    assert!(math_dir.join("divide_numbers.json").exists());
    assert!(math_dir.join("slow_tool.json").exists());

    let content = std::fs::read_to_string(math_dir.join("add_numbers.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["name"], "add_numbers");
    assert_eq!(parsed["server"], "math");
    assert!(parsed["description"].as_str().unwrap().contains("Adds two numbers"));
}

#[tokio::test]
async fn test_tool_proxy_call_tool_routes_to_nested_server() {
    let test = math_proxy().await;

    let result = call_proxy_tool(&test, "math", "add_numbers", serde_json::json!({"a": 3, "b": 4})).await;

    assert!(result.unwrap().contains('7'));
}

#[tokio::test]
async fn test_tool_proxy_call_tool_unknown_server_returns_error() {
    let test = math_proxy().await;

    let error = call_proxy_tool(&test, "nonexistent", "some_tool", serde_json::json!({}))
        .await
        .expect_err("unknown server fails");

    assert!(error.contains("nonexistent") || error.contains("not part of proxy") || error.contains("not connected"));
}

#[tokio::test]
async fn test_tool_proxy_multiple_nested_servers() {
    let test = McpTestBuilder::new()
        .proxy_server("server_a", FakeMcpServer::new())
        .proxy_server("server_b", FakeMcpServer::new())
        .build()
        .await;
    let snapshot = test.snapshot();

    assert_eq!(snapshot.tool_definitions.len(), 1);
    let tool_dir = extract_tool_dir(proxy_instructions(snapshot)).expect("tool directory is listed");
    let tool_dir = std::path::Path::new(&tool_dir);
    assert!(tool_dir.join("server_a/add_numbers.json").exists());
    assert!(tool_dir.join("server_b/add_numbers.json").exists());
}

#[tokio::test]
async fn test_tool_proxy_member_server_status_shows_connected_and_proxied() {
    let test = math_proxy().await;
    let snapshot = test.snapshot();

    assert!(!snapshot.server_statuses.iter().any(|status| status.name == "proxy"));
    let math = snapshot.server_statuses.iter().find(|status| status.name == "math").expect("math status exists");
    assert!(matches!(math.status, mcp_utils::status::McpServerStatus::Connected { .. }));
    assert!(math.proxied);
}

async fn selective_math_proxy() -> McpTest {
    McpTestBuilder::new()
        .server_with_exposure("math", FakeMcpServer::new(), ToolExposure::Proxied(ToolProxyRules::new(&[], &["add_*"])))
        .build()
        .await
}

#[tokio::test]
async fn include_only_proxy_server_instructions_cover_its_direct_tools() {
    let test = McpTestBuilder::new()
        .server_with_exposure(
            "math",
            FakeMcpServer::new(),
            ToolExposure::Proxied(ToolProxyRules::new(&["divide_*"], &[])),
        )
        .build()
        .await;

    assert!(test.snapshot().instructions.contains_key("math"));
    assert!(test.snapshot().instructions.contains_key("proxy"));
}

#[tokio::test]
async fn proxy_rules_with_no_direct_tools_do_not_emit_server_instructions() {
    let test = McpTestBuilder::new()
        .server_with_exposure("math", FakeMcpServer::new(), ToolExposure::Proxied(ToolProxyRules::new(&["*"], &[])))
        .build()
        .await;

    assert!(!test.snapshot().instructions.contains_key("math"));
    assert!(test.snapshot().instructions.contains_key("proxy"));
}

#[tokio::test]
async fn selectively_proxied_server_instructions_cover_its_direct_tools() {
    let test = selective_math_proxy().await;

    assert!(test.snapshot().instructions.contains_key("math"));
    assert!(test.snapshot().instructions.contains_key("proxy"));
}

#[tokio::test]
async fn proxy_forwards_tools_added_after_initial_discovery() {
    let server = FakeMcpServer::new();
    let state = server.state();
    let test = McpTestBuilder::new().proxy_server("dynamic", server).build().await;
    state.add_tool(FakeTool::new("added_later").responds(FakeToolResponse::text("available")));

    let result = call_proxy_tool(&test, "dynamic", "added_later", serde_json::json!({})).await;
    assert!(result.unwrap().contains("available"));
}

#[tokio::test]
async fn mixed_direct_full_proxy_and_selective_servers_have_stable_definitions_and_statuses() {
    let test = McpTestBuilder::new()
        .server("direct", FakeMcpServer::new())
        .proxy_server("hidden", FakeMcpServer::new())
        .server_with_exposure(
            "selective",
            FakeMcpServer::new(),
            ToolExposure::Proxied(ToolProxyRules::new(&[], &["add_*"])),
        )
        .build()
        .await;

    let snapshot = test.snapshot();
    let names = snapshot.tool_definitions.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "proxy__call_tool",
            "direct__add_numbers",
            "direct__divide_numbers",
            "direct__slow_tool",
            "selective__add_numbers"
        ]
    );
    assert_eq!(
        snapshot.server_statuses.iter().map(|status| (status.name.as_str(), status.proxied)).collect::<Vec<_>>(),
        [("direct", false), ("hidden", true), ("selective", true)]
    );
}

#[tokio::test]
async fn selectively_proxied_tools_are_partitioned_between_direct_and_proxy_routes() {
    let test = selective_math_proxy().await;
    let names: Vec<&str> = test.snapshot().tool_definitions.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["proxy__call_tool", "math__add_numbers"]);

    let direct_tool = test
        .snapshot()
        .tool_definitions
        .iter()
        .find(|tool| tool.name == "math__add_numbers")
        .expect("selected tool is exposed directly");
    assert!(direct_tool.description.contains("Adds two numbers"));
    assert_eq!(direct_tool.server.as_deref(), Some("math"));
    assert_eq!(direct_tool.parameters, serde_json::json!({"type": "object", "properties": {}}));

    let direct = test.call("math", "add_numbers", serde_json::json!({"a": 3, "b": 4})).await;
    assert!(direct.result.unwrap().result.contains('7'));

    let direct_proxied = test.call("math", "divide_numbers", serde_json::json!({"a": 8, "b": 2})).await;
    let direct_error = direct_proxied.result.expect_err("proxy-only tool has no direct route");
    assert!(direct_error.error.contains("Tool not found: math__divide_numbers"));

    assert!(
        call_proxy_tool(&test, "math", "divide_numbers", serde_json::json!({"a": 8, "b": 2}))
            .await
            .unwrap()
            .contains('4')
    );
    let direct_route_error = call_proxy_tool(&test, "math", "add_numbers", serde_json::json!({"a": 3, "b": 4}))
        .await
        .expect_err("direct tool is rejected by the proxy route");
    assert!(direct_route_error.contains("call 'math__add_numbers' instead"));
    assert!(call_proxy_tool(&test, "math", "unknown", serde_json::json!({})).await.is_err());
}

#[tokio::test]
async fn proxy_discovery_write_failure_does_not_disconnect_server() {
    let home = tempfile::tempdir().unwrap();
    let proxy_dir = home.path().join("tool-proxy/proxy");
    std::fs::create_dir_all(home.path().join("tool-proxy")).unwrap();
    let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(50);
    let mut manager = McpManager::new(event_tx, None).with_aether_home(home.path());
    let server = fake_mcp("math", FakeMcpServer::new()).with_exposure(ToolExposure::proxied_all());

    let mut pending = manager.register_pending(vec![server]).await.unwrap();
    if proxy_dir.exists() {
        std::fs::remove_dir_all(&proxy_dir).unwrap();
    }
    std::fs::write(&proxy_dir, "not a directory").unwrap();

    let attempt = manager.connect_pending_task(pending.pop().unwrap()).await;
    manager.apply_connection_attempt(attempt).await;

    let status = manager.server_statuses().into_iter().find(|status| status.name == "math").unwrap();
    assert!(matches!(status.status, mcp_utils::status::McpServerStatus::Connected { .. }));
    assert_eq!(manager.tool_definitions()[0].name, "proxy__call_tool");
}

#[tokio::test]
async fn selectively_proxied_tools_are_omitted_from_proxy_discovery_files() {
    let test = selective_math_proxy().await;
    let tool_dir = extract_tool_dir(proxy_instructions(test.snapshot())).expect("tool directory is listed");
    let math_dir = std::path::Path::new(&tool_dir).join("math");

    assert!(!math_dir.join("add_numbers.json").exists());
    assert!(math_dir.join("divide_numbers.json").exists());
    assert!(math_dir.join("slow_tool.json").exists());
}
