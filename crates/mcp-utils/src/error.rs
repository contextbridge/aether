use rmcp::model::ErrorData;
use rmcp::service::{ClientInitializeError, ServerInitializeError, ServiceError};
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::task::JoinError;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
    #[error("Tool '{tool}' is exposed directly; call '{direct_name}' instead")]
    ToolNotDeferred { tool: String, direct_name: String },
    #[error("Name is not namespaced as server__name: {0}")]
    NotNamespaced(String),
    #[error("Server not found: {0}")]
    ServerNotFound(String),
    #[error("Failed to spawn '{command}': {source}")]
    Spawn {
        command: String,
        #[source]
        source: io::Error,
    },
    #[error("Failed to connect to MCP socket {}: {source}", path.display())]
    SocketConnect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Failed to connect to MCP server '{server}': {source}")]
    Connect {
        server: String,
        #[source]
        source: Box<ClientInitializeError>,
    },
    #[error("MCP server '{server}' requires OAuth authorization")]
    AuthRequired { server: String, challenge: Option<String> },
    #[error("MCP server '{server}' cannot be authorized interactively")]
    OAuthUnavailable { server: String },
    #[cfg(feature = "client")]
    #[error("OAuth failed for MCP server '{server}': {source}")]
    Auth {
        server: String,
        #[source]
        source: aether_auth::OAuthError,
    },
    #[error("Authentication for MCP server '{server}' timed out after {timeout:?}")]
    AuthTimedOut { server: String, timeout: Duration },
    #[error("Request to MCP server '{server}' failed: {source}")]
    Request {
        server: String,
        #[source]
        source: Box<ServiceError>,
    },
    #[error("Failed to create MCP socket directory {}: {source}", path.display())]
    SocketDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Failed to bind MCP socket {}: {source}", path.display())]
    Bind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("MCP server initialization failed: {0}")]
    ServerInit(#[source] Box<ServerInitializeError>),
    #[error("MCP server task failed: {0}")]
    ServerTask(#[source] JoinError),
    #[error("MCP gateway is closed")]
    GatewayClosed,
}

#[cfg(feature = "client")]
impl McpError {
    pub(crate) fn connect(server: impl Into<String>, source: ClientInitializeError) -> Self {
        Self::Connect { server: server.into(), source: Box::new(source) }
    }

    pub(crate) fn request(server: impl Into<String>, source: ServiceError) -> Self {
        Self::Request { server: server.into(), source: Box::new(source) }
    }
}

impl From<ServerInitializeError> for McpError {
    fn from(error: ServerInitializeError) -> Self {
        McpError::ServerInit(Box::new(error))
    }
}

/// Unknown or malformed names are the MCP client's mistake; anything else is the server's.
impl From<McpError> for ErrorData {
    fn from(error: McpError) -> Self {
        match error {
            McpError::ToolNotFound(_)
            | McpError::ToolNotDeferred { .. }
            | McpError::NotNamespaced(_)
            | McpError::ServerNotFound(_) => ErrorData::invalid_params(error.to_string(), None),
            _ => ErrorData::internal_error(error.to_string(), None),
        }
    }
}

#[cfg(feature = "client")]
pub(crate) type Result<T> = std::result::Result<T, McpError>;
