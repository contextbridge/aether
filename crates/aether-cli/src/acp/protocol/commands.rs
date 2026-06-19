use agent_client_protocol::schema as acp;
use rmcp::model::Prompt as McpPrompt;

/// Converts an MCP Prompt to an ACP `AvailableCommand`
///
/// Strips the MCP namespace from the prompt name (e.g., "`coding__web`" -> "web")
/// and creates a slash command that clients can invoke.
pub fn map_mcp_prompt_to_available_command(prompt: &McpPrompt) -> acp::AvailableCommand {
    let command_name = prompt.name.split("__").last().unwrap_or(prompt.name.as_ref()).to_string();

    let hint = prompt
        .arguments
        .as_ref()
        .and_then(|args| args.iter().find(|a| a.name.as_str() == "ARGUMENTS").and_then(|a| a.description.as_deref()))
        .unwrap_or("optional arguments");

    let input = Some(acp::AvailableCommandInput::Unstructured(acp::UnstructuredCommandInput::new(hint)));

    let description = prompt.description.clone().unwrap_or_else(|| "No description available".to_string());

    acp::AvailableCommand::new(command_name, description).input(input)
}
