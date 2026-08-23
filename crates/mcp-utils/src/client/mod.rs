pub mod config;
pub mod error;
pub mod manager;
pub mod oauth_handler;

mod call_tool;
mod connection;
mod connection_attempt_manager;
mod elicitation;
mod mcp_client;
mod mcp_snapshot;
mod mrtr;
mod naming;
mod task;
mod tool_catalog;
mod tool_filter;

pub use call_tool::{CallToolError, CallToolOptions, ToolCallEvent, call_tool};
pub use config::{
    AETHER_OAUTH_CALLBACK_PORT, AETHER_OAUTH_CLIENT_METADATA_URL, DeferredToolRules, InMemoryServerConfig,
    InMemoryServerSpec, InMemoryType, McpConfig, McpHttpConfig, McpOAuthConfig, McpServer, McpServerConfig,
    McpTransport, ParseError, RemoteServerConfig, RemoteType, ResolvedOAuth, StdioServerConfig, StdioType,
    ToolExposure, loopback_redirect_uri,
};
pub use connection::{McpConnectAttempt, McpConnectOutcome, McpServerConnection};
pub use connection_attempt_manager::McpConnectionAttemptManager;
pub use error::{McpError, Result};
pub use manager::{
    ElicitationRequest, McpClientEvent, McpConnectionDetails, McpManager, McpServerStatus, McpServerStatusEntry,
    OAuthHandlerContext, OAuthHandlerFactory, RuntimeMcpServer, RuntimeMcpTransport,
};
pub use mcp_client::{McpClient, cancel_result, client_capabilities, client_capabilities_for};
pub use mcp_snapshot::McpSnapshot;
pub use mrtr::AbortReason;
pub use naming::{SERVERNAME_DELIMITER, split_on_server_name};
pub use oauth_handler::ElicitingOAuthHandler;
pub use task::TaskErrorReason;
pub use tokio_util::sync::CancellationToken;
pub use tool_catalog::{
    CatalogTool, CatalogTools, PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME, ServerCatalogEntry, ServerDescription,
    ToolCatalog, ToolExposureKind, ToolRoute,
};
pub use tool_filter::{ToolAnnotationMatcher, ToolFilter, ToolMatcher};
