use aether_core::events::{AgentEvent, ToolEvent};
use aether_core::mcp::{DeferredToolsClient, McpCatalog};
use aether_core::testing::{TestAgentBuilder, TestScenario};
use llm::testing::llm_response;
use llm::{ToolCallError, ToolCallResult};
use mcp_utils::testing::CapturedElicitation;
use serde_json::Value;

pub const CALL_ID: &str = "call-1";

pub struct ToolRun {
    pub events: Vec<AgentEvent>,
    pub elicitations: Vec<CapturedElicitation>,
}

#[derive(Debug, PartialEq)]
pub enum TaskOutcome {
    Completed(String),
    Failed(String),
    Cancelled,
}

pub async fn deferred_tools(mcp: &McpCatalog) -> Vec<String> {
    let Some(socket) = mcp.deferred_tools_socket() else { return Vec::new() };
    let client = DeferredToolsClient::connect(socket.to_path_buf()).await.expect("connect to the deferred tools");
    let servers = client.servers().await.expect("list the deferred servers");
    servers
        .iter()
        .flat_map(|server| server.tools.iter().map(|tool| format!("{}__{}", server.name, tool.name)))
        .collect()
}

pub async fn call_tool(agent: TestAgentBuilder, tool: &str, arguments: Value) -> ToolRun {
    let arguments = arguments.to_string();
    let result = agent
        .llm_responses(&[
            llm_response().tool_call(CALL_ID, tool, &[&arguments]).build(),
            llm_response().text(&["called"]).build(),
            llm_response().text(&["handled the task outcome"]).build(),
        ])
        .scenario(TestScenario::new().user_text("call the tool").wait_for(finishes_call))
        .run_with_context()
        .await
        .expect("the test agent runs");
    ToolRun { events: result.messages, elicitations: result.elicitations }
}

impl ToolRun {
    pub fn result(&self) -> Result<&ToolCallResult, &ToolCallError> {
        self.tool_events()
            .find_map(|event| match event {
                ToolEvent::Result { result, .. } => Some(Ok(result)),
                ToolEvent::Error { error } => Some(Err(error)),
                _ => None,
            })
            .expect("the tool call finished in the foreground")
    }

    pub fn created_task(&self) -> Option<&str> {
        self.tool_events().find_map(|event| match event {
            ToolEvent::TaskCreated { task_id, .. } => Some(task_id.as_str()),
            _ => None,
        })
    }

    pub fn task_outcome(&self) -> TaskOutcome {
        self.tool_events()
            .find_map(|event| match event {
                ToolEvent::TaskCompleted { result, .. } => Some(TaskOutcome::Completed(result.result.clone())),
                ToolEvent::TaskFailed { error, .. } => Some(TaskOutcome::Failed(error.error.clone())),
                ToolEvent::TaskCancelled { .. } => Some(TaskOutcome::Cancelled),
                _ => None,
            })
            .expect("the tool call started a background task that finished")
    }

    pub fn tool_events(&self) -> impl Iterator<Item = &ToolEvent> {
        self.events.iter().filter_map(|event| match event {
            AgentEvent::Tool(event) => Some(event),
            _ => None,
        })
    }
}

fn finishes_call(event: &AgentEvent) -> bool {
    match event {
        AgentEvent::Tool(ToolEvent::Result { result, .. }) => result.id == CALL_ID,
        AgentEvent::Tool(ToolEvent::Error { error }) => error.id == CALL_ID,
        AgentEvent::Tool(
            ToolEvent::TaskCompleted { request, .. }
            | ToolEvent::TaskFailed { request, .. }
            | ToolEvent::TaskCancelled { request, .. },
        ) => request.id == CALL_ID,
        _ => false,
    }
}
