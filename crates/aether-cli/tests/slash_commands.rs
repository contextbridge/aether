use agent_client_protocol::schema as acp;
use rmcp::model::{Prompt, PromptArgument};

/// Create an MCP prompt with no arguments.
fn create_prompt(name: &str, description: Option<&str>) -> Prompt {
    Prompt::new(name.to_string(), description.map(str::to_string), None)
}

/// Create an MCP prompt with an ARGUMENTS parameter (unified prompt format).
fn create_prompt_with_hint(name: &str, description: Option<&str>, hint: &str) -> Prompt {
    let arguments = Some(vec![PromptArgument::new("ARGUMENTS").with_description(hint).with_required(false)]);
    Prompt::new(name.to_string(), description.map(str::to_string), arguments)
}

#[test]
fn test_map_prompt_to_command_strips_namespace() {
    let prompt = create_prompt("coding__web", Some("Search the web"));

    let command = aether_cli::map_mcp_prompt_to_available_command(&prompt);

    assert_eq!(command.name, "web");
    assert_eq!(command.description, "Search the web");
    assert!(command.input.is_some());
}

#[test]
fn test_map_prompt_to_command_without_namespace() {
    let prompt = create_prompt("test", Some("Run tests"));

    let command = aether_cli::map_mcp_prompt_to_available_command(&prompt);

    assert_eq!(command.name, "test");
    assert_eq!(command.description, "Run tests");
}

#[test]
fn test_map_prompt_to_command_with_argument_hint() {
    let prompt = create_prompt_with_hint("search", Some("Search code"), "[query]");

    let command = aether_cli::map_mcp_prompt_to_available_command(&prompt);

    assert_eq!(command.name, "search");
    match command.input {
        Some(acp::AvailableCommandInput::Unstructured(input)) => {
            assert_eq!(input.hint, "[query]");
        }
        _ => panic!("Expected Unstructured input with hint"),
    }
}
