use crate::client::ClientError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("Server not found: {0}")]
    ServerNotFound(String),
    #[error("MCP gateway is closed")]
    Closed,
    #[error(transparent)]
    Client(#[from] ClientError),
}
