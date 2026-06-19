use acp_utils::server::AcpServerError;
use agent_client_protocol::schema::{self as acp, SessionId};
use agent_client_protocol::{Client, ConnectionTo};
use llm::ContentBlock;
use std::collections::HashSet;
use tracing::{error, info};

use super::agent_runtime::AgentRuntime;
use super::error::{SessionError, SlashCommandError};

pub(crate) async fn expand_slash_command_in_content(
    runtime: &AgentRuntime,
    mut content: Vec<ContentBlock>,
) -> Vec<ContentBlock> {
    if let Some(ContentBlock::Text { text }) = content.first()
        && text.starts_with('/')
    {
        let expanded = expand_slash_command_if_needed(runtime, text.clone()).await;
        content[0] = ContentBlock::text(expanded);
    }
    content
}

async fn expand_slash_command_if_needed(runtime: &AgentRuntime, text: String) -> String {
    let Some(slash_command_text) = text.strip_prefix('/') else {
        return text;
    };

    let (command_name, args_text) = if let Some(space_idx) = slash_command_text.find(char::is_whitespace) {
        let (cmd, args) = slash_command_text.split_at(space_idx);
        (cmd, args.trim())
    } else {
        (slash_command_text, "")
    };

    match expand_slash_command(runtime, command_name, args_text).await {
        Ok(expanded) => {
            info!("Expanded slash command '{}' -> {} chars", command_name, expanded.len());
            expanded
        }
        Err(e) => {
            error!("Failed to expand slash command '{}': {}", command_name, e);
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

    let prompts = runtime.list_prompts().await.map_err(|error| match error {
        SessionError::CommandChannel(message) => SlashCommandError::CommandChannel(message),
        other => SlashCommandError::McpOperation(other.to_string()),
    })?;

    let matching_prompt = prompts
        .iter()
        .find(|p| p.name.split("__").last().unwrap_or("") == command_name)
        .ok_or_else(|| SlashCommandError::NotFound(command_name.to_string()))?;

    let namespaced_name = matching_prompt.name.clone();

    let prompt_result = runtime.get_prompt(namespaced_name.clone(), arguments).await.map_err(|error| match error {
        SessionError::CommandChannel(message) => SlashCommandError::CommandChannel(message),
        other => SlashCommandError::McpOperation(other.to_string()),
    })?;

    prompt_result
        .messages
        .first()
        .and_then(|message| match &message.content {
            rmcp::model::PromptMessageContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .ok_or(SlashCommandError::NoTextContent)
}

/// Parse slash command arguments into a map with both positional and special variables.
///
/// Creates an argument map with:
/// - "ARGUMENTS": The full argument string
/// - "1", "2", "3", etc.: Individual positional arguments (1-based)
fn parse_slash_command_arguments(args_text: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    if args_text.is_empty() {
        None
    } else {
        let mut arg_map = serde_json::Map::new();
        arg_map.insert("ARGUMENTS".to_string(), serde_json::Value::String(args_text.to_string()));
        for (i, arg) in args_text.split_whitespace().enumerate() {
            arg_map.insert((i + 1).to_string(), serde_json::Value::String(arg.to_string()));
        }
        Some(arg_map)
    }
}

pub(crate) fn dedupe_commands_by_name(commands: Vec<acp::AvailableCommand>) -> Vec<acp::AvailableCommand> {
    let mut seen_names = HashSet::new();
    commands.into_iter().filter(|command| seen_names.insert(command.name.clone())).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argument_parsing() {
        let arg_map = parse_slash_command_arguments("do a thing that has spaces").expect("Expected Some");
        let expected = serde_json::Map::from_iter([
            ("ARGUMENTS".to_string(), serde_json::Value::String("do a thing that has spaces".to_string())),
            ("1".to_string(), serde_json::Value::String("do".to_string())),
            ("2".to_string(), serde_json::Value::String("a".to_string())),
            ("3".to_string(), serde_json::Value::String("thing".to_string())),
            ("4".to_string(), serde_json::Value::String("that".to_string())),
            ("5".to_string(), serde_json::Value::String("has".to_string())),
            ("6".to_string(), serde_json::Value::String("spaces".to_string())),
        ]);
        assert_eq!(arg_map, expected);
    }

    #[test]
    fn test_empty_arguments_returns_none() {
        assert!(parse_slash_command_arguments("").is_none());
    }
}
