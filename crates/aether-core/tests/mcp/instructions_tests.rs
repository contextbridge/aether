use aether_core::core::{McpInstructions, Prompt};
use aether_core::mcp::mcp;
use aether_core::testing::{mcp_instructions, ready_catalog, test_agent};
use llm::ChatMessage;
use llm::testing::llm_response;
use mcp_utils::gateway::ServerSpec;
use mcp_utils::testing::FakeMcpServer;
use std::error::Error;

#[tokio::test]
async fn each_server_contributes_its_own_instructions() {
    let servers = vec![
        ServerSpec::in_process("server1", FakeMcpServer::math().with_instructions("First server")),
        ServerSpec::in_process("server2", FakeMcpServer::math().with_instructions("Second server")),
        ServerSpec::in_process("silent", FakeMcpServer::math()),
    ];
    let instructions = ready_catalog(mcp("/workspace").with_servers(servers)).await.instructions();

    assert_eq!(instructions, mcp_instructions(&[("server1", "First server"), ("server2", "Second server")]));
}

#[tokio::test]
async fn progressive_discovery_renders_apart_from_the_server_instructions() {
    let only_deferred = McpInstructions { progressive_discovery: true, ..McpInstructions::default() };

    let formatted = Prompt::McpInstructions(only_deferred).build().await.unwrap();

    assert!(formatted.contains("aether mcp"));
    assert!(!formatted.contains("<mcp-server"), "{formatted}");
}

#[tokio::test]
async fn server_instructions_render_as_tagged_sections_in_name_order() {
    let instructions = mcp_instructions(&[("zebra", "Always confirm."), ("alpha", "Use absolute paths.")]);

    let formatted = Prompt::McpInstructions(instructions).build().await.unwrap();

    assert!(formatted.starts_with("# MCP Server Instructions"), "{formatted}");
    let alpha = formatted.find("<mcp-server name=\"alpha\">\nUse absolute paths.\n</mcp-server>").expect("alpha");
    let zebra = formatted.find("<mcp-server name=\"zebra\">\nAlways confirm.\n</mcp-server>").expect("zebra");
    assert!(alpha < zebra);
}

#[tokio::test]
async fn test_agent_builder_includes_mcp_instructions_in_system_prompt() -> Result<(), Box<dyn Error>> {
    let result = test_agent()
        .mcp_server("test-server", FakeMcpServer::math().with_instructions("Use the math tools"))
        .llm_responses(&[llm_response().text(&["done"]).build()])
        .system_prompt(Prompt::text("You are a test agent"))
        .user_text("test")
        .run_with_context()
        .await?;

    let contexts = result.contexts();
    let first_message = contexts[0].messages().first().expect("expected at least one message");
    let ChatMessage::System { content, .. } = first_message else {
        panic!("Expected system message, got: {first_message:?}");
    };
    assert!(content.contains("You are a test agent"));
    assert!(content.contains("<mcp-server name=\"test-server\">\nUse the math tools\n</mcp-server>"));
    Ok(())
}

#[tokio::test]
async fn test_agent_builder_works_without_mcp_instructions() -> Result<(), Box<dyn Error>> {
    let result = test_agent()
        .without_mcp()
        .llm_responses(&[llm_response().text(&["done"]).build()])
        .system_prompt(Prompt::text("You are a test agent"))
        .user_text("test")
        .run_with_context()
        .await?;

    let contexts = result.contexts();
    let first_message = contexts[0].messages().first().expect("expected at least one message");
    let ChatMessage::System { content, .. } = first_message else {
        panic!("Expected system message, got: {first_message:?}");
    };
    assert!(content.contains("You are a test agent"));
    assert!(!content.contains("<mcp-server"), "no MCP instructions section should be added: {content}");
    Ok(())
}
