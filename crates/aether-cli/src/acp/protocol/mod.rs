//! ACP protocol mappers.
//!
//! Each submodule owns one direction of translation between Aether core types
//! and the `agent_client_protocol` schema:

pub(crate) mod commands;
pub(crate) mod content;
pub(crate) mod diff;
pub(crate) mod events;
pub(crate) mod mcp;
pub(crate) mod replay;

pub use commands::map_mcp_prompt_to_available_command;

pub(crate) fn notify(
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    notification: impl agent_client_protocol::JsonRpcNotification,
) {
    let method = notification.method().to_owned();
    if let Err(error) = connection.send_notification(notification) {
        tracing::warn!(%method, %error, "Failed to send ACP notification");
    }
}
