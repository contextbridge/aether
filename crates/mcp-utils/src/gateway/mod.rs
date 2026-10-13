mod connection;
mod error;
mod mcp_gateway;

pub use error::GatewayError;
pub use mcp_gateway::{ConnectedServer, McpGateway, ServerPrompt, ServerSpec};
