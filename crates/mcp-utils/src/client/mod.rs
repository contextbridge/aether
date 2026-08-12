pub mod config;
pub mod error;
pub mod manager;
pub mod oauth_handler;

mod call_tool;
mod connection;
mod connection_attempt_manager;
mod elicitation;
mod mcp_client;
mod mrtr;
mod naming;
mod task;
mod tool_proxy;

pub use call_tool::{CallToolError, CallToolOptions, ToolCallEvent, call_tool};
pub use config::{
    InMemoryServerConfig, InMemoryType, McpConfig, McpHttpConfig, McpOAuthConfig, McpServer, McpServerCloneError,
    McpServerConfig, McpTransport, ParseError, RemoteServerConfig, RemoteType, ServerFactory, StdioServerConfig,
    StdioType,
};
pub use connection::{McpConnectAttempt, McpConnectOutcome, McpServerConnection};
pub use connection_attempt_manager::McpConnectionAttemptManager;
pub use error::{McpError, Result};
pub use manager::{
    ElicitationRequest, McpClientEvent, McpConnectionDetails, McpManager, McpServerStatus, McpServerStatusEntry,
    OAuthHandlerContext, OAuthHandlerFactory,
};
pub use mcp_client::{McpClient, cancel_result, client_capabilities};
pub use mrtr::AbortReason;
pub use naming::{SERVERNAME_DELIMITER, split_on_server_name};
pub use oauth_handler::ElicitingOAuthHandler;
pub use task::TaskErrorReason;

use std::path::PathBuf;

pub(crate) fn aether_home() -> Option<PathBuf> {
    utils::SettingsStore::new("AETHER_HOME", ".aether").map(|s| s.home().to_path_buf())
}
