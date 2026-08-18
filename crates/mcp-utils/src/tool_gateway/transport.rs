use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::{RoleClient, RoleServer, ServiceExt};
use std::env::{temp_dir, var_os};
use std::fs::{Permissions, create_dir_all, remove_dir};
use std::fs::{remove_file, set_permissions};
use std::{
    io,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener as StdUnixListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum UnixSocketTransportError {
    #[error("failed to create MCP socket directory: {0}")]
    CreateDirectory(#[source] io::Error),
    #[error("failed to bind MCP socket: {0}")]
    Bind(#[source] io::Error),
    #[error("MCP socket path must be absolute")]
    NotAbsolute,
    #[error("MCP socket path is not valid UTF-8")]
    InvalidPath,
}

/// A session endpoint allocated by this process, or inherited from the session environment.
#[derive(Debug)]
pub struct UnixSocketPath {
    directory: PathBuf,
    socket: PathBuf,
    /// Only the allocating process owns endpoint removal; inherited paths never remove.
    remove_on_drop: bool,
}

impl UnixSocketPath {
    pub fn new() -> Result<Self, UnixSocketTransportError> {
        let short_id = &Uuid::new_v4().simple().to_string()[..8];
        let runtime_dir =
            var_os("XDG_RUNTIME_DIR").filter(|path| Path::new(path).is_absolute()).map_or_else(temp_dir, PathBuf::from);
        let socket_dir = runtime_dir.join("aether").join(format!("aether-{short_id}"));
        let socket = socket_dir.join("ipc.sock");
        create_dir_all(&socket_dir).map_err(UnixSocketTransportError::CreateDirectory)?;
        set_permissions(&socket_dir, Permissions::from_mode(0o700))
            .map_err(UnixSocketTransportError::CreateDirectory)?;
        Ok(Self { directory: socket_dir, socket, remove_on_drop: true })
    }

    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self, UnixSocketTransportError> {
        let socket = path.into();
        if !socket.is_absolute() {
            return Err(UnixSocketTransportError::NotAbsolute);
        }
        let directory = socket.parent().ok_or(UnixSocketTransportError::InvalidPath)?.to_path_buf();
        Ok(Self { directory, socket, remove_on_drop: false })
    }

    pub fn path(&self) -> &Path {
        &self.socket
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

impl Drop for UnixSocketPath {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = remove_file(&self.socket);
            let _ = remove_dir(&self.directory);
        }
    }
}

pub struct UnixSocketMcpTransport {
    path: UnixSocketPath,
    listener: UnixListener,
}

impl UnixSocketMcpTransport {
    pub fn bind(path: UnixSocketPath) -> Result<Self, UnixSocketTransportError> {
        let _ = remove_file(path.path());
        let listener = StdUnixListener::bind(path.path()).map_err(UnixSocketTransportError::Bind)?;
        listener.set_nonblocking(true).map_err(UnixSocketTransportError::Bind)?;
        let listener = UnixListener::from_std(listener).map_err(UnixSocketTransportError::Bind)?;
        Ok(Self { path, listener })
    }

    pub fn path(&self) -> &Path {
        self.path.path()
    }

    /// Serve connections until the returned server is dropped.
    pub fn spawn<T>(self, server: T) -> UnixSocketServer
    where
        T: Clone + ServiceExt<RoleServer> + Send + 'static,
    {
        let Self { path, listener } = self;
        let cancellation = CancellationToken::new();
        let accept_cancellation = cancellation.clone();
        let connections = Arc::new(Mutex::new(JoinSet::new()));
        let accept_connections = Arc::clone(&connections);
        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    () = accept_cancellation.cancelled() => break,
                    result = listener.accept() => result,
                };
                let Ok((stream, _)) = accepted else { break };
                let server = server.clone();
                let connection_cancellation = accept_cancellation.clone();
                accept_connections.lock().unwrap().spawn(async move {
                    match server.serve(stream).await {
                        Ok(running) => {
                            let service_cancellation = running.cancellation_token();
                            let mut waiting = Box::pin(running.waiting());
                            tokio::select! {
                                () = connection_cancellation.cancelled() => service_cancellation.cancel(),
                                _ = &mut waiting => {}
                            }
                        }
                        Err(error) => tracing::debug!(%error, "MCP Unix socket client ended during initialization"),
                    }
                });
            }
        });
        UnixSocketServer { path, cancellation, task, connections }
    }
}

