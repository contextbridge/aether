use rmcp::RoleClient;
use rmcp::RoleServer;
use rmcp::service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};
use rmcp::transport::Transport;
use std::future::Future;
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum InMemoryTransportError {
    #[error("Channel closed")]
    ChannelClosed,
}

/// In-memory transport for connecting `McpServer` and `McpClient` in tests
pub struct InMemoryTransport<T: ServiceRole> {
    tx: mpsc::Sender<TxJsonRpcMessage<T>>,
    rx: mpsc::Receiver<RxJsonRpcMessage<T>>,
}

impl<T: ServiceRole> InMemoryTransport<T> {
    fn new(tx: mpsc::Sender<TxJsonRpcMessage<T>>, rx: mpsc::Receiver<RxJsonRpcMessage<T>>) -> Self {
        Self { tx, rx }
    }
}

/// Create a pair of transports for client and server
pub fn create_in_memory_transport() -> (InMemoryTransport<RoleClient>, InMemoryTransport<RoleServer>) {
    // Client sends ClientRequest/ClientResult, receives ServerRequest/ServerResult
    // Server sends ServerRequest/ServerResult, receives ClientRequest/ClientResult
    let (client_tx, server_rx) = mpsc::channel(1000); // Client -> Server
    let (server_tx, client_rx) = mpsc::channel(1000); // Server -> Client

    let client_transport = InMemoryTransport::new(client_tx, client_rx);
    let server_transport = InMemoryTransport::new(server_tx, server_rx);

    (client_transport, server_transport)
}

impl<R: ServiceRole> Transport<R> for InMemoryTransport<R> {
    type Error = InMemoryTransportError;

    fn send(&mut self, item: TxJsonRpcMessage<R>) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let tx = self.tx.clone();
        async move { tx.send(item).await.map_err(|_| InMemoryTransportError::ChannelClosed) }
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<R>>> + Send {
        async move { self.rx.recv().await }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        // Channels will be closed when dropped
        std::future::ready(Ok(()))
    }
}
