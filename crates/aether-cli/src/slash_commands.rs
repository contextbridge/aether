use aether_core::mcp::McpCommandClient;
use agent_client_protocol::schema::v1::AvailableCommand;
use rmcp::model::{ContentBlock, GetPromptResult, Prompt as McpPrompt};
use std::collections::HashSet;
use thiserror::Error;

pub(crate) struct SlashCommand<'a> {
    pub(crate) command_name: &'a str,
    pub(crate) args_text: &'a str,
}

#[derive(Error, Debug)]
pub(crate) enum SlashCommandError {
    #[error("MCP operation failed: {0}")]
    McpOperation(String),
    #[error("slash command '/{0}' not found")]
    NotFound(String),
    #[error("prompt result contains no text content")]
    NoTextContent,
}

pub(crate) fn parse_slash_command(text: &str) -> Option<SlashCommand<'_>> {
    let slash_command_text = text.strip_prefix('/')?;

    let (command_name, args_text) = if let Some(space_idx) = slash_command_text.find(char::is_whitespace) {
        let (cmd, args) = slash_command_text.split_at(space_idx);
        (cmd, args.trim())
    } else {
        (slash_command_text, "")
    };

    Some(SlashCommand { command_name, args_text })
}

pub(crate) fn dedupe_commands_by_name(commands: Vec<AvailableCommand>) -> Vec<AvailableCommand> {
    let mut seen_names = HashSet::new();
    commands.into_iter().filter(|command| seen_names.insert(command.name.clone())).collect()
}

pub(crate) async fn expand_slash_command(
    mcp_client: &McpCommandClient,
    command_name: &str,
    args_text: &str,
) -> Result<String, SlashCommandError> {
    let arguments = parse_slash_command_arguments(args_text);
    let prompts = list_prompts(mcp_client).await?;
    let prompt_name = find_prompt_name(&prompts, command_name)?;
    let prompt_result = get_prompt(mcp_client, prompt_name, arguments).await?;
    prompt_result_text(&prompt_result)
}

pub(crate) async fn list_prompts(mcp_client: &McpCommandClient) -> Result<Vec<McpPrompt>, SlashCommandError> {
    mcp_client.list_prompts().await.map_err(|error| SlashCommandError::McpOperation(error.to_string()))
}

async fn get_prompt(
    mcp_client: &McpCommandClient,
    name: String,
    arguments: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<GetPromptResult, SlashCommandError> {
    mcp_client.get_prompt(name, arguments).await.map_err(|error| SlashCommandError::McpOperation(error.to_string()))
}

fn find_prompt_name(prompts: &[McpPrompt], command_name: &str) -> Result<String, SlashCommandError> {
    prompts
        .iter()
        .find(|p| p.name.split("__").last().unwrap_or("") == command_name)
        .map(|prompt| prompt.name.clone())
        .ok_or_else(|| SlashCommandError::NotFound(command_name.to_string()))
}

fn prompt_result_text(prompt_result: &GetPromptResult) -> Result<String, SlashCommandError> {
    prompt_result
        .messages
        .first()
        .and_then(|message| match &message.content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .ok_or(SlashCommandError::NoTextContent)
}

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
