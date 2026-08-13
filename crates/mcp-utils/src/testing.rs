use crate::protocol::client_lifecycle_mode;
use crate::transport::create_in_memory_transport;
use rmcp::{
    RoleClient, RoleServer, Service, serve_client_with_lifecycle, serve_server,
    service::{ClientInitializeError, RunningService, ServerInitializeError},
};

#[cfg(feature = "client")]
pub use elicitation_script::{CapturedElicitation, ElicitationScript};
#[cfg(all(feature = "client", any(test, feature = "testing")))]
pub use fake_mcp::{
    CapturedTaskUpdate, CapturedToolCall, FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse, fake_mcp,
};

#[cfg(all(feature = "client", any(test, feature = "testing")))]
mod fake_mcp;

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

#[cfg(feature = "client")]
mod elicitation_script {
    use crate::client::McpClientEvent;
    use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, PoisonError};
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;

    /// Scripts the user's side of elicitation round trips: answers each
    /// incoming request with the next queued response (Cancel once the queue
    /// is empty) and records what arrived for assertions.
    pub struct ElicitationScript {
        captured: Arc<Mutex<Vec<CapturedElicitation>>>,
        task: JoinHandle<()>,
    }

    #[derive(Clone)]
    pub struct CapturedElicitation {
        pub server_name: String,
        pub request: ElicitRequestParams,
    }

    impl ElicitationScript {
        pub fn spawn(
            mut event_rx: mpsc::Receiver<McpClientEvent>,
            responses: impl IntoIterator<Item = ElicitResult>,
        ) -> Self {
            let mut responses = responses.into_iter().collect::<VecDeque<_>>();
            let captured = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&captured);
            let task = tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    if let McpClientEvent::Elicitation(event) = event {
                        recorder
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(CapturedElicitation { server_name: event.server_name, request: event.request });
                        let response =
                            responses.pop_front().unwrap_or_else(|| ElicitResult::new(ElicitationAction::Cancel));
                        let _ = event.response_sender.send(response);
                    }
                }
            });
            Self { captured, task }
        }

        pub fn captured(&self) -> Vec<CapturedElicitation> {
            self.captured.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }
    }

    impl Drop for ElicitationScript {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
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
