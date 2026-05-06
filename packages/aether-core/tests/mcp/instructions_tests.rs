use aether_core::core::Prompt;
use aether_core::events::{AgentMessage, UserMessage};
use aether_core::mcp::{McpSpawnResult, mcp};
use aether_core::testing::{FakeMcpServer, fake_mcp};
use llm::testing::FakeLlmProvider;
use mcp_utils::client::ServerInstructions;

#[tokio::test]
async fn test_fake_mcp_server_has_instructions() {
    // FakeMcpServer has instructions set to "A fake MCP server for testing"
    let McpSpawnResult {
        tool_definitions: _,
        instructions,
        server_statuses: _,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await.unwrap();

    // FakeMcpServer does provide instructions, so we should get them
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].server_name, "test");
    assert!(instructions[0].instructions.contains("A fake MCP server for testing"));
}

#[tokio::test]
async fn test_multiple_servers_with_instructions() {
    let McpSpawnResult {
        tool_definitions: _,
        instructions,
        server_statuses: _,
        command_tx: _,
        event_rx: _,
        handle: _,
        ..
    } = mcp()
        .with_servers(vec![fake_mcp("server1", FakeMcpServer::new()), fake_mcp("server2", FakeMcpServer::new())])
        .spawn()
        .await
        .unwrap();

    // Both servers should have instructions
    assert_eq!(instructions.len(), 2);

    // Find server1 and server2 instructions
    let server1_instr = instructions.iter().find(|i| i.server_name == "server1").unwrap();
    let server2_instr = instructions.iter().find(|i| i.server_name == "server2").unwrap();

    assert!(server1_instr.instructions.contains("A fake MCP server for testing"));
    assert!(server2_instr.instructions.contains("A fake MCP server for testing"));
}

#[tokio::test]
async fn test_format_mcp_instructions_xml_structure() {
    let instructions =
        vec![ServerInstructions { server_name: "coding".to_string(), instructions: "Use absolute paths.".to_string() }];

    let formatted = Prompt::mcp_instructions(instructions).build().await.unwrap();

    // Check for XML tags with server names
    assert!(formatted.contains("<mcp-server name=\"coding\">"));
    assert!(formatted.contains("</mcp-server>\n"));
    assert!(formatted.contains("Use absolute paths."));
    assert!(formatted.contains("# MCP Server Instructions"));
}

#[tokio::test]
async fn test_format_mcp_instructions_multiple_servers() {
    let instructions = vec![
        ServerInstructions { server_name: "coding".to_string(), instructions: "Use absolute paths.".to_string() },
        ServerInstructions {
            server_name: "plugins".to_string(),
            instructions: "Always confirm before spawning.".to_string(),
        },
    ];

    let formatted = Prompt::mcp_instructions(instructions).build().await.unwrap();

    // Check for XML tags with both server names
    assert!(formatted.contains("<mcp-server name=\"coding\">"));
    assert!(formatted.contains("<mcp-server name=\"plugins\">"));
    assert!(formatted.contains("Use absolute paths."));
    assert!(formatted.contains("Always confirm before spawning."));
}

#[tokio::test]
async fn test_agent_builder_includes_mcp_instructions_in_system_prompt() {
    use aether_core::core::agent;

    let instructions = vec![ServerInstructions {
        server_name: "test-server".to_string(),
        instructions: "Test instructions".to_string(),
    }];

    let llm = FakeLlmProvider::new(vec![]);
    let captured_contexts = llm.captured_contexts();

    let McpSpawnResult {
        tool_definitions,
        instructions: _,
        server_statuses: _,
        command_tx: mcp_tx,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await.unwrap();

    let (tx, mut rx, _handle) = agent(llm)
        .system_prompt(Prompt::text("You are a test agent"))
        .system_prompt(Prompt::mcp_instructions(instructions))
        .tools(mcp_tx, tool_definitions)
        .spawn()
        .await
        .unwrap();

    // Send a simple message to trigger context capture
    tx.send(UserMessage::text("test")).await.unwrap();
    drop(tx);

    // Wait for the agent to process
    while let Some(msg) = rx.recv().await {
        if matches!(msg, AgentMessage::Done) {
            break;
        }
    }

    // Check that the captured context includes our MCP instructions
    let contexts = captured_contexts.lock().unwrap();
    assert!(!contexts.is_empty());

    // The system message should contain our MCP instructions
    if let Some(first_msg) = contexts[0].messages().first() {
        if let llm::ChatMessage::System { content, .. } = first_msg {
            assert!(content.contains("<mcp-server name=\"test-server\">"));
            assert!(content.contains("Test instructions"));
        } else {
            panic!("Expected system message, got: {first_msg:?}");
        }
    } else {
        panic!("Expected at least one message");
    }
}

#[tokio::test]
async fn test_agent_builder_works_without_mcp_instructions() {
    use aether_core::core::agent;

    let llm = FakeLlmProvider::new(vec![]);
    let captured_contexts = llm.captured_contexts();

    let McpSpawnResult {
        tool_definitions,
        instructions: _,
        server_statuses: _,
        command_tx: mcp_tx,
        event_rx: _,
        handle: _,
        ..
    } = mcp().with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await.unwrap();

    // No mcp_instructions provided - should still work
    let (tx, mut rx, _handle) = agent(llm)
        .system_prompt(Prompt::text("You are a test agent"))
        .tools(mcp_tx, tool_definitions)
        .spawn()
        .await
        .unwrap();

    // Send a simple message
    tx.send(UserMessage::text("test")).await.unwrap();
    drop(tx);

    // Wait for the agent to process
    while let Some(msg) = rx.recv().await {
        if matches!(msg, AgentMessage::Done) {
            break;
        }
    }

    // Should have captured context without any MCP instructions section
    let contexts = captured_contexts.lock().unwrap();
    assert!(!contexts.is_empty());
}
