use aether_core::mcp::{McpSpawnResult, mcp};
use aether_core::testing::{FakeMcpServer, fake_mcp_with_proxy};
use tokio::sync::mpsc;

#[tokio::test]
async fn test_tool_proxy_exposes_only_call_tool() {
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)];

    let McpSpawnResult {
        tool_definitions,
        instructions: _,
        server_statuses: _,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    // The proxy should expose exactly one tool: proxy__call_tool
    assert_eq!(tool_definitions.len(), 1);
    assert_eq!(tool_definitions[0].name, "proxy__call_tool");
    assert!(tool_definitions[0].description.contains("Execute a tool on a nested MCP server"));
}

#[tokio::test]
async fn test_tool_proxy_instructions_mention_tool_directory() {
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)];

    let McpSpawnResult {
        tool_definitions: _,
        instructions,
        server_statuses: _,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    let proxy_instr =
        instructions.iter().find(|i| i.server_name == "proxy").expect("Expected instructions from tool-proxy 'ext'");

    assert!(
        proxy_instr.instructions.contains("tool-proxy"),
        "Instructions should mention tool-proxy directory: {}",
        proxy_instr.instructions
    );
    assert!(
        proxy_instr.instructions.contains("call_tool"),
        "Instructions should mention call_tool: {}",
        proxy_instr.instructions
    );
    assert!(
        proxy_instr.instructions.contains("## Connected Servers"),
        "Instructions should contain Connected Servers section: {}",
        proxy_instr.instructions
    );
    assert!(
        proxy_instr.instructions.contains("**math**"),
        "Instructions should list the 'math' server: {}",
        proxy_instr.instructions
    );
}

#[tokio::test]
async fn test_tool_proxy_does_not_expose_nested_server_tools() {
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)];

    let McpSpawnResult {
        tool_definitions,
        instructions: _,
        server_statuses: _,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    // The agent should NOT see individual tools like math__add_numbers
    for td in &tool_definitions {
        assert!(!td.name.contains("add_numbers"), "Nested tool should not be exposed: {}", td.name);
        assert!(!td.name.contains("divide_numbers"), "Nested tool should not be exposed: {}", td.name);
    }

    // Only the proxy's call_tool
    assert_eq!(tool_definitions.len(), 1);
    assert_eq!(tool_definitions[0].name, "proxy__call_tool");
}

#[tokio::test]
async fn test_tool_proxy_does_not_leak_nested_instructions() {
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)];

    let McpSpawnResult {
        tool_definitions: _,
        instructions,
        server_statuses: _,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    // Nested server instructions should NOT appear as top-level entries
    assert!(
        !instructions.iter().any(|i| i.server_name == "math"),
        "Nested server 'math' should not have its own instructions entry"
    );

    // Only the proxy should have instructions
    assert!(instructions.iter().any(|i| i.server_name == "proxy"), "Proxy 'noleak' should have instructions");
}

#[tokio::test]
async fn test_tool_proxy_writes_tool_files_to_disk() {
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)];

    let McpSpawnResult {
        tool_definitions: _,
        instructions,
        server_statuses: _,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    let proxy_instr = instructions
        .iter()
        .find(|i| i.server_name == "proxy")
        .expect("Expected instructions from tool-proxy 'filetest'");

    let tool_dir = extract_tool_dir(&proxy_instr.instructions).expect("Should find tool directory in instructions");
    let tool_dir = std::path::Path::new(&tool_dir);
    assert!(tool_dir.exists(), "Tool directory should exist: {tool_dir:?}");

    // Should have a "math" subdirectory
    let math_dir = tool_dir.join("math");
    assert!(math_dir.exists(), "math server directory should exist");

    // FakeMcpServer has add_numbers, divide_numbers, slow_tool
    let add_file = math_dir.join("add_numbers.json");
    assert!(add_file.exists(), "add_numbers.json should exist");

    let divide_file = math_dir.join("divide_numbers.json");
    assert!(divide_file.exists(), "divide_numbers.json should exist");

    let slow_file = math_dir.join("slow_tool.json");
    assert!(slow_file.exists(), "slow_tool.json should exist");

    // Verify the JSON content is valid
    let content = std::fs::read_to_string(&add_file).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["name"], "add_numbers");
    assert_eq!(parsed["server"], "math");
    assert!(parsed["description"].as_str().unwrap().contains("Adds two numbers"));

    // Cleanup
    let _ = std::fs::remove_dir_all(tool_dir);
}

