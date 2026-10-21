use acp_utils::client::AcpClientError;
use thiserror::Error;

/// Failures while connecting to an agent and opening its session.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Server did not advertise the Aether remote-server contract; connect to `aether server`")]
    MissingRemoteContract,
    #[error("{0}")]
    Acp(#[from] AcpClientError),
}
