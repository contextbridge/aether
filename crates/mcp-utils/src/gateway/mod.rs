mod catalog;
mod connection;
mod deferred_tools;
mod exposure;
mod mcp_gateway;
mod servers;
mod tool_filter;

pub use catalog::McpCatalog;
pub use exposure::ToolExposure;
pub use mcp_gateway::{McpGateway, ServerSpec};
pub use tool_filter::{ToolAnnotationMatcher, ToolFilter, ToolMatcher};

pub const LIST_SERVERS_TOOL: &str = "_aether_list_servers";
pub const SERVERNAME_DELIMITER: &str = "__";

pub fn namespaced(server: &str, name: &str) -> String {
    format!("{server}{SERVERNAME_DELIMITER}{name}")
}

pub fn split_namespaced(name: &str) -> Option<(&str, &str)> {
    name.split_once(SERVERNAME_DELIMITER)
}
