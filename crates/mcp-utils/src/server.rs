use futures::future::BoxFuture;
use rmcp::service::{DynService, ServerInitializeError, serve_server_with_ct};
use rmcp::transport::{IntoTransport, stdio};
use rmcp::{RoleServer, ServerHandler, Service};
use std::env::{temp_dir, var_os};
use std::fs::{Permissions, create_dir_all, remove_dir, remove_file, set_permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fmt, io};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixListener;
use tokio::task::JoinError;
use tokio_util::sync::{CancellationToken, DropGuard};
use uuid::Uuid;

#[derive(Clone)]
pub struct McpServer {
    service: Arc<dyn Fn() -> Box<dyn DynService<RoleServer>> + Send + Sync>,
}

#[derive(Debug)]
pub struct ServerHandle {
    connections: CancellationToken,
    _stop_on_drop: DropGuard,
    socket: SocketPath,
}

#[derive(Debug)]
pub struct UnixSocket {
    listener: UnixListener,
    socket: SocketPath,
}

#[derive(Debug, Error)]
pub enum ServeError {
    #[error("Failed to create MCP socket directory {}: {source}", path.display())]
    SocketDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Failed to bind MCP socket {}: {source}", path.display())]
    Bind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("MCP server initialization failed: {0}")]
    Init(#[source] Box<ServerInitializeError>),
    #[error("MCP server task failed: {0}")]
    Task(#[source] JoinError),
}

impl McpServer {
    pub fn new(handler: impl ServerHandler) -> Self {
        let handler = Arc::new(handler);
        Self { service: Arc::new(move || Box::new(Arc::clone(&handler)) as Box<dyn DynService<RoleServer>>) }
    }

    pub async fn serve_stdio(self) -> Result<(), ServeError> {
        host(self.service(), stdio(), CancellationToken::new()).await
    }

    pub fn serve_unix(self, socket: UnixSocket) -> ServerHandle {
        let UnixSocket { listener, socket } = socket;
        let connections = CancellationToken::new();
        tokio::spawn(accept_connections(listener, self, connections.clone()));
        ServerHandle { _stop_on_drop: connections.clone().drop_guard(), connections, socket }
    }

    #[cfg(feature = "client")]
    pub(crate) fn serve_in_memory(&self) -> tokio::io::DuplexStream {
        let (client, server) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_connection(self.service(), server, CancellationToken::new()));
        client
    }

    fn service(&self) -> Box<dyn DynService<RoleServer>> {
        (self.service)()
    }
}

impl<H: ServerHandler> From<H> for McpServer {
    fn from(handler: H) -> Self {
        Self::new(handler)
    }
}

impl fmt::Debug for McpServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("McpServer").finish_non_exhaustive()
    }
}

impl ServerHandle {
    pub fn path(&self) -> &Path {
        self.socket.path()
    }

    /// Stops accepting connections, ends the open ones, and removes the socket, as dropping the handle does.
    pub fn stop(&self) {
        self.connections.cancel();
        self.socket.remove();
    }
}

impl UnixSocket {
    pub fn bind() -> Result<Self, ServeError> {
        let socket = SocketPath::allocate()?;
        let listener = UnixListener::bind(socket.path())
            .map_err(|source| ServeError::Bind { path: socket.path().to_path_buf(), source })?;
        Ok(Self { listener, socket })
    }

    pub fn path(&self) -> &Path {
        self.socket.path()
    }
}

#[derive(Debug)]
struct SocketPath {
    directory: PathBuf,
    socket: PathBuf,
}

impl SocketPath {
    fn allocate() -> Result<Self, ServeError> {
        let short_id = &Uuid::new_v4().simple().to_string()[..8];
        let runtime_dir =
            var_os("XDG_RUNTIME_DIR").filter(|path| Path::new(path).is_absolute()).map_or_else(temp_dir, PathBuf::from);
        let directory = runtime_dir.join("aether").join(format!("aether-{short_id}"));
        create_dir_all(&directory).map_err(|source| ServeError::SocketDirectory { path: directory.clone(), source })?;
        let path = Self { socket: directory.join("ipc.sock"), directory };
        set_permissions(&path.directory, Permissions::from_mode(0o700))
            .map_err(|source| ServeError::SocketDirectory { path: path.directory.clone(), source })?;
        Ok(path)
    }

    fn path(&self) -> &Path {
        &self.socket
    }

    fn remove(&self) {
        let _ = remove_file(&self.socket);
        let _ = remove_dir(&self.directory);
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        self.remove();
    }
}

async fn accept_connections(listener: UnixListener, server: McpServer, cancellation: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            () = cancellation.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else { break };
        tokio::spawn(serve_connection(server.service(), stream, cancellation.child_token()));
    }
}

async fn serve_connection<S>(service: Box<dyn DynService<RoleServer>>, stream: S, cancellation: CancellationToken)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    if let Err(error) = host(service, stream, cancellation).await {
        tracing::debug!(%error, "MCP server connection ended with an error");
    }
}

fn host<T, U, V, X>(
    service: T,
    transport: U,
    cancellation: CancellationToken,
) -> BoxFuture<'static, Result<(), ServeError>>
where
    T: Service<RoleServer>,
    U: IntoTransport<RoleServer, V, X>,
    V: std::error::Error + Send + Sync + 'static,
{
    Box::pin(async move {
        let running = serve_server_with_ct(service, transport, cancellation)
            .await
            .map_err(|error| ServeError::Init(Box::new(error)))?;
        running.waiting().await.map_err(ServeError::Task)?;
        Ok(())
    })
}
