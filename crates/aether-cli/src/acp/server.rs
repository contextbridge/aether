use super::state::{AcpState, ClientGuard};
use super::{AcpArgs, AcpRunError, create_acp_state};
use crate::output::OutputFormat;
use crate::prompt::prompt_or_stdin;
use acp_utils::websocket::WebSocketTransport;
use std::fs::canonicalize;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::{
    handshake::server::{ErrorResponse, Request},
    http::StatusCode,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[derive(clap::Args, Debug)]
pub struct ServerArgs {
    /// Address to listen on. The raw server is unauthenticated; use a private network or authenticating proxy.
    #[arg(long, default_value = "127.0.0.1:8765")]
    pub listen: SocketAddr,

    /// Server workspace used to resolve settings and as the default session directory.
    #[arg(short = 'C', long, default_value = ".")]
    pub cwd: PathBuf,

    /// Initial prompt to run.
    #[arg(long)]
    pub prompt: Option<String>,

    #[command(flatten)]
    pub detached: DetachedArgs,

    #[command(flatten)]
    pub acp: AcpArgs,
}

#[derive(clap::Args, Clone, Debug, Default)]
pub struct DetachedArgs {
    /// Output format for when no clients are attached.
    #[arg(long)]
    pub output: Option<OutputFormat>,

    /// How many seconds to wait after the agent becomes idle before running the --on-idle command.
    #[arg(long, requires = "on_idle")]
    pub idle_after: Option<u64>,

    /// Shell command to spawn when the agent becomes idle.
    #[arg(long, requires = "idle_after")]
    pub on_idle: Option<String>,
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
    #[error("Failed to read the initial prompt from stdin: {0}")]
    PromptStdin(#[source] io::Error),
    #[error("Failed to start the initial session: {0}")]
    InitialSession(#[source] agent_client_protocol::Error),
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

    let prompt = prompt_or_stdin(args.prompt).map_err(ServerRunError::PromptStdin)?;
    let state = Arc::new(create_acp_state(args.acp, &cwd, args.detached)?);
    let server = AcpServer::bind(args.listen, state.clone()).await?;
    info!(address = %args.listen, cwd = %cwd.display(), "Starting Aether ACP WebSocket server");

    let result = async {
        if let Some(prompt) = prompt {
            let session_id = state.start_session(prompt).await.map_err(ServerRunError::InitialSession)?;
            info!(session_id = %session_id.0, "Started initial server session");
        }
        server.run_until(async { shutdown_signal().await.map_err(ServerRunError::Signal) }).await
    }
    .await;
    state.shutdown_all().await;
    result
}

/// Owns networking, not session lifetime. Drop closes the listener and aborts
/// connection tasks; explicit shutdown also waits for output detachment.
pub(crate) struct AcpServer {
    listener: TcpListener,
    state: Arc<AcpState>,
    connections: JoinSet<()>,
    stop: CancellationToken,
}

impl AcpServer {
    pub(crate) async fn bind(address: SocketAddr, state: Arc<AcpState>) -> Result<Self, ServerRunError> {
        let listener = TcpListener::bind(address).await.map_err(|source| ServerRunError::Bind { address, source })?;
        Ok(Self { listener, stop: state.stop_token().child_token(), state, connections: JoinSet::new() })
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub(crate) async fn run_until(
        mut self,
        stop: impl Future<Output = Result<(), ServerRunError>>,
    ) -> Result<(), ServerRunError> {
        let result = tokio::select! {
            result = self.run() => result,
            result = stop => result,
        };
        self.shutdown().await;
        result
    }

    async fn run(&mut self) -> Result<(), ServerRunError> {
        loop {
            tokio::select! {
                biased;
                () = self.stop.cancelled() => return Ok(()),
                result = self.connections.join_next(), if !self.connections.is_empty() => {
                    if let Some(Err(error)) = result {
                        warn!(%error, "ACP connection task failed");
                    }
                }
                accepted = self.listener.accept() => {
                    let (socket, peer) = accepted.map_err(ServerRunError::Accept)?;
                    self.accept(socket, peer);
                }
            }
        }
    }

    /// Close admission and join all networking, leaving the host running.
    pub(crate) async fn shutdown(self) {
        self.stop.cancel();
        drop(self.listener);
        let mut connections = self.connections;
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                warn!(%error, "ACP connection task failed during shutdown");
            }
        }
    }

    fn accept(&mut self, socket: TcpStream, peer: SocketAddr) {
        let stop = self.stop.clone();
        let guard = self.state.try_attach();
        self.connections.spawn(serve_connection(socket, peer, guard, stop));
    }
}

#[expect(clippy::result_large_err, reason = "tungstenite's handshake callback requires an HTTP error response")]
async fn serve_connection(socket: TcpStream, peer: SocketAddr, guard: Option<ClientGuard>, stop: CancellationToken) {
    let handshake = tokio::select! {
        biased;
        () = stop.cancelled() => {
            if let Some(guard) = guard {
                guard.release().await;
            }
            return;
        },
        result = tokio_tungstenite::accept_hdr_async(socket, |_request: &Request, response| {
            if guard.is_some() {
                Ok(response)
            } else {
                let mut error = ErrorResponse::new(Some("client already attached".to_string()));
                *error.status_mut() = StatusCode::CONFLICT;
                Err(error)
            }
        }) => result,
    };
    let socket = match handshake {
        Ok(socket) => socket,
        Err(error) => {
            warn!(%peer, %error, "ACP WebSocket handshake failed");
            if let Some(guard) = guard {
                guard.release().await;
            }
            return;
        }
    };
    if let Err(error) =
        guard.expect("successful handshake owns the client").serve(WebSocketTransport::new(socket), stop).await
    {
        warn!(%peer, %error, "ACP connection failed");
    }
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
