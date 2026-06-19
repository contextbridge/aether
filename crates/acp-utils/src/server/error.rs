/// Errors returned by ACP server-side outbound traffic.
#[derive(Debug, thiserror::Error)]
pub enum AcpServerError {
    #[error("ACP protocol error during {operation}: {source}")]
    Protocol {
        operation: String,
        #[source]
        source: agent_client_protocol::Error,
    },
}

impl AcpServerError {
    pub fn protocol(operation: &'static str, source: agent_client_protocol::Error) -> Self {
        Self::Protocol { operation: operation.to_string(), source }
    }
}
