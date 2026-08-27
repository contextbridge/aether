use rmcp::RoleClient;
use rmcp::RoleServer;
use rmcp::service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};
use rmcp::transport::Transport;
use std::future::Future;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};

#[derive(Debug, Error)]
pub enum InMemoryTransportError {
    #[error("Channel closed")]
    ChannelClosed,
}

/// In-memory transport for connecting `McpServer` and `McpClient` in tests
pub struct InMemoryTransport<R: ServiceRole> {
    tx: Arc<Mutex<mpsc::Sender<TxJsonRpcMessage<R>>>>,
    rx: Arc<Mutex<mpsc::Receiver<RxJsonRpcMessage<R>>>>,
}

impl<R: ServiceRole> InMemoryTransport<R> {
    fn new(tx: mpsc::Sender<TxJsonRpcMessage<R>>, rx: mpsc::Receiver<RxJsonRpcMessage<R>>) -> Self {
        Self { tx: Arc::new(Mutex::new(tx)), rx: Arc::new(Mutex::new(rx)) }
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
        async move {
            let tx = tx.lock().await;
            tx.send(item).await.map_err(|_| InMemoryTransportError::ChannelClosed)?;
            Ok(())
        }
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<R>>> + Send {
        let rx = self.rx.clone();
        async move {
            let mut rx = rx.lock().await;
            rx.recv().await
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        // Channels will be closed when dropped
        std::future::ready(Ok(()))
    }
}
