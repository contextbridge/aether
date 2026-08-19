use crate::mcp::{McpBuilder, ServerFactory};
use futures::FutureExt;
use mcp_utils::client::{InMemoryServerSpec, McpServer, McpTransport, ToolExposure};
use rmcp::{RoleServer, service::DynService};

pub use mcp_utils::testing::{
    CapturedTaskUpdate, CapturedToolCall, FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse, fake_mcp,
};

pub trait McpBuilderTestExt {
    fn with_fake_mcp(self, name: impl Into<String>, server: FakeMcpServer) -> Self;
}

impl McpBuilderTestExt for McpBuilder {
    fn with_fake_mcp(self, name: impl Into<String>, server: FakeMcpServer) -> Self {
        let name = name.into();
        let factory_name = name.clone();
        let factory: ServerFactory = Box::new(move |_spec, _services| {
            let server = server.clone();
            async move { Box::new(server) as Box<dyn DynService<RoleServer>> }.boxed()
        });
        self.register_in_memory_server(factory_name.clone(), factory).with_servers(vec![McpServer::new(
            name,
            McpTransport::InMemory {
                spec: InMemoryServerSpec { factory: factory_name, args: Vec::new(), input: None },
            },
            ToolExposure::ModelVisible,
        )])
    }
}
