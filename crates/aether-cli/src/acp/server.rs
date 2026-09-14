use super::agent::acp_agent_builder;
use super::state::AcpState;
use super::{AcpArgs, AcpRunError, initialize_host};
use acp_utils::websocket::WebSocketTransport;
use std::fs::canonicalize;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{info, warn};

#[derive(clap::Args, Debug)]
pub struct ServerArgs {
    /// Address to listen on. The raw server is unauthenticated; use a private network or authenticating proxy.
    #[arg(long, default_value = "127.0.0.1:8765")]
    pub listen: SocketAddr,

    /// Server workspace used to resolve settings and as the default session directory.
    #[arg(short = 'C', long, default_value = ".")]
    pub cwd: PathBuf,

    #[command(flatten)]
    pub acp: AcpArgs,
}

#[derive(Debug, Error)]
pub enum ServerRunError {
    #[error("Invalid server workspace {path}: {source}")]
    Workspace { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Initialization(#[from] AcpRunError),
    #[error("Failed to bind ACP listener at {address}: {source}")]
    Bind { address: SocketAddr, source: io::Error },
    #[error("ACP listener failed: {0}")]
    Accept(#[source] io::Error),
    #[error("Failed to listen for shutdown signals: {0}")]
    Signal(#[source] io::Error),
}

pub async fn run_server(args: ServerArgs) -> Result<(), ServerRunError> {
    let cwd = canonicalize(&args.cwd)
        .and_then(|cwd| {
            if cwd.is_dir() {
                Ok(cwd)
            } else {
                Err(io::Error::new(io::ErrorKind::NotADirectory, "workspace must be a directory"))
            }
        })
        .map_err(|source| ServerRunError::Workspace { path: args.cwd, source })?;

    let server = AcpServer::bind(args.listen).await?;

    let state = initialize_host(args.acp, &cwd, Some(cwd.clone()))?;
    info!(address = %args.listen, cwd = %cwd.display(), "Starting Aether ACP WebSocket server");
    let result = server.run(state.clone(), shutdown_signal()).await;
    state.shutdown_all().await;
    result
}

/// Owns the listener and its connection tasks. Dropping the server closes the
/// listener and aborts connections; normal completion also joins those tasks.
pub(crate) struct AcpServer {
    listener: TcpListener,
    admission: Arc<Semaphore>,
    connections: JoinSet<()>,
}

impl AcpServer {
    pub(crate) async fn bind(address: SocketAddr) -> Result<Self, ServerRunError> {
        let listener = TcpListener::bind(address).await.map_err(|source| ServerRunError::Bind { address, source })?;
        Ok(Self { listener, admission: Arc::new(Semaphore::new(1)), connections: JoinSet::new() })
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn admission(&self) -> Arc<Semaphore> {
        self.admission.clone()
    }

    pub(crate) async fn run(
        mut self,
        state: Arc<AcpState>,
        shutdown: impl Future<Output = io::Result<()>>,
    ) -> Result<(), ServerRunError> {
        tokio::pin!(shutdown);
        let result = loop {
            tokio::select! {
                biased;
                result = &mut shutdown => break result.map_err(ServerRunError::Signal),
                result = self.connections.join_next(), if !self.connections.is_empty() => {
                    if let Some(Err(error)) = result {
                        warn!(%error, "ACP connection task failed");
                    }
                }
                accepted = self.listener.accept() => {
                    let (socket, peer) = match accepted {
                        Ok(accepted) => accepted,
                        Err(error) => break Err(ServerRunError::Accept(error)),
                    };
                    let permit = self.admission.clone().try_acquire_owned();
                    let state = state.clone();
                    self.connections.spawn(async move {
                        match permit {
                            Ok(permit) => {
                                serve_connection(socket, state, peer).await;
                                drop(permit);
                            }
                            Err(_) => {
                                if let Err(error) = reject_connection(socket).await {
                                    warn!(%peer, %error, "Failed to reject occupied ACP connection");
                                }
                            }
                        }
                    });
                }
            }
        };

        let Self { listener, mut connections, .. } = self;
        drop(listener);
        connections.abort_all();
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                warn!(%error, "ACP connection task failed during shutdown");
            }
        }
        result
    }
}

async fn serve_connection(socket: TcpStream, state: Arc<AcpState>, peer: SocketAddr) {
    let Ok(socket) = tokio_tungstenite::accept_async(socket).await else {
        warn!(%peer, "ACP WebSocket handshake failed");
        return;
    };
    if let Err(error) = acp_agent_builder(state.clone()).connect_to(WebSocketTransport::new(socket)).await {
        warn!(%peer, %error, "ACP connection failed");
    }
    if let Err(error) = state.detach_client().await {
        warn!(%peer, %error, "Failed to detach ACP client");
    }
}

async fn reject_connection(mut socket: TcpStream) -> io::Result<()> {
    socket
        .write_all(b"HTTP/1.1 409 Conflict\r\nContent-Type: text/plain\r\nContent-Length: 23\r\nConnection: close\r\n\r\nclient already attached")
        .await?;
    socket.shutdown().await
}

async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
