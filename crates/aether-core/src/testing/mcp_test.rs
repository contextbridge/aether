use crate::events::TraceContext;
use crate::mcp::mcp;
use crate::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use mcp_utils::client::{McpConnectionDetails, McpServer, McpTransport};
use mcp_utils::testing::ElicitationScript;
use rmcp::ServerHandler;
use rmcp::model::{ElicitResult, ProgressNotificationParam};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

pub use mcp_utils::testing::CapturedElicitation;

const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct McpTestBuilder {
    servers: Vec<McpServer>,
    elicitation_responses: Vec<ElicitResult>,
    trace_context: Option<TraceContext>,
}

pub struct McpTest {
    command_tx: mpsc::Sender<McpCommand>,
    snapshot: McpConnectionDetails,
    elicitations: ElicitationScript,
    trace_context: Option<TraceContext>,
    next_call_id: AtomicU64,
    _aether_home: tempfile::TempDir,
}

pub struct ToolCallOutcome {
    pub result: Result<llm::ToolCallResult, llm::ToolCallError>,
    pub progress: Vec<ProgressNotificationParam>,
}

impl McpTestBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn server<S>(self, name: impl Into<String>, server: S) -> Self
    where
        S: ServerHandler,
    {
        self.add_server(name, server, false)
    }

    pub fn proxy_server<S>(self, name: impl Into<String>, server: S) -> Self
    where
        S: ServerHandler,
    {
        self.add_server(name, server, true)
    }

    pub fn elicitation_response(mut self, response: ElicitResult) -> Self {
        self.elicitation_responses.push(response);
        self
    }

    pub fn trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }

    pub async fn build(self) -> McpTest {
        let aether_home = tempfile::tempdir().expect("temp aether home is created");
        let mut spawn = mcp("/workspace")
            .with_aether_home(aether_home.path())
            .with_servers(self.servers)
            .spawn()
            .await
            .expect("MCP test manager spawns");
        let snapshot = spawn.block_until_ready().await.expect("MCP test manager becomes ready");

        McpTest {
            command_tx: spawn.command_tx,
            snapshot,
            elicitations: ElicitationScript::spawn(spawn.event_rx, self.elicitation_responses),
            trace_context: self.trace_context,
            next_call_id: AtomicU64::new(1),
            _aether_home: aether_home,
        }
    }

    fn add_server<S>(mut self, name: impl Into<String>, server: S, proxy: bool) -> Self
    where
        S: ServerHandler,
    {
        self.servers.push(McpServer::new(name, McpTransport::InMemory { server: Box::new(server) }, proxy));
        self
    }
}

impl McpTest {
    pub async fn call(&self, server: &str, tool: &str, arguments: Value) -> ToolCallOutcome {
        let id = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        let request = llm::ToolCallRequest {
            id: format!("mcp-test-{id}"),
            name: format!("{server}__{tool}"),
            arguments: arguments.to_string(),
        };
        let (event_tx, mut event_rx) = mpsc::channel(64);
        self.command_tx
            .send(McpCommand::ExecuteTool {
                request,
                trace_context: self.trace_context.clone(),
                timeout: TOOL_TIMEOUT,
                tx: event_tx,
            })
            .await
            .expect("MCP test manager accepts tool call");

        let mut progress = Vec::new();
        while let Some(event) = event_rx.recv().await {
            match event {
                ToolExecutionEvent::Progress { progress: event, .. } => progress.push(event),
                ToolExecutionEvent::Complete { result, .. } => return ToolCallOutcome { result, progress },
            }
        }
        panic!("MCP test tool event stream ended before completion");
    }

    pub fn snapshot(&self) -> &McpConnectionDetails {
        &self.snapshot
    }

    pub fn elicitations(&self) -> Vec<CapturedElicitation> {
        self.elicitations.captured()
    }
}
