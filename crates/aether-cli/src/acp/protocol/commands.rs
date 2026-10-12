use agent_client_protocol::schema::v2::{self as acp, AvailableCommandInput, TextCommandInput};
use mcp_utils::gateway::ServerPrompt;

pub(crate) fn available_commands(prompts: &[ServerPrompt]) -> Vec<acp::AvailableCommand> {
    prompts.iter().map(|listed| available_command(&listed.prompt)).collect()
}

fn available_command(prompt: &mcp_utils::model::Prompt) -> acp::AvailableCommand {
    let hint = prompt
        .arguments
        .as_ref()
        .and_then(|args| args.iter().find(|a| a.name.as_str() == "ARGUMENTS").and_then(|a| a.description.as_deref()))
        .unwrap_or("optional arguments");
    let description = prompt.description.clone().unwrap_or_else(|| "No description available".to_string());
    acp::AvailableCommand::new(&prompt.name, description)
        .input(Some(AvailableCommandInput::Text(TextCommandInput::new(hint))))
}
