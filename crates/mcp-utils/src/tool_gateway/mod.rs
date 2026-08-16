mod transport;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
pub use transport::{UnixSocketMcpHandle, UnixSocketMcpTransport};

pub const AETHER_MCP_IPC_SOCKET: &str = "AETHER_MCP_IPC_SOCKET";
pub const LIST_SERVERS_TOOL: &str = "_aether_list_servers";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnixSocketPath {
    socket_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolGatewayEndpointParseError {
    #[error("socket path must be a non-empty absolute path")]
    InvalidSocketPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerDescription {
    pub name: String,
    pub description: String,
}

impl UnixSocketPath {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self { socket_path: socket_path.into() }
    }

    pub fn parse(socket_path: impl AsRef<OsStr>) -> Result<Self, ToolGatewayEndpointParseError> {
        let socket_path = PathBuf::from(socket_path.as_ref());
        if !socket_path.is_absolute() || socket_path.as_os_str().is_empty() {
            return Err(ToolGatewayEndpointParseError::InvalidSocketPath);
        }
        Ok(Self::new(socket_path))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        vec![(OsString::from(AETHER_MCP_IPC_SOCKET), self.socket_path.as_os_str().to_owned())]
    }
}
