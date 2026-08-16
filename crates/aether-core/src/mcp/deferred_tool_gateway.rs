use super::McpCommandClient;
use mcp_utils::tool_gateway::{UnixSocketMcpHandle, UnixSocketMcpTransport, UnixSocketPath};
use std::ffi::OsString;
use std::io;

pub struct DeferredToolGateway {
    transport: UnixSocketMcpTransport,
}

pub struct DeferredToolGatewayHandle {
    _handle: UnixSocketMcpHandle,
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
        DeferredToolGatewayHandle { _handle: self.transport.start(client) }
    }
}
