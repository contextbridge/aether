use super::actor::SessionIo;
use agent_client_protocol::schema::v2 as acp;
use llm::ContentBlock;
use tracing::{error, info};

use crate::slash_commands::{expand_slash_command, parse_slash_command};
use aether_core::mcp::McpHandle;

pub(crate) async fn expand_slash_command_in_content(
    mcp: &McpHandle,
    mut content: Vec<ContentBlock>,
) -> Vec<ContentBlock> {
    if let Some(ContentBlock::Text { text }) = content.first() {
        let expanded = expand_slash_command_text(mcp, text.clone()).await;
        content[0] = ContentBlock::text(expanded);
    }
    content
}

async fn expand_slash_command_text(mcp: &McpHandle, text: String) -> String {
    let Some(slash_command) = parse_slash_command(&text) else {
        return text;
    };

    match expand_slash_command(mcp, slash_command.command_name, slash_command.args_text).await {
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

pub(crate) fn send_available_commands(io: &SessionIo, available_commands: Vec<acp::AvailableCommand>) {
    io.send_update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(available_commands)));
}
