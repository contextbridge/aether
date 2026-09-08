use aether_core::core::Prompt;
use aether_core::testing::{FakeMcpServer, McpTestBuilder, mcp_instructions as instructions, test_agent};
use llm::ChatMessage;
use llm::testing::llm_response;
use std::error::Error;

#[tokio::test]
async fn test_fake_mcp_server_has_instructions() {
    let mcp_test = McpTestBuilder::new().server("test", FakeMcpServer::new()).build().await;
    let instructions = mcp_test.snapshot().model_instructions();

    // FakeMcpServer does provide instructions, so we should get them
    assert_eq!(instructions.len(), 1);
    assert!(instructions.get("test").unwrap().contains("A fake MCP server for testing"));
}

#[tokio::test]
async fn test_multiple_servers_with_instructions() {
    let mcp_test = McpTestBuilder::new()
        .server("server1", FakeMcpServer::new())
        .server("server2", FakeMcpServer::new())
        .build()
        .await;
    let instructions = mcp_test.snapshot().model_instructions();

    // Both servers should have instructions
    assert_eq!(instructions.len(), 2);
    assert!(instructions.get("server1").unwrap().contains("A fake MCP server for testing"));
    assert!(instructions.get("server2").unwrap().contains("A fake MCP server for testing"));
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
async fn test_agent_builder_includes_mcp_instructions_in_system_prompt() -> Result<(), Box<dyn Error>> {
    let result = test_agent()
        .llm_responses(&[llm_response("message_1").text(&["done"]).build()])
        .system_prompt(Prompt::text("You are a test agent"))
        .system_prompt(Prompt::McpInstructions(instructions(&[("test-server", "Test instructions")])))
        .user_text("test")
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    let first_message = contexts[0].messages().first().expect("expected at least one message");
    let ChatMessage::System { content, .. } = first_message else {
        panic!("Expected system message, got: {first_message:?}");
    };
    assert!(content.contains("<mcp-server name=\"test-server\">"));
    assert!(content.contains("Test instructions"));
    Ok(())
}

#[tokio::test]
async fn test_agent_builder_works_without_mcp_instructions() -> Result<(), Box<dyn Error>> {
    let result = test_agent()
        .llm_responses(&[llm_response("message_1").text(&["done"]).build()])
        .system_prompt(Prompt::text("You are a test agent"))
        .user_text("test")
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    let first_message = contexts[0].messages().first().expect("expected at least one message");
    let ChatMessage::System { content, .. } = first_message else {
        panic!("Expected system message, got: {first_message:?}");
    };
    assert!(content.contains("You are a test agent"));
    assert!(!content.contains("<mcp-server"), "no MCP instructions section should be added: {content}");
    Ok(())
}
