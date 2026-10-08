pub use elicitation_script::{CapturedElicitation, ElicitationScript, ElicitationScriptBuilder, elicitation};
pub use fake_mcp::{
    CapturedTaskUpdate, CapturedToolCall, FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse,
    completed_task_payload, fake_mcp,
};

mod elicitation_script;
mod fake_mcp;

use crate::McpError;
use crate::client::{ClientOptions, McpClient, Transport, client_lifecycle_mode};
use crate::server::McpServer;
use rmcp::model::ClientConfig;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServerHandler, serve_client_with_lifecycle};
use serde_json::{Map, Value};

/// Connects an [`McpClient`] named `name` to an in-process `server`.
pub async fn connect(name: &str, server: impl Into<McpServer>, options: &ClientOptions) -> McpClient {
    McpClient::connect(name, Transport::InProcess(server.into()), options).await.expect("connect in-process server")
}

/// The JSON object `value` as tool-call arguments.
pub fn args(value: Value) -> Map<String, Value> {
    let Value::Object(arguments) = value else { panic!("tool arguments must be a JSON object") };
    arguments
}

/// A bare rmcp client session, for tests that drive the protocol below [`McpClient`]
/// (raw task polling, unanswered `input_required` results, custom client info).
pub struct RawClient {
    client: RunningService<RoleClient, ClientConfig>,
}

impl RawClient {
    pub async fn connect(server: impl ServerHandler, info: ClientConfig) -> Result<Self, McpError> {
        let stream = McpServer::new(server).serve_in_memory();
        let client = serve_client_with_lifecycle(info, stream, client_lifecycle_mode())
            .await
            .map_err(|source| McpError::connect("raw", source))?;
        Ok(Self { client })
    }

    pub fn raw(&self) -> &RunningService<RoleClient, ClientConfig> {
        &self.client
    }
}
