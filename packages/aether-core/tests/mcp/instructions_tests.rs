use aether_core::core::Prompt;
use aether_core::events::{AgentMessage, UserMessage};
use aether_core::mcp::mcp;
use aether_core::testing::{FakeMcpServer, fake_mcp, mcp_instructions as instructions};
use llm::testing::FakeLlmProvider;

#[tokio::test]
async fn test_fake_mcp_server_has_instructions() {
    // FakeMcpServer has instructions set to "A fake MCP server for testing"
    let mut spawn = mcp().with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await.unwrap();
    let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");

    // FakeMcpServer does provide instructions, so we should get them
    assert_eq!(snapshot.instructions.len(), 1);
    assert!(snapshot.instructions.get("test").unwrap().contains("A fake MCP server for testing"));
}

#[tokio::test]
async fn test_multiple_servers_with_instructions() {
    let mut spawn = mcp()
        .with_servers(vec![fake_mcp("server1", FakeMcpServer::new()), fake_mcp("server2", FakeMcpServer::new())])
        .spawn()
        .await
        .unwrap();
    let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");

    // Both servers should have instructions
    assert_eq!(snapshot.instructions.len(), 2);
    assert!(snapshot.instructions.get("server1").unwrap().contains("A fake MCP server for testing"));
    assert!(snapshot.instructions.get("server2").unwrap().contains("A fake MCP server for testing"));
}

#[tokio::test]
async fn test_format_mcp_instructions_xml_structure() {
    let formatted = Prompt::McpInstructions(instructions(&[("coding", "Use absolute paths.")])).build().await.unwrap();

    // Check for XML tags with server names
    assert!(formatted.contains("<mcp-server name=\"coding\">"));
    assert!(formatted.contains("</mcp-server>\n"));
    assert!(formatted.contains("Use absolute paths."));
    assert!(formatted.contains("# MCP Server Instructions"));
}

#[tokio::test]
async fn test_format_mcp_instructions_multiple_servers() {
    let formatted =
        Prompt::McpInstructions(instructions(&[("coding", "Use absolute paths."), ("plugins", "Always confirm.")]))
            .build()
            .await
            .unwrap();

    // Check for XML tags with both server names
    assert!(formatted.contains("<mcp-server name=\"coding\">"));
    assert!(formatted.contains("<mcp-server name=\"plugins\">"));
    assert!(formatted.contains("Use absolute paths."));
    assert!(formatted.contains("Always confirm."));
}

#[tokio::test]
async fn test_format_mcp_instructions_is_deterministically_ordered() {
    let a = Prompt::McpInstructions(instructions(&[("zebra", "Z"), ("alpha", "A")])).build().await.unwrap();
    let b = Prompt::McpInstructions(instructions(&[("alpha", "A"), ("zebra", "Z")])).build().await.unwrap();
    assert_eq!(a, b);
    let alpha_pos = a.find("name=\"alpha\"").unwrap();
    let zebra_pos = a.find("name=\"zebra\"").unwrap();
    assert!(alpha_pos < zebra_pos, "alphabetical order: alpha before zebra");
}

#[tokio::test]
async fn test_agent_builder_includes_mcp_instructions_in_system_prompt() {
    use aether_core::core::agent;

    let llm = FakeLlmProvider::new(vec![]);
    let captured_contexts = llm.captured_contexts();

    let mut spawn = mcp().with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await.unwrap();
    let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");

    let (tx, mut rx, _handle) = agent(llm)
        .system_prompt(Prompt::text("You are a test agent"))
        .system_prompt(Prompt::McpInstructions(instructions(&[("test-server", "Test instructions")])))
        .tools(spawn.command_tx, snapshot.tool_definitions)
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

    let mut spawn = mcp().with_servers(vec![fake_mcp("test", FakeMcpServer::new())]).spawn().await.unwrap();
    let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");

    // No mcp_instructions provided - should still work
    let (tx, mut rx, _handle) = agent(llm)
        .system_prompt(Prompt::text("You are a test agent"))
        .tools(spawn.command_tx, snapshot.tool_definitions)
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
