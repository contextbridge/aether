use super::GatewayError;
use super::connection::Connection;
use crate::client::{ClientOptions, McpClient, ToolCall, ToolCallOptions, Transport};
use crate::server::McpServer;
use futures::future::join_all;
use futures::stream::select_all;
use futures::{Stream, StreamExt};
use rmcp::model::{GetPromptResult, Prompt, Tool};
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio_stream::wrappers::WatchStream;
use tokio_util::sync::{CancellationToken, DropGuard};
use tokio_util::task::TaskTracker;
use utils::mcp_status::McpServerStatusEntry;

#[derive(Clone)]
pub struct McpGateway {
    connections: Arc<[Connection]>,
    stop: CancellationToken,
    tasks: TaskTracker,
    _stop_on_drop: Arc<DropGuard>,
}

#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub name: String,
    pub transport: Transport,
}

#[derive(Debug, Clone)]
pub struct ConnectedServer {
    client: McpClient,
    tools: Arc<[Tool]>,
}

#[derive(Debug, Clone)]
pub struct ServerPrompt {
    pub server: String,
    pub prompt: Prompt,
}

impl McpGateway {
    /// Connects to every server in `specs`, which must have unique names.
    pub fn new(specs: Vec<ServerSpec>, options: &ClientOptions) -> Self {
        let stop = CancellationToken::new();
        let tasks = TaskTracker::new();
        let connections =
            specs.into_iter().map(|spec| Connection::spawn(spec, options, stop.clone(), &tasks)).collect();

        tasks.close();
        Self { connections, _stop_on_drop: Arc::new(stop.clone().drop_guard()), stop, tasks }
    }

    pub async fn ready(&self) {
        join_all(self.connections().iter().map(Connection::settled)).await;
    }

    pub fn changes(&self) -> impl Stream<Item = ()> + Send + Sync + Unpin + use<> {
        select_all(self.connections.iter().map(|connection| WatchStream::from_changes(connection.subscribe())))
            .map(|_| ())
    }

    pub fn servers(&self) -> Vec<ConnectedServer> {
        self.connections().iter().filter_map(Connection::connected).collect()
    }

    pub fn statuses(&self) -> Vec<McpServerStatusEntry> {
        self.connections().iter().map(Connection::status).collect()
    }

    pub async fn list_prompts(&self) -> Vec<ServerPrompt> {
        let listed = join_all(self.servers().into_iter().map(|server| async move {
            match server.client.list_prompts().await {
                Ok(prompts) => prompts
                    .into_iter()
                    .map(|prompt| ServerPrompt { server: server.name().to_string(), prompt })
                    .collect(),
                Err(error) => {
                    tracing::warn!(server = server.name(), %error, "Failed to list MCP prompts");
                    Vec::new()
                }
            }
        }))
        .await;
        listed.into_iter().flatten().collect()
    }

    pub async fn get_prompt(
        &self,
        server: &str,
        prompt: &str,
        args: Map<String, Value>,
    ) -> Result<GetPromptResult, GatewayError> {
        let connected = self.server(server).ok_or_else(|| GatewayError::ServerNotFound(server.to_string()))?;
        Ok(connected.client.get_prompt(prompt, args).await?)
    }

    pub async fn authenticate(&self, server: &str) -> Result<(), GatewayError> {
        if self.stop.is_cancelled() {
            return Err(GatewayError::Closed);
        }
        self.connection(server).ok_or_else(|| GatewayError::ServerNotFound(server.to_string()))?.sign_in().await
    }

    pub async fn shutdown(&self) {
        self.stop.cancel();
        self.tasks.wait().await;
    }

    pub fn server(&self, name: &str) -> Option<ConnectedServer> {
        self.connection(name)?.connected()
    }

    fn connections(&self) -> &[Connection] {
        if self.stop.is_cancelled() { &[] } else { &self.connections }
    }

    fn connection(&self, name: &str) -> Option<&Connection> {
        self.connections().iter().find(|connection| connection.name() == name)
    }
}

impl ServerSpec {
    pub fn in_process(name: impl Into<String>, server: impl Into<McpServer>) -> Self {
        Self { name: name.into(), transport: Transport::InProcess(server.into()) }
    }
}

impl ConnectedServer {
    pub fn name(&self) -> &str {
        self.client.name()
    }

    pub fn description(&self) -> Option<String> {
        self.client.description()
    }

    pub fn instructions(&self) -> Option<String> {
        self.client.instructions()
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub fn call_tool(&self, name: &str, args: Map<String, Value>, options: ToolCallOptions) -> ToolCall {
        self.client.call_tool(name, args, options)
    }

    pub(super) fn new(client: McpClient, tools: Vec<Tool>) -> Self {
        Self { client, tools: tools.into() }
    }

    pub(super) fn client(&self) -> &McpClient {
        &self.client
    }
}