#[tokio::test]
async fn test_tool_proxy_call_tool_routes_to_nested_server() {
    use aether_core::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
    use std::time::Duration;
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)];

    let McpSpawnResult {
        tool_definitions: _,
        instructions,
        server_statuses: _,
        command_tx,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    // Call add_numbers through the proxy using ExecuteTool
    let arguments = serde_json::json!({
        "server": "math",
        "tool": "add_numbers",
        "arguments": {"a": 3, "b": 4}
    })
    .to_string();

    let request =
        llm::ToolCallRequest { id: "test_call_1".to_string(), name: "proxy__call_tool".to_string(), arguments };

    let (event_tx, mut event_rx) = mpsc::channel(10);
    command_tx.send(McpCommand::ExecuteTool { request, timeout: Duration::from_secs(10), tx: event_tx }).await.unwrap();

    // Collect events until we get Complete
    let mut result_text = String::new();
    while let Some(event) = event_rx.recv().await {
        if let ToolExecutionEvent::Complete { result, .. } = event {
            match result {
                Ok(tool_result) => result_text = tool_result.result,
                Err(e) => panic!("Tool execution failed: {}", e.error),
            }
            break;
        }
    }

    // The result should contain the sum (7)
    assert!(result_text.contains('7'), "Expected result to contain sum of 3+4=7, got: {result_text}");

    // Cleanup
    cleanup_tool_dir(&instructions, "proxy");
}

#[tokio::test]
async fn test_tool_proxy_call_tool_unknown_server_returns_error() {
    use aether_core::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
    use std::time::Duration;
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)];

    let McpSpawnResult {
        tool_definitions: _,
        instructions,
        server_statuses: _,
        command_tx,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    let arguments = serde_json::json!({
        "server": "nonexistent",
        "tool": "some_tool",
        "arguments": {}
    })
    .to_string();

    let request =
        llm::ToolCallRequest { id: "test_call_2".to_string(), name: "proxy__call_tool".to_string(), arguments };

    let (event_tx, mut event_rx) = mpsc::channel(10);
    command_tx.send(McpCommand::ExecuteTool { request, timeout: Duration::from_secs(10), tx: event_tx }).await.unwrap();

    while let Some(event) = event_rx.recv().await {
        if let ToolExecutionEvent::Complete { result, .. } = event {
            // Should be an error about unknown server
            match result {
                Err(e) => {
                    assert!(
                        e.error.contains("nonexistent")
                            || e.error.contains("not part of proxy")
                            || e.error.contains("not connected"),
                        "Expected error mentioning unknown server, got: {}",
                        e.error
                    );
                }
                Ok(r) => {
                    panic!("Expected error for unknown server, got success: {}", r.result);
                }
            }
            break;
        }
    }

    cleanup_tool_dir(&instructions, "proxy");
}

#[tokio::test]
async fn test_tool_proxy_multiple_nested_servers() {
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![
        fake_mcp_with_proxy("server_a", FakeMcpServer::new(), true),
        fake_mcp_with_proxy("server_b", FakeMcpServer::new(), true),
    ];

    let McpSpawnResult {
        tool_definitions,
        instructions,
        server_statuses: _,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    // Still only one tool exposed
    assert_eq!(tool_definitions.len(), 1);
    assert_eq!(tool_definitions[0].name, "proxy__call_tool");

    // Verify both server directories exist
    let proxy_instr = instructions.iter().find(|i| i.server_name == "proxy").expect("Expected instructions");

    let tool_dir = extract_tool_dir(&proxy_instr.instructions).expect("Should find tool directory");
    let tool_dir = std::path::Path::new(&tool_dir);

    assert!(tool_dir.join("server_a").exists());
    assert!(tool_dir.join("server_b").exists());

    // Both should have the same tools (both are FakeMcpServer)
    assert!(tool_dir.join("server_a/add_numbers.json").exists());
    assert!(tool_dir.join("server_b/add_numbers.json").exists());

    let _ = std::fs::remove_dir_all(tool_dir);
}

#[tokio::test]
async fn test_tool_proxy_server_status_shows_connected() {
    let aether_home = tempfile::tempdir().unwrap();
    let servers = vec![fake_mcp_with_proxy("math", FakeMcpServer::new(), true)];

    let McpSpawnResult {
        tool_definitions: _,
        instructions,
        server_statuses,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_aether_home(aether_home.path()).with_servers(servers).spawn().await.unwrap();

    // The proxy itself should show as Connected with tool_count=1 (just call_tool)
    let proxy_status = server_statuses.iter().find(|s| s.name == "proxy").expect("Expected proxy status entry");

    assert!(
        matches!(proxy_status.status, mcp_utils::status::McpServerStatus::Connected { tool_count: 1 }),
        "Expected Connected with 1 tool, got: {:?}",
        proxy_status.status
    );

    cleanup_tool_dir(&instructions, "proxy");
}

/// Extract the tool directory path from proxy instructions.
/// The instructions contain the path in backticks: `<path>`
fn extract_tool_dir(instructions: &str) -> Option<String> {
    let start = instructions.find('`')? + 1;
    let end = instructions[start..].find('`')? + start;
    Some(instructions[start..end].to_string())
}

fn cleanup_tool_dir(instructions: &[mcp_utils::client::ServerInstructions], proxy_name: &str) {
    if let Some(instr) = instructions.iter().find(|i| i.server_name == proxy_name)
        && let Some(tool_dir) = extract_tool_dir(&instr.instructions)
    {
        let _ = std::fs::remove_dir_all(tool_dir);
    }
}
