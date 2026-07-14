use aether_core::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use aether_core::mcp::{McpSpawnResult, mcp};
use aether_core::testing::{FakeMcpServer, fake_mcp_with_proxy};
use mcp_utils::client::McpConnectionDetails;
use std::time::Duration;
use tokio::sync::mpsc;

struct ProxyFixture {
    _home: tempfile::TempDir,
    spawn: McpSpawnResult,
    snapshot: McpConnectionDetails,
}

async fn spawn_proxy(servers: Vec<mcp_utils::client::McpServer>) -> ProxyFixture {
    let aether_home = tempfile::tempdir().unwrap();
    let mut spawn = mcp("/workspace").with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();
    let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");
    ProxyFixture { _home: aether_home, spawn, snapshot }
}

async fn call_proxy_tool(
    spawn: &McpSpawnResult,
    server: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<String, String> {
    let arguments = serde_json::json!({ "server": server, "tool": tool, "arguments": arguments }).to_string();
    let request = llm::ToolCallRequest { id: "test_call".to_string(), name: "proxy__call_tool".to_string(), arguments };
    let (event_tx, mut event_rx) = mpsc::channel(10);
    spawn
        .command_tx
        .send(McpCommand::ExecuteTool { request, timeout: Duration::from_secs(10), tx: event_tx })
        .await
        .unwrap();

    while let Some(event) = event_rx.recv().await {
        if let ToolExecutionEvent::Complete { result, .. } = event {
            return result.map(|r| r.result).map_err(|e| e.error);
        }
    }
    panic!("event stream ended without Complete");
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
    let ProxyFixture { _home, snapshot, .. } =
        spawn_proxy(vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)]).await;

    assert_eq!(snapshot.tool_definitions.len(), 1);
    assert_eq!(snapshot.tool_definitions[0].name, "proxy__call_tool");
    assert!(snapshot.tool_definitions[0].description.contains("Execute a tool on a nested MCP server"));
}

#[tokio::test]
async fn test_tool_proxy_instructions_mention_tool_directory() {
    let ProxyFixture { _home, snapshot, .. } =
        spawn_proxy(vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)]).await;
    let proxy_instr = proxy_instructions(&snapshot);

    assert!(proxy_instr.contains("tool-proxy"), "Instructions should mention tool-proxy directory: {proxy_instr}");
    assert!(proxy_instr.contains("call_tool"), "Instructions should mention call_tool: {proxy_instr}");
    assert!(
        proxy_instr.contains("## Connected Servers"),
        "Instructions should contain Connected Servers section: {proxy_instr}"
    );
    assert!(proxy_instr.contains("**math**"), "Instructions should list the 'math' server: {proxy_instr}");
}

#[tokio::test]
async fn test_tool_proxy_does_not_expose_nested_server_tools() {
    let ProxyFixture { _home, snapshot, .. } =
        spawn_proxy(vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)]).await;

    for td in &snapshot.tool_definitions {
        assert!(!td.name.contains("add_numbers"), "Nested tool should not be exposed: {}", td.name);
        assert!(!td.name.contains("divide_numbers"), "Nested tool should not be exposed: {}", td.name);
    }
    assert_eq!(snapshot.tool_definitions.len(), 1);
    assert_eq!(snapshot.tool_definitions[0].name, "proxy__call_tool");
}

#[tokio::test]
async fn test_tool_proxy_does_not_leak_nested_instructions() {
    let ProxyFixture { _home, snapshot, .. } =
        spawn_proxy(vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)]).await;

    assert!(
        !snapshot.instructions.contains_key("math"),
        "Nested server 'math' should not have its own instructions entry"
    );
    assert!(snapshot.instructions.contains_key("proxy"), "Proxy should have instructions");
}

#[tokio::test]
async fn test_tool_proxy_writes_tool_files_to_disk() {
    let ProxyFixture { _home, snapshot, .. } =
        spawn_proxy(vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)]).await;

    let tool_dir = extract_tool_dir(proxy_instructions(&snapshot)).expect("Should find tool directory");
    let tool_dir = std::path::Path::new(&tool_dir);
    assert!(tool_dir.exists(), "Tool directory should exist: {tool_dir:?}");

    let math_dir = tool_dir.join("math");
    assert!(math_dir.exists(), "math server directory should exist");
    assert!(math_dir.join("add_numbers.json").exists(), "add_numbers.json should exist");
    assert!(math_dir.join("divide_numbers.json").exists(), "divide_numbers.json should exist");
    assert!(math_dir.join("slow_tool.json").exists(), "slow_tool.json should exist");

    let content = std::fs::read_to_string(math_dir.join("add_numbers.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["name"], "add_numbers");
    assert_eq!(parsed["server"], "math");
    assert!(parsed["description"].as_str().unwrap().contains("Adds two numbers"));
}

#[tokio::test]
async fn test_tool_proxy_call_tool_routes_to_nested_server() {
    let ProxyFixture { _home, spawn, snapshot: _snapshot } =
        spawn_proxy(vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)]).await;

    let result = call_proxy_tool(&spawn, "math", "add_numbers", serde_json::json!({"a": 3, "b": 4})).await;
    assert!(result.unwrap().contains('7'), "Expected result to contain sum of 3+4=7");
}

#[tokio::test]
async fn test_tool_proxy_call_tool_unknown_server_returns_error() {
    let ProxyFixture { _home, spawn, snapshot: _snapshot } =
        spawn_proxy(vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)]).await;

    let err = call_proxy_tool(&spawn, "nonexistent", "some_tool", serde_json::json!({}))
        .await
        .expect_err("Expected error for unknown server");
    assert!(
        err.contains("nonexistent") || err.contains("not part of proxy") || err.contains("not connected"),
        "Expected error mentioning unknown server, got: {err}"
    );
}

#[tokio::test]
async fn test_tool_proxy_multiple_nested_servers() {
    let ProxyFixture { _home, snapshot, .. } = spawn_proxy(vec![
        fake_mcp_with_proxy("server_a", FakeMcpServer::new(), true),
        fake_mcp_with_proxy("server_b", FakeMcpServer::new(), true),
    ])
    .await;

    assert_eq!(snapshot.tool_definitions.len(), 1);
    assert_eq!(snapshot.tool_definitions[0].name, "proxy__call_tool");

    let tool_dir = extract_tool_dir(proxy_instructions(&snapshot)).expect("Should find tool directory");
    let tool_dir = std::path::Path::new(&tool_dir);

    assert!(tool_dir.join("server_a").exists());
    assert!(tool_dir.join("server_b").exists());
    assert!(tool_dir.join("server_a/add_numbers.json").exists());
    assert!(tool_dir.join("server_b/add_numbers.json").exists());
}

#[tokio::test]
async fn test_tool_proxy_member_server_status_shows_connected_and_proxied() {
    let ProxyFixture { _home, snapshot, .. } =
        spawn_proxy(vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)]).await;

    assert!(!snapshot.server_statuses.iter().any(|s| s.name == "proxy"));

    let math_status = snapshot.server_statuses.iter().find(|s| s.name == "math").expect("Expected 'math' status entry");
    assert!(matches!(math_status.status, mcp_utils::status::McpServerStatus::Connected { .. }));
    assert!(math_status.proxied);
}