/// Owns the accept task, its connection tasks, and the session endpoint.
/// Dropping it cancels in-flight connections and removes the endpoint.
pub struct UnixSocketServer {
    path: UnixSocketPath,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
    connections: Arc<Mutex<JoinSet<()>>>,
}

impl UnixSocketServer {
    pub fn path(&self) -> &Path {
        self.path.path()
    }
}

impl Drop for UnixSocketServer {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
        self.connections.lock().unwrap().detach_all();
    }
}

/// Connect an rmcp client to an inherited session endpoint.
pub async fn connect(
    path: impl AsRef<Path>,
) -> io::Result<AsyncRwTransport<RoleClient, ReadHalf<UnixStream>, WriteHalf<UnixStream>>> {
    let stream = UnixStream::connect(path).await?;
    let (read, write) = tokio::io::split(stream);
    Ok(AsyncRwTransport::new_client(read, write))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::ServerHandler;
    use rmcp::handler::server::router::tool::ToolRouter;
    use rmcp::model::{ServerCapabilities, ServerInfo};
    use rmcp::{tool, tool_handler, tool_router};

    #[derive(Clone)]
    struct TestServer {
        tool_router: ToolRouter<Self>,
    }

    #[tool_router]
    impl TestServer {}

    #[tool_handler(router = self.tool_router)]
    impl ServerHandler for TestServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }
    }

    #[test]
    fn allocated_endpoint_is_removed_on_drop_without_binding() {
        let path = UnixSocketPath::new().unwrap();
        let directory = path.directory().to_path_buf();
        assert!(directory.exists());
        drop(path);
        assert!(!directory.exists());
    }

    #[test]
    fn inherited_endpoint_is_not_removed_on_drop() {
        let path = UnixSocketPath::new().unwrap();
        let directory = path.directory().to_path_buf();
        let inherited = UnixSocketPath::from_path(path.path()).unwrap();
        drop(inherited);
        assert!(directory.exists());
        drop(path);
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn allocated_endpoint_is_private_and_removed_with_transport() {
        let path = UnixSocketPath::new().unwrap();
        let directory = path.directory().to_path_buf();
        let transport = UnixSocketMcpTransport::bind(path).unwrap();
        assert_eq!(std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(transport.path().exists());
        drop(transport);
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn spawned_endpoint_is_removed_with_server() {
        let path = UnixSocketPath::new().unwrap();
        let directory = path.directory().to_path_buf();
        let transport = UnixSocketMcpTransport::bind(path).unwrap();
        let socket = transport.path().to_path_buf();
        let server = transport.spawn(TestServer { tool_router: TestServer::tool_router() });
        assert!(socket.exists());
        drop(server);
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn connected_client_completes_initialization() {
        let path = UnixSocketPath::new().unwrap();
        let transport = UnixSocketMcpTransport::bind(path).unwrap();
        let socket = transport.path().to_path_buf();
        let _server = transport.spawn(TestServer { tool_router: TestServer::tool_router() });
        let _client = ().serve(connect(&socket).await.unwrap()).await.unwrap();
    }

    #[tokio::test]
    async fn dropping_server_cancels_in_flight_connections() {
        use rmcp::model::CallToolRequestParams;
        use std::time::Duration;
        use tokio::sync::watch;

        #[derive(Clone)]
        struct SlowServer {
            tool_router: ToolRouter<Self>,
            started: watch::Sender<bool>,
        }

        #[tool_router]
        impl SlowServer {
            #[tool(description = "Blocks until the connection is aborted")]
            async fn slow(&self) -> String {
                let _ = self.started.send(true);
                tokio::time::sleep(Duration::from_mins(10)).await;
                "done".to_string()
            }
        }

        #[tool_handler(router = self.tool_router)]
        impl ServerHandler for SlowServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
        }

        let path = UnixSocketPath::new().unwrap();
        let transport = UnixSocketMcpTransport::bind(path).unwrap();
        let socket = transport.path().to_path_buf();
        let (started_tx, mut started_rx) = watch::channel(false);
        let server = transport.spawn(SlowServer { tool_router: SlowServer::tool_router(), started: started_tx });

        let client = ().serve(connect(&socket).await.unwrap()).await.unwrap();
        let call = tokio::spawn(async move {
            let _ = client.call_tool_once(CallToolRequestParams::new("slow")).await;
        });
        started_rx.changed().await.unwrap();

        drop(server);
        call.await.unwrap();
    }
}
