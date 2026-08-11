use aether_core::testing::{FakeMcpServer, McpTest, McpTestBuilder};
use mcp_utils::client::McpConnectionDetails;

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
