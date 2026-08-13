use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    /// Tool not found in the registry
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
    /// The tool is exposed directly and cannot be called through the proxy.
    #[error("Tool '{tool_name}' is exposed directly; call '{direct_name}' instead")]
    DirectToolRequiresDirectRoute { tool_name: String, direct_name: String },
    /// The server's tools are not routed through the proxy
    #[error("Server '{0}' is not part of the tool proxy")]
    ServerNotProxied(String),
    /// The proxied server has no active connection
    #[error("Proxied server '{0}' is not connected")]
    ProxiedServerNotConnected(String),
    /// Invalid tool name format (should be `server__tool`)
    #[error("Invalid tool name format: {0}")]
    InvalidToolNameFormat(String),
    /// MCP server not found
    #[error("Server not found: {0}")]
    ServerNotFound(String),
    /// Failed to execute tool on MCP server
    #[error("Failed to execute tool {tool_name} on server {server_name}: {error}")]
    ToolExecutionFailed { tool_name: String, server_name: String, error: String },
    /// Tool execution returned an error
    #[error("Tool execution failed: {0}")]
    ToolExecutionError(String),
    /// Tool discovery failed
    #[error("Tool discovery failed: {0}")]
    ToolDiscoveryFailed(String),
    /// Prompt not found in the registry
    #[error("Prompt not found: {0}")]
    PromptNotFound(String),
    /// Prompt listing failed
    #[error("Prompt listing failed: {0}")]
    PromptListFailed(String),
    /// Prompt retrieval failed
    #[error("Prompt retrieval failed: {0}")]
    PromptGetFailed(String),
    /// Failed to spawn a stdio server process
    #[error("Failed to spawn '{command}': {reason}")]
    SpawnFailed { command: String, reason: String },
    /// Server connection failed
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    /// Server startup failed
    #[error("Server startup failed: {0}")]
    ServerStartupFailed(String),
    /// Transport error
    #[error("Transport error: {0}")]
    TransportError(String),
    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    JsonError(String),
    /// Generic error for other cases
    #[error("{0}")]
    Other(String),
}

impl From<serde_json::Error> for McpError {
    fn from(error: serde_json::Error) -> Self {
        McpError::JsonError(error.to_string())
    }
}

impl From<std::io::Error> for McpError {
    fn from(error: std::io::Error) -> Self {
        McpError::TransportError(error.to_string())
    }
}

impl From<rmcp::service::ClientInitializeError> for McpError {
    fn from(error: rmcp::service::ClientInitializeError) -> Self {
        McpError::ConnectionFailed(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, McpError>;
