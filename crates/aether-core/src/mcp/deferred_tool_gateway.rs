use super::McpCommandClient;
use mcp_utils::tool_gateway::{UnixSocketMcpHandle, UnixSocketMcpTransport, UnixSocketPath};
use std::ffi::OsString;
use std::io;

pub struct DeferredToolGateway {
    transport: UnixSocketMcpTransport,
}

pub struct DeferredToolGatewayHandle {
    endpoint: UnixSocketPath,
    handle: UnixSocketMcpHandle,
}

impl DeferredToolGateway {
    pub fn bind() -> io::Result<Self> {
        UnixSocketMcpTransport::bind().map(|transport| Self { transport })
    }

    pub fn endpoint(&self) -> UnixSocketPath {
        self.transport.endpoint()
    }

    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        self.transport.environment()
    }

    pub fn start(self, client: McpCommandClient) -> DeferredToolGatewayHandle {
        let endpoint = self.transport.endpoint();
        DeferredToolGatewayHandle { endpoint, handle: self.transport.start(client) }
    }
}

impl DeferredToolGatewayHandle {
    pub fn endpoint(&self) -> &UnixSocketPath {
        &self.endpoint
    }

    pub fn is_running(&self) -> bool {
        !self.handle.is_finished()
    }
}
