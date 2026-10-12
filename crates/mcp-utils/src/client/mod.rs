//! [`McpClient`]: one live connection to one MCP server.

mod elicitation;
mod error;
mod handler;
mod mcp_client;
mod oauth;
mod options;
mod sigv4;
mod task;
mod tool_call;
mod transport;

#[cfg(any(test, feature = "testing"))]
pub(crate) use elicitation::cancelled;
pub use elicitation::{Elicitation, ElicitationRequest, ElicitationSink};
pub use error::ClientError;
pub use mcp_client::McpClient;
pub use oauth::OAuthSettings;
pub use options::ClientOptions;
pub use reqwest::header::HeaderMap;
pub use task::TaskErrorReason;
pub use tokio_util::sync::CancellationToken;
pub use tool_call::{ToolCall, ToolCallError, ToolCallEvent, ToolCallOptions};
pub use transport::{HttpAuth, Transport};
