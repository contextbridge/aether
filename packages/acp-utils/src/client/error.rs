/// Errors returned by ACP client-side operations.
#[derive(Debug, thiserror::Error)]
pub enum AcpClientError {
    /// The agent command string could not be parsed.
    #[error("invalid agent command: {0}")]
    InvalidAgentCommand(#[source] agent_client_protocol::Error),

    /// The transport could not be established before the ACP handshake.
    #[error("ACP connection failed before handshake: {0}")]
    ConnectFailed(#[source] agent_client_protocol::Error),

    /// The agent subprocess exited unexpectedly.
    #[error("agent subprocess crashed: {0}")]
    AgentCrashed(String),

    /// The ACP request/response exchange failed (`initialize`, `new_session`,
    /// `prompt`, etc.).
    #[error("ACP protocol error: {0}")]
    Protocol(#[source] agent_client_protocol::Error),
}
