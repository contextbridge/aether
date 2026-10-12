pub use elicitation_script::{
    CapturedElicitation, ElicitationScript, ElicitationScriptBuilder, elicitation_host, elicitation_sink,
};
pub use fake_auth_server::{
    ACCESS_TOKEN, CLIENT_METADATA_URL, FakeAuthServer, OAuthPrompt, client_metadata, complete_authorization,
};
pub use fake_http_server::{FakeHttpServer, RecordedRequest};
pub use fake_mcp::{
    CapturedTaskUpdate, CapturedToolCall, FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse,
    completed_task_payload,
};
pub use fake_task::{
    FakeTask, accept_with_name, cancelled_task, completed_task, failed_task, form_request, input_requests,
    input_required_task, name_input, task, url_request, working_task,
};
pub use gate::Gate;

mod elicitation_script;
mod fake_auth_server;
mod fake_http_server;
mod fake_mcp;
mod fake_task;
mod gate;

use crate::client::{ClientOptions, McpClient, ToolCallError, ToolCallOptions, Transport};
use crate::gateway::McpGateway;
use crate::model::CallToolResult;
use crate::server::McpServer;
use futures::StreamExt;
use serde_json::{Map, Value};

/// Connects an [`McpClient`] named `name` to an in-process `server`.
pub async fn connect(name: &str, server: impl Into<McpServer>, options: &ClientOptions) -> McpClient {
    McpClient::connect(name, Transport::InProcess(server.into()), options).await.expect("connect in-process server")
}

/// Calls `name` with default options and waits for its result.
pub async fn call(client: &McpClient, name: &str, arguments: Value) -> Result<CallToolResult, ToolCallError> {
    client.call_tool(name, args(arguments), ToolCallOptions::default()).result().await
}

pub async fn wait_until(gateway: &McpGateway, condition: impl Fn(&McpGateway) -> bool) {
    let mut changes = gateway.changes();
    while !condition(gateway) {
        changes.next().await.expect("the gateway stopped before the condition held");
    }
}

pub fn args(value: Value) -> Map<String, Value> {
    let Value::Object(arguments) = value else { panic!("tool arguments must be a JSON object") };
    arguments
}
