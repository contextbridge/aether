//! [`McpClient`]: one live connection to one MCP server.

mod elicitation;
mod handler;
mod mcp_client;
mod oauth;
mod options;
mod tools;
mod transport;

pub(crate) use elicitation::cancelled;
pub use elicitation::{Elicitation, ElicitationRequest};
pub use mcp_client::McpClient;
pub use options::ClientOptions;
pub use reqwest::header::HeaderMap;
pub use tokio_util::sync::{CancellationToken, DropGuard};
pub use tools::{TaskErrorReason, ToolCall, ToolCallError, ToolCallEvent, ToolCallOptions};
pub use transport::Transport;
#[cfg(any(test, feature = "testing"))]
pub(crate) use transport::client_lifecycle_mode;
