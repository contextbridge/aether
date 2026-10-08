mod mcp_builder;
pub mod tool_bridge;

pub use mcp_builder::{
    AETHER_MCP_IPC_SOCKET, McpBuilder, McpRuntime, McpSpawnError, RuntimeServices, ServerFactory, mcp, mcp_instructions,
};
