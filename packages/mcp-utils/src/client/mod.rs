pub mod config;
pub mod error;
pub mod manager;

mod connection;
mod mcp_client;
mod naming;
mod roots;
mod tool_proxy;
mod variables;

pub use config::{
    HttpServerConfig, HttpType, InMemoryServerConfig, InMemoryType, McpConfig, McpServer, McpServerConfig,
    McpTransport, ParseError, ServerFactory, SseServerConfig, SseType, StdioServerConfig, StdioType,
};
pub use connection::{McpConnectAttempt, McpConnectOutcome, McpServerConnection, ServerInstructions};
pub use error::{McpError, Result};
pub use manager::{
    ElicitationRequest, McpClientEvent, McpManager, McpServerStatus, McpServerStatusEntry, OAuthHandlerFactory,
    UrlElicitationCompleteParams,
};
pub use mcp_client::{McpClient, cancel_result};
pub use naming::{SERVERNAME_DELIMITER, split_on_server_name};
pub use rmcp::model::Root;
pub use roots::root_from_path;

use std::path::PathBuf;

pub(crate) fn aether_home() -> Option<PathBuf> {
    utils::SettingsStore::new("AETHER_HOME", ".aether").map(|s| s.home().to_path_buf())
}
