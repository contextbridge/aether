use aether_auth::OAuthError;
use rmcp::service::{ClientInitializeError, ServiceError};
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
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
    #[error("OAuth failed for MCP server '{server}': {source}")]
    Auth {
        server: String,
        #[source]
        source: OAuthError,
    },
    #[error("Authentication for MCP server '{server}' timed out after {timeout:?}")]
    AuthTimedOut { server: String, timeout: Duration },
    #[error("Request to MCP server '{server}' failed: {source}")]
    Request {
        server: String,
        #[source]
        source: Box<ServiceError>,
    },
    #[error("MCP server '{server}' disconnected")]
    Disconnected { server: String },
}

impl ClientError {
    pub(crate) fn connect(server: impl Into<String>, source: ClientInitializeError) -> Self {
        Self::Connect { server: server.into(), source: Box::new(source) }
    }

    pub(crate) fn request(server: impl Into<String>, source: ServiceError) -> Self {
        Self::Request { server: server.into(), source: Box::new(source) }
    }
}

pub(crate) type Result<T> = std::result::Result<T, ClientError>;
