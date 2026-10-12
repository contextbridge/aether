use aether_core::mcp::McpCatalog;
use llm::ContentBlock as MessageContent;
use mcp_utils::gateway::{GatewayError, ServerPrompt};
use mcp_utils::model::{ContentBlock, GetPromptResult};
use std::collections::HashSet;
use thiserror::Error;
use tracing::{debug, error, info};

pub(crate) async fn expand_slash_command_in_content(
    mcp: &McpCatalog,
    mut content: Vec<MessageContent>,
) -> Vec<MessageContent> {
    if let Some(MessageContent::Text { text }) = content.first() {
        let expanded = expand_slash_command_text(mcp, text.clone()).await;
        content[0] = MessageContent::text(expanded);
    }
    content
}

pub(crate) async fn expand_slash_command_text(mcp: &McpCatalog, text: String) -> String {
    let Some(slash_command) = parse_slash_command(&text) else {
        return text;
    };

    match expand_slash_command(mcp, slash_command.command_name, slash_command.args_text).await {
        Ok(expanded) => {
            info!("Expanded slash command -> {} chars", expanded.len());
            expanded
        }
        Err(SlashCommandError::NotFound(command)) => {
            debug!(command, "Passing an unknown slash command through unexpanded");
            text
        }
        Err(error) => {
            error!("Failed to expand slash command: {error}");
            text
        }
    }
}

pub(crate) async fn prompt_commands(mcp: &McpCatalog) -> Vec<ServerPrompt> {
    let mut seen_names = HashSet::new();
    let mut prompts = mcp.list_prompts().await;
    prompts.retain(|listed| seen_names.insert(listed.prompt.name.clone()));
    prompts
}

struct SlashCommand<'a> {
    command_name: &'a str,
    args_text: &'a str,
}

#[derive(Error, Debug)]
enum SlashCommandError {
    #[error("MCP operation failed: {0}")]
    McpOperation(#[from] GatewayError),
    #[error("slash command '/{0}' not found")]
    NotFound(String),
    #[error("prompt result contains no text content")]
    NoTextContent,
}

fn parse_slash_command(text: &str) -> Option<SlashCommand<'_>> {
    let slash_command_text = text.strip_prefix('/')?;

    let (command_name, args_text) = if let Some(space_idx) = slash_command_text.find(char::is_whitespace) {
        let (cmd, args) = slash_command_text.split_at(space_idx);
        (cmd, args.trim())
    } else {
        (slash_command_text, "")
    };

    Some(SlashCommand { command_name, args_text })
}

async fn expand_slash_command(
    mcp: &McpCatalog,
    command_name: &str,
    args_text: &str,
) -> Result<String, SlashCommandError> {
    let command = prompt_commands(mcp)
        .await
        .into_iter()
        .find(|listed| listed.prompt.name == command_name)
        .ok_or_else(|| SlashCommandError::NotFound(command_name.to_string()))?;
    let arguments = parse_slash_command_arguments(args_text);
    let prompt_result = mcp.get_prompt(&command.server, &command.prompt.name, arguments).await?;
    prompt_result_text(&prompt_result)
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

fn parse_slash_command_arguments(args_text: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut arg_map = serde_json::Map::new();
    if args_text.is_empty() {
        return arg_map;
    }
    arg_map.insert("ARGUMENTS".to_string(), serde_json::Value::String(args_text.to_string()));
    for (i, arg) in args_text.split_whitespace().enumerate() {
        arg_map.insert((i + 1).to_string(), serde_json::Value::String(arg.to_string()));
    }
    arg_map
}
