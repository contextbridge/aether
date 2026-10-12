mod config;
mod deferred_tools;
mod exposure;
mod mcp_builder;
mod mcp_catalog;
pub(crate) mod tool_bridge;
mod tool_filter;

pub use config::{
    InMemoryServerConfig, InMemoryType, McpConfig, McpOAuthConfig, McpServerConfig, ParseError, RemoteServerConfig,
    RemoteType, StdioServerConfig, StdioType, http_transport,
};
pub(crate) use deferred_tools::PROGRESSIVE_DISCOVERY_INSTRUCTIONS;
pub use deferred_tools::{
    AETHER_MCP_IPC_SOCKET, DeferredServer, DeferredTool, DeferredToolsClient, DeferredToolsError,
};
pub use exposure::ToolExposure;
pub use mcp_builder::{McpBuilder, McpSpawnError, RuntimeServices, ServerFactoryError, mcp};
pub use mcp_catalog::McpCatalog;
pub use tool_filter::{ToolAnnotationMatcher, ToolFilter, ToolMatcher};
