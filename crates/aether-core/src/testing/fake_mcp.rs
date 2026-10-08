use crate::mcp::{McpBuilder, ServerFactory};
use futures::FutureExt;
use mcp_utils::client::{InMemoryServerSpec, McpServer, McpTransport, ToolExposure};
use rmcp::{RoleServer, service::DynService};

pub use mcp_utils::testing::{
    CapturedTaskUpdate, CapturedToolCall, FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse, fake_mcp,
};

pub trait McpBuilderTestExt {
    /// Registers an in-memory server whose connections are created by `factory` and
    /// exposes it to agents under `name` with model-visible tools.
    fn with_in_memory_mcp(self, name: impl Into<String>, factory: ServerFactory) -> Self;

    /// Registers `server` as an in-memory MCP server exposed to agents under `name`.
    fn with_fake_mcp(self, name: impl Into<String>, server: FakeMcpServer) -> Self;
}

impl McpBuilderTestExt for McpBuilder {
    fn with_in_memory_mcp(self, name: impl Into<String>, factory: ServerFactory) -> Self {
        let name = name.into();
        let spec = InMemoryServerSpec { factory: name.clone(), args: Vec::new(), input: None };
        self.register_in_memory_server(name.clone(), factory).with_servers(vec![McpServer::new(
            name,
            McpTransport::InMemory { spec },
            ToolExposure::ModelVisible,
        )])
    }

    fn with_fake_mcp(self, name: impl Into<String>, server: FakeMcpServer) -> Self {
        self.with_in_memory_mcp(
            name,
            Box::new(move |_spec, _services| {
                let server = server.clone();
                async move { Box::new(server) as Box<dyn DynService<RoleServer>> }.boxed()
            }),
        )
    }
}
