use crate::protocol::client_lifecycle_mode;
use crate::transport::create_in_memory_transport;
use rmcp::{
    RoleClient, RoleServer, Service, serve_client_with_lifecycle, serve_server,
    service::{ClientInitializeError, RunningService, ServerInitializeError},
};

/// Helper function to connect an MCP server and client via in-memory transport
/// This handles the dual-era discovery/initialization handshake by running both concurrently
pub async fn connect<S, C>(
    server: S,
    client: C,
) -> Result<(RunningService<RoleServer, S>, RunningService<RoleClient, C>), ConnectError>
where
    S: Service<RoleServer>,
    C: Service<RoleClient>,
{
    let (client_transport, server_transport) = create_in_memory_transport();

    let (server_result, client_result) = tokio::join!(
        serve_server(server, server_transport),
        serve_client_with_lifecycle(client, client_transport, client_lifecycle_mode())
    );

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

#[cfg(test)]
mod tests {
    use super::connect;
    use rmcp::{
        ClientHandler, ServerHandler,
        model::{ErrorData, Implementation, InitializeRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo},
        service::RequestContext,
    };
    use std::borrow::Cow;

    #[tokio::test]
    async fn connect_prefers_stateless_discovery_for_modern_servers() {
        let (_server, client) = connect(McpServer728, TestClient).await.expect("connect");
        assert_eq!(client.peer_info().expect("peer info").protocol_version, ProtocolVersion::V_2026_07_28);
        client.list_tools(None).await.expect("list tools");
        client.cancel().await.expect("cancel client");
    }

    #[tokio::test]
    async fn connect_selects_an_older_mutually_supported_revision() {
        let (_server, client) = connect(McpServer618, TestClient).await.expect("connect");

        assert_eq!(client.peer_info().expect("peer info").protocol_version, ProtocolVersion::V_2025_06_18);
        client.list_tools(None).await.expect("list tools");
        client.cancel().await.expect("cancel client");
    }

    #[tokio::test]
    async fn connect_falls_back_to_legacy_initialization() {
        let (_server, client) = connect(McpServer1125, TestClient).await.expect("connect");

        assert_eq!(client.peer_info().expect("peer info").protocol_version, ProtocolVersion::V_2025_11_25);
        client.cancel().await.expect("cancel client");
    }

    #[derive(Clone, Default)]
    struct TestClient;

    impl ClientHandler for TestClient {}

    #[derive(Clone, Default)]
    struct McpServer728;

    impl ServerHandler for McpServer728 {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("modern-only", "1.0.0"))
                .with_protocol_version(ProtocolVersion::V_2026_07_28)
        }

        fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
            Cow::Owned(vec![ProtocolVersion::V_2026_07_28])
        }

        fn initialize(
            &self,
            _request: InitializeRequestParams,
            _context: RequestContext<rmcp::RoleServer>,
        ) -> impl std::future::Future<Output = Result<rmcp::model::InitializeResult, ErrorData>> + Send + '_ {
            std::future::ready(Err(ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "initialize is not supported",
                None,
            )))
        }
    }

    #[derive(Clone, Default)]
    struct McpServer618;

    impl ServerHandler for McpServer618 {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("older-revision", "1.0.0"))
                .with_protocol_version(ProtocolVersion::V_2025_06_18)
        }

        fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
            Cow::Owned(vec![ProtocolVersion::V_2025_06_18])
        }
    }

    #[derive(Clone, Default)]
    struct McpServer1125;

    impl ServerHandler for McpServer1125 {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("legacy", "1.0.0"))
                .with_protocol_version(ProtocolVersion::V_2025_11_25)
        }

        fn discover(
            &self,
            _context: RequestContext<rmcp::RoleServer>,
        ) -> impl Future<Output = Result<rmcp::model::DiscoverResult, ErrorData>> + Send + '_ {
            std::future::ready(Err(ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "server/discover is not supported",
                None,
            )))
        }
    }
}
