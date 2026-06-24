pub mod config;
pub mod error;
pub mod manager;
pub mod oauth_handler;

mod connection;
mod connection_attempt_manager;
mod mcp_client;
mod naming;
mod tool_proxy;

pub use config::{
    HttpServerConfig, HttpType, InMemoryServerConfig, InMemoryType, McpConfig, McpServer, McpServerCloneError,
    McpServerConfig, McpTransport, ParseError, ServerFactory, SseServerConfig, SseType, StdioServerConfig, StdioType,
};
pub use connection::{McpConnectAttempt, McpConnectOutcome, McpServerConnection};
pub use connection_attempt_manager::McpConnectionAttemptManager;
pub use error::{McpError, Result};
pub use manager::{
    ElicitationRequest, McpClientEvent, McpConnectionDetails, McpManager, McpServerStatus, McpServerStatusEntry,
    OAuthHandlerContext, OAuthHandlerFactory, UrlElicitationCompleteParams,
};
pub use mcp_client::{McpClient, cancel_result};
pub use naming::{SERVERNAME_DELIMITER, split_on_server_name};
pub use oauth_handler::{AETHER_OAUTH_ELICITATION_ID, ElicitingOAuthHandler};

use std::path::PathBuf;

pub(crate) fn aether_home() -> Option<PathBuf> {
    utils::SettingsStore::new("AETHER_HOME", ".aether").map(|s| s.home().to_path_buf())
}
