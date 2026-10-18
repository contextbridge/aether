use crate::Displayed;
use crate::setup::agent_mcp;
use crate::subagents::SubAgentProgressSink;
use aether_core::{
    agent_spec::AgentSpec,
    core::{AgentBuilder, AgentDeps, AgentError, AgentHandle, AgentRegistryError, recv_agent_event},
    events::{AgentEvent, Command, MessageEvent, SubAgentProgressPayload, TurnEvent, TurnOutcome, UserCommand},
    mcp::{McpCatalog, McpSpawnError},
};
use futures::FutureExt;
use llm::{ContentBlock, MessageId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::timeout;
use utils::display_meta::ToolDisplayMeta;
use utils::panic::panic_message;

/// Input for a single agent task within a batch
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentTask {
    /// Name of the agent to spawn (must exist in project settings and be agent-invocable)
    #[serde(alias = "agent_name")]
    pub agent_name: String,
    /// Task for the agent to perform
    pub prompt: String,
}

/// Input for spawning sub-agents
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSubAgentsInput {
    /// Array of agent tasks to execute in parallel
    pub tasks: Vec<SubAgentTask>,
    /// Run sub-agents in the background, so you can continue work without waiting for them to complete.
    #[serde(default, alias = "run_in_background", skip_serializing_if = "std::ops::Not::not")]
    pub run_in_background: bool,
}

/// Status of a sub-agent execution
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SubAgentStatus {
    Success,
    Error,
}

/// Result from a single sub-agent
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentResult {
    /// Unique identifier for this task
    pub task_id: String,
    /// Name of the agent that executed
    pub agent_name: String,
    /// Status of execution
    pub status: SubAgentStatus,
    /// Raw output from the sub-agent (present on success)
    pub output: Option<String>,
    /// Error message if status is Error
    pub error: Option<String>,
}

impl SubAgentResult {
    fn new(task_id: String, agent_name: String, result: Result<String, String>) -> Self {
        let (status, output, error) = match result {
            Ok(output) => (SubAgentStatus::Success, Some(output), None),
            Err(error) => (SubAgentStatus::Error, None, Some(error)),
        };
        Self { task_id, agent_name, status, output, error }
    }
}

/// Output from spawning sub-agents
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSubAgentsOutput {
    /// Results from all spawned agents (order matches input tasks)
    pub results: Vec<SubAgentResult>,
    /// Number of successful completions
    pub success_count: usize,
    /// Number of failures
    pub error_count: usize,
}

/// Prompt instructions appended to sub-agent prompts to ensure structured output
pub const STRUCTURED_OUTPUT_INSTRUCTIONS: &str = include_str!("structured_output_instructions.md");

/// Executor for spawning and running sub-agents
#[derive(Clone)]
pub struct AgentExecutor {
    progress: Option<SubAgentProgressSink>,
    project_root: PathBuf,
    deps: AgentDeps,
}

impl AgentExecutor {
    /// Create a new `AgentExecutor` rooted at `project_root`. The agents it may
    /// spawn come from `deps`, so nested delegation sees the same catalog.
    pub fn new(project_root: PathBuf, deps: AgentDeps) -> Self {
        Self { progress: None, project_root, deps }
    }

    /// Relay every sub-agent event to `sink` while tasks run.
    pub fn with_progress(mut self, sink: SubAgentProgressSink) -> Self {
        self.progress = Some(sink);
        self
    }

    /// Execute multiple agent tasks in parallel and return results
    pub async fn execute_tasks(&self, tasks: Vec<SubAgentTask>) -> Displayed<SpawnSubAgentsOutput> {
        if tasks.is_empty() {
            let output = SpawnSubAgentsOutput { results: vec![], success_count: 0, error_count: 0 };
            return Displayed { output, meta: None };
        }

        let task_count = tasks.len();
        let first_agent_name = tasks.first().unwrap().agent_name.clone();

        let mut children = JoinSet::new();
        for (index, task) in tasks.into_iter().enumerate() {
            let executor = self.clone();
            let task_id = format!("task_{index}");
            let agent_name = task.agent_name.clone();
            children.spawn(async move {
                let result = AssertUnwindSafe(executor.execute_single(task_id.clone(), task))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|panic| {
                        let error = format!("Sub-agent task panicked: {}", panic_message(&*panic));
                        SubAgentResult::new(task_id, agent_name, Err(error))
                    });
                (index, result)
            });
        }

        let mut indexed_results = Vec::with_capacity(task_count);
        while let Some(joined) = children.join_next().await {
            indexed_results.push(joined.expect("sub-agent tasks are only aborted when the batch future is dropped"));
        }
        indexed_results.sort_unstable_by_key(|(index, _)| *index);
        let results: Vec<SubAgentResult> = indexed_results.into_iter().map(|(_, result)| result).collect();

        let success_count = results.iter().filter(|r| matches!(r.status, SubAgentStatus::Success)).count();
        let error_count = results.len() - success_count;

        let display_meta = ToolDisplayMeta::new("Spawn agent", format!("{first_agent_name} (1/{task_count})"));
        Displayed::new(SpawnSubAgentsOutput { results, success_count, error_count }, display_meta)
    }

    /// Execute a single sub-agent and return its result
    async fn execute_single(&self, task_id: String, task: SubAgentTask) -> SubAgentResult {
        let agent_name = task.agent_name.clone();

        let result: Result<String, SubAgentError> = async {
            let spec = self.deps.agent_registry.resolve_agent_invocable(&task.agent_name)?;

            let mcp = self.spawn_mcp(&spec)?;
            if timeout(MCP_READY_TIMEOUT, mcp.ready()).await.is_err() {
                tracing::warn!(agent = %agent_name, "Starting sub-agent before every MCP server connected");
            }

            let (user_tx, mut agent_rx, _agent) = self.spawn_agent(&spec, mcp).await?;

            let prompt_with_instructions = format!("{}\n\n{}", task.prompt, STRUCTURED_OUTPUT_INSTRUCTIONS);
            user_tx
                .send(Command::UserCommand(UserCommand::Text {
                    message_id: MessageId::new(),
                    content: vec![ContentBlock::text(&prompt_with_instructions)],
                }))
                .await
                .map_err(|_| SubAgentError::Stopped)?;

            self.report(&task_id, &agent_name, &AgentEvent::Turn(TurnEvent::Started { content: vec![] })).await;

            let mut final_output = String::new();

            loop {
                let message = recv_agent_event(&mut agent_rx).await;
                self.report(&task_id, &agent_name, &message).await;

                match &message {
                    AgentEvent::Message(MessageEvent::Text { chunk, is_complete, .. }) if *is_complete => {
                        final_output.clone_from(chunk);
                    }

                    AgentEvent::Turn(TurnEvent::Ended { outcome }) => match outcome {
                        TurnOutcome::Completed => return Ok(final_output),
                        TurnOutcome::Failed { error, .. } => return Err(SubAgentError::Failed(error.clone())),
                        TurnOutcome::Cancelled => return Err(SubAgentError::Cancelled),
                    },

                    _ => {}
                }
            }
        }
        .await;

        SubAgentResult::new(task_id, agent_name, result.map_err(|error| error.to_string()))
    }

    async fn report(&self, task_id: &str, agent_name: &str, event: &AgentEvent) {
        if event.is_stream_delta() {
            return;
        }
        if let Some(sink) = &self.progress {
            let payload = SubAgentProgressPayload {
                task_id: task_id.to_string(),
                agent_name: agent_name.to_string(),
                event: event.clone(),
            };
            sink.send(payload).await;
        }
    }

    fn spawn_mcp(&self, spec: &AgentSpec) -> Result<McpCatalog, SubAgentError> {
        let builder =
            agent_mcp(&self.project_root, self.deps.clone(), spec.tools.clone()).with_config(spec.mcp.clone());
        Ok(builder.spawn()?)
    }

    async fn spawn_agent(
        &self,
        spec: &AgentSpec,
        mcp: McpCatalog,
    ) -> Result<(mpsc::Sender<Command>, mpsc::Receiver<AgentEvent>, AgentHandle), SubAgentError> {
        let builder = AgentBuilder::from_spec(spec, &self.deps).await.map_err(SubAgentError::Build)?;
        builder.mcp(mcp).spawn().await.map_err(SubAgentError::Spawn)
    }
}

#[derive(Debug, Error)]
enum SubAgentError {
    #[error(transparent)]
    Resolve(#[from] AgentRegistryError),
    #[error("Failed to spawn MCP servers: {0}")]
    McpSpawn(#[from] McpSpawnError),
    #[error("Failed to build agent from spec: {0}")]
    Build(#[source] AgentError),
    #[error("Failed to spawn agent: {0}")]
    Spawn(#[source] AgentError),
    #[error("Agent stopped before it received its task")]
    Stopped,
    #[error("Agent error: {0}")]
    Failed(String),
    #[error("Agent cancelled")]
    Cancelled,
}

const MCP_READY_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_task_accepts_snake_case_agent_name() {
        let task: SubAgentTask = serde_json::from_value(serde_json::json!({
            "agent_name": "codebase-explorer",
            "prompt": "Find entrypoints"
        }))
        .unwrap();

        assert_eq!(task.agent_name, "codebase-explorer");
        assert_eq!(task.prompt, "Find entrypoints");
    }
}
