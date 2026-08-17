use super::UnixSocketPath;
use rmcp::{ServerHandler, ServiceExt};
use std::env::{temp_dir, var_os};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::{Path, PathBuf};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub struct UnixSocketMcpTransport {
    listener: UnixListener,
    socket_file: SocketFileGuard,
    endpoint: UnixSocketPath,
}

pub struct UnixSocketMcpHandle {
    _socket_file: SocketFileGuard,
    handle: JoinHandle<()>,
}

impl UnixSocketMcpTransport {
    pub fn bind() -> io::Result<Self> {
        match var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
            Some(runtime_dir) => Self::bind_in(PathBuf::from(runtime_dir).join("aether")),
            None => Self::bind_in(temp_dir()),
        }
    }

    pub fn bind_in(parent: impl AsRef<Path>) -> io::Result<Self> {
        let runtime_dir = create_runtime_directory(parent.as_ref())?;
        let socket_path = runtime_dir.join("ipc.sock");
        let std_listener = StdUnixListener::bind(&socket_path)?;
        std_listener.set_nonblocking(true)?;
        let listener = UnixListener::from_std(std_listener)?;
        let endpoint = UnixSocketPath::new(socket_path.clone());
        Ok(Self { listener, socket_file: SocketFileGuard { socket_path, runtime_dir }, endpoint })
    }

    pub fn endpoint(&self) -> UnixSocketPath {
        self.endpoint.clone()
    }

    pub fn environment(&self) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        self.endpoint.environment()
    }

    pub fn start<T: ServerHandler + Clone>(self, service: T) -> UnixSocketMcpHandle {
        let Self { listener, socket_file, .. } = self;
        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let service = service.clone();
                        tokio::spawn(async move {
                            match service.serve(stream).await {
                                Ok(running) => {
                                    if let Err(error) = running.waiting().await {
                                        tracing::debug!(%error, "Tool gateway MCP connection ended with an error");
                                    }
                                }
                                Err(error) => tracing::debug!(%error, "Tool gateway MCP initialization failed"),
                            }
                        });
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Tool gateway Unix socket accept failed; retrying");
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        });
        UnixSocketMcpHandle { _socket_file: socket_file, handle }
    }
}

impl UnixSocketMcpHandle {
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

impl Drop for UnixSocketMcpHandle {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct SocketFileGuard {
    socket_path: PathBuf,
    runtime_dir: PathBuf,
}

impl Drop for SocketFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_dir(&self.runtime_dir);
    }
}

fn create_runtime_directory(parent: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(parent)?;
    for _ in 0..8 {
        let path = parent.join(format!("aether-{}", &Uuid::new_v4().simple().to_string()[..12]));
        match fs::create_dir(&path) {
            Ok(()) => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "failed to allocate private MCP runtime directory"))
}
