use acp_utils::server::AcpServerError;
use agent_client_protocol::schema::v1::{self as acp, SessionId};
use agent_client_protocol::{Client, ConnectionTo};
use llm::ContentBlock;
use tracing::{error, info};

use super::agent_runtime::AgentRuntime;
use crate::slash_commands::{expand_slash_command, parse_slash_command};

pub(crate) async fn expand_slash_command_in_content(
    runtime: &AgentRuntime,
    mut content: Vec<ContentBlock>,
) -> Vec<ContentBlock> {
    if let Some(ContentBlock::Text { text }) = content.first() {
        let expanded = expand_slash_command_text(runtime, text.clone()).await;
        content[0] = ContentBlock::text(expanded);
    }
    content
}

async fn expand_slash_command_text(runtime: &AgentRuntime, text: String) -> String {
    let Some(slash_command) = parse_slash_command(&text) else {
        return text;
    };

    match expand_slash_command(&runtime.mcp_client(), slash_command.command_name, slash_command.args_text).await {
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
