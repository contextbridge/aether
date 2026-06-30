use acp_utils::server::AcpServerError;
use agent_client_protocol::schema::{self as acp, SessionId};
use agent_client_protocol::{Client, ConnectionTo};
use llm::ContentBlock;
use tracing::{error, info};

use super::agent_runtime::AgentRuntime;
use crate::acp::error::SessionError;
pub(crate) use crate::slash_commands::dedupe_commands_by_name;
use crate::slash_commands::{
    SlashCommandError, find_prompt_name, parse_slash_command, parse_slash_command_arguments, prompt_result_text,
};

pub(crate) async fn expand_slash_command_in_content(
    runtime: &AgentRuntime,
    mut content: Vec<ContentBlock>,
) -> Vec<ContentBlock> {
    if let Some(ContentBlock::Text { text }) = content.first()
        && text.starts_with('/')
    {
        let expanded = expand_slash_command_text(runtime, text.clone()).await;
        content[0] = ContentBlock::text(expanded);
    }
    content
}

async fn expand_slash_command_text(runtime: &AgentRuntime, text: String) -> String {
    let Some(slash_command) = parse_slash_command(&text) else {
        return text;
    };

    match expand_slash_command(runtime, slash_command.command_name, slash_command.args_text).await {
        Ok(expanded) => {
            info!("Expanded slash command -> {} chars", expanded.len());
            expanded
        }
        Err(e) => {
            error!("Failed to expand slash command: {}", e);
            text
        }
    }
}

async fn expand_slash_command(
    runtime: &AgentRuntime,
    command_name: &str,
    args_text: &str,
) -> Result<String, SlashCommandError> {
    let arguments = parse_slash_command_arguments(args_text);
    let prompts = runtime.list_prompts().await.map_err(slash_command_error)?;
    let prompt_name = find_prompt_name(&prompts, command_name)?;
    let prompt_result = runtime.get_prompt(prompt_name, arguments).await.map_err(slash_command_error)?;

    prompt_result_text(&prompt_result)
}

fn slash_command_error(error: super::error::SessionError) -> SlashCommandError {
    match error {
        SessionError::CommandChannel(message) => SlashCommandError::CommandChannel(message),
        other => SlashCommandError::McpOperation(other.to_string()),
    }
}

pub(crate) fn send_available_commands(
    connection: &ConnectionTo<Client>,
    acp_session_id: SessionId,
    available_commands: Vec<acp::AvailableCommand>,
) {
    if let Err(e) = connection
        .send_notification(acp::SessionNotification::new(
            acp_session_id,
            acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(available_commands)),
        ))
        .map_err(|e| AcpServerError::protocol("session/update", e))
    {
        error!("Failed to send available commands update: {:?}", e);
    }
}
