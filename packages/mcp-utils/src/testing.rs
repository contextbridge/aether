use crate::transport::create_in_memory_transport;
use rmcp::{
    RoleClient, RoleServer, Service, serve_client, serve_server,
    service::{ClientInitializeError, RunningService, ServerInitializeError},
};

/// Helper function to connect an MCP server and client via in-memory transport
/// This handles the initialization handshake by running both concurrently
pub async fn connect<S, C>(
    server: S,
    client: C,
) -> Result<(RunningService<RoleServer, S>, RunningService<RoleClient, C>), ConnectError>
where
    S: Service<RoleServer>,
    C: Service<RoleClient>,
{
    let (client_transport, server_transport) = create_in_memory_transport();

    let (server_result, client_result) =
        tokio::join!(serve_server(server, server_transport), serve_client(client, client_transport));

    let server = server_result.map_err(ConnectError::ServerInit)?;
    let client = client_result.map_err(ConnectError::ClientInit)?;

    Ok((server, client))
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("Server initialization failed: {0}")]
    ServerInit(ServerInitializeError),
    #[error("Client initialization failed: {0}")]
    ClientInit(ClientInitializeError),
}
