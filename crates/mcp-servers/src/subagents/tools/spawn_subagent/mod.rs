use crate::setup::McpBuilderExt;
use aether_auth::OAuthCredentialStorage;
use aether_core::{
    agent_spec::McpConfigSource,
    core::{AgentBuilder, AgentHandle, Prompt},
    events::{AgentEvent, Command, MessageEvent, TurnEvent, TurnOutcome, UserCommand},
    mcp::{McpSpawnResult, mcp, run_mcp_task::McpCommand},
};
use aether_project::AgentCatalog;
use llm::ToolDefinition;
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::{spawn, sync::mpsc};

/// Reference to a file artifact discovered or modified by a sub-agent
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactReference {
    /// Absolute file path
    pub path: String,
    /// How this file relates to the task: read, modified, discovered, relevant
    pub relation: String,
    /// Brief note on why this file matters
    pub note: Option<String>,
}

/// Structured output from a sub-agent, designed to prevent information loss
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StructuredAgentOutput {
    /// Brief summary of what the agent accomplished
    pub summary: String,
    /// Files read, modified, or discovered as relevant
    pub artifacts: Vec<ArtifactReference>,
    /// Key decisions, findings, or conclusions
    pub decisions: Vec<String>,
    /// Recommended follow-up tasks
    pub next_steps: Vec<String>,
    /// Full detailed output (optional, for deep dives)
    pub details: Option<String>,
}

impl StructuredAgentOutput {
    /// Parse agent output into structured format with fallback for non-JSON responses
    pub fn parse(raw_output: &str) -> Result<Self, String> {
        // Try to parse as JSON directly
        if let Ok(parsed) = serde_json::from_str::<Self>(raw_output) {
            return Ok(parsed);
        }

        if let Some(json_str) = extract_json_from_markdown(raw_output)
            && let Ok(parsed) = serde_json::from_str::<Self>(&json_str)
        {
            return Ok(parsed);
        }

        // Fallback: wrap raw output in structured format
        Ok(Self {
            summary: "Agent did not return structured output".to_string(),
            artifacts: vec![],
            decisions: vec![],
            next_steps: vec![],
            details: Some(raw_output.to_string()),
        })
    }
}

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
}

/// Status of a sub-agent execution
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SubAgentStatus {
    Success,
    Error,
    Cancelled,
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
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Prompt instructions appended to sub-agent prompts to ensure structured output
pub const STRUCTURED_OUTPUT_INSTRUCTIONS: &str = include_str!("structured_output_instructions.md");

/// Extract JSON from markdown code blocks (```json ... ``` or ``` ... ```)
pub fn extract_json_from_markdown(text: &str) -> Option<String> {
    // Look for ```json ... ``` pattern
    let json_block_start = text.find("```json")?;
    let content_start = json_block_start + 7;
    let remaining = &text[content_start..];

    // Find the closing ```
    let content_end = remaining.find("```")?;
    let json_content = remaining[..content_end].trim();

    if json_content.is_empty() {
        return None;
    }

    Some(json_content.to_string())
}

/// Callback for receiving progress updates during agent execution
pub type ProgressCallback = Box<dyn Fn(&str, &str, &AgentEvent) + Send + Sync>;

/// Executor for spawning and running sub-agents
pub struct AgentExecutor {
    catalog: Arc<AgentCatalog>,
    progress_callback: Option<Arc<ProgressCallback>>,
    project_root: PathBuf,
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
}

impl AgentExecutor {
    /// Create a new `AgentExecutor` with the given agent catalog and project root.
    pub fn new(
        catalog: AgentCatalog,
        project_root: PathBuf,
        oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    ) -> Self {
        Self { catalog: Arc::new(catalog), progress_callback: None, project_root, oauth_credential_store }
    }

    /// Set a callback for receiving progress updates during agent execution
    pub fn with_progress_callback(mut self, callback: ProgressCallback) -> Self {
        self.progress_callback = Some(Arc::new(callback));
        self
    }

    /// Execute multiple agent tasks in parallel and return results
    pub async fn execute_tasks(&self, tasks: Vec<SubAgentTask>) -> SpawnSubAgentsOutput {
        if tasks.is_empty() {
            return SpawnSubAgentsOutput { results: vec![], success_count: 0, error_count: 0, meta: None };
        }

        // Store task count and first task for display metadata
        let task_count = tasks.len();

        // Clone the first task for display metadata (we need to keep the original for execution)
        let first_task = tasks.first().unwrap();
        let first_agent_name = first_task.agent_name.clone();

        let catalog = Arc::clone(&self.catalog);
        let progress_callback = self.progress_callback.clone();
        let project_root = self.project_root.clone();
        let oauth_credential_store = self.oauth_credential_store.clone();
        let handles: Vec<_> = tasks
            .into_iter()
            .enumerate()
            .map(|(i, task)| {
                let task_id = format!("task_{i}");
                let catalog = Arc::clone(&catalog);
                let progress_callback = progress_callback.clone();
                let project_root = project_root.clone();
                let oauth_credential_store = oauth_credential_store.clone();
                spawn(async move {
                    execute_single_agent(
                        task_id,
                        task,
                        catalog,
                        progress_callback,
                        project_root,
                        oauth_credential_store,
                    )
                    .await
                })
            })
            .collect();

        let results: Vec<SubAgentResult> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|join_result| {
                join_result.unwrap_or_else(|e| SubAgentResult {
                    task_id: "unknown".to_string(),
                    agent_name: "unknown".to_string(),
                    status: SubAgentStatus::Error,
                    output: None,
                    error: Some(format!("Task panicked: {e}")),
                })
            })
            .collect();

        let success_count = results.iter().filter(|r| matches!(r.status, SubAgentStatus::Success)).count();

        let error_count = results.iter().filter(|r| matches!(r.status, SubAgentStatus::Error)).count();

        // Create display metadata using the first task
        let display_meta = ToolDisplayMeta::new("Spawn agent", format!("{first_agent_name} (1/{task_count})"));

        SpawnSubAgentsOutput { results, success_count, error_count, meta: Some(display_meta.into()) }
    }
}

/// Execute a single sub-agent and return its result
#[allow(clippy::too_many_lines)]
async fn execute_single_agent(
    task_id: String,
    task: SubAgentTask,
    catalog: Arc<AgentCatalog>,
    progress_callback: Option<Arc<ProgressCallback>>,
    project_root: PathBuf,
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
) -> SubAgentResult {
    let agent_name = task.agent_name.clone();

    let result: Result<String, String> = async {
        let mut spec = catalog.resolve(&task.agent_name).map_err(|e| e.to_string())?;

        if !spec.exposure.agent_invocable {
            return Err(format!("Agent '{}' is not agent-invocable", task.agent_name));
        }

        let mut spawn_result =
            spawn_mcps(&spec.mcp_config_sources, project_root, oauth_credential_store.clone()).await?;
        let snapshot = spawn_result
            .block_until_ready()
            .await
            .ok_or_else(|| "MCP bootstrap aborted before completion".to_string())?;
        let filtered_tools = spec.tools.apply(snapshot.tool_definitions);
        spec.prompts.push(Prompt::McpInstructions(snapshot.instructions));
        let command_tx = spawn_result.command_tx;

        let (user_tx, mut agent_rx, _agent_handle) =
            spawn_agent(spec, command_tx, filtered_tools, oauth_credential_store).await?;

        let prompt_with_instructions = format!("{}\n\n{}", task.prompt, STRUCTURED_OUTPUT_INSTRUCTIONS);
        user_tx
            .send(Command::UserCommand(UserCommand::Text {
                content: vec![llm::ContentBlock::text(&prompt_with_instructions)],
            }))
            .await
            .map_err(|e| format!("Failed to send message to agent: {e}"))?;

        if let Some(ref callback) = progress_callback {
            callback(&task_id, &agent_name, &AgentEvent::Turn(TurnEvent::Started));
        }

        let mut final_output = String::new();

        while let Some(message) = agent_rx.recv().await {
            if let Some(ref callback) = progress_callback {
                callback(&task_id, &agent_name, &message);
            }

            match &message {
                AgentEvent::Message(MessageEvent::Text { chunk, is_complete, .. }) if *is_complete => {
                    final_output.clone_from(chunk);
                }

                AgentEvent::Turn(TurnEvent::Ended { outcome }) => match outcome {
                    TurnOutcome::Completed => return Ok(final_output),
                    TurnOutcome::Failed { error } => return Err(format!("Agent error: {error}")),
                    TurnOutcome::Cancelled => return Err("Agent cancelled".to_string()),
                },

                _ => {}
            }
        }

        Ok(final_output)
    }
    .await;

    match result {
        Ok(output) => {
            SubAgentResult { task_id, agent_name, status: SubAgentStatus::Success, output: Some(output), error: None }
        }
        Err(error) => {
            SubAgentResult { task_id, agent_name, status: SubAgentStatus::Error, output: None, error: Some(error) }
        }
    }
}

async fn spawn_mcps(
    effective_mcp_config_sources: &[McpConfigSource],
    project_root: PathBuf,
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
) -> Result<McpSpawnResult, String> {
    let mut builder = mcp(&project_root);
    if let Some(store) = oauth_credential_store {
        builder = builder.with_oauth_credential_store(store);
    }
    builder = builder.with_builtin_servers();

    if !effective_mcp_config_sources.is_empty() {
        builder = builder
            .from_mcp_config_sources(effective_mcp_config_sources)
            .await
            .map_err(|e| format!("Failed to load mcp configs: {e}"))?;
    }

    builder.spawn().await.map_err(|e| format!("Failed to spawn MCP manager: {e}"))
}

async fn spawn_agent(
    spec: aether_core::agent_spec::AgentSpec,
    mcp_tx: mpsc::Sender<McpCommand>,
    tools: Vec<ToolDefinition>,
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
) -> Result<(mpsc::Sender<Command>, mpsc::Receiver<AgentEvent>, AgentHandle), String> {
    AgentBuilder::from_spec(&spec, vec![], oauth_credential_store)
        .await
        .map_err(|e| format!("Failed to build agent from spec: {e}"))?
        .tools(mcp_tx, tools)
        .spawn()
        .await
        .map_err(|e| format!("Failed to spawn agent: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_json() {
        let json = r#"{
            "summary": "Found the main entry point",
            "artifacts": [{"path": "/src/main.rs", "relation": "read", "note": "entry point"}],
            "decisions": ["Use async runtime"],
            "nextSteps": ["Implement feature"],
            "details": null
        }"#;

        let result = StructuredAgentOutput::parse(json).expect("Should parse valid JSON");

        assert_eq!(result.summary, "Found the main entry point");
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.artifacts[0].path, "/src/main.rs");
        assert_eq!(result.artifacts[0].relation, "read");
        assert_eq!(result.decisions.len(), 1);
        assert_eq!(result.next_steps.len(), 1);
    }

    #[test]
    fn test_parse_json_in_markdown() {
        let markdown = r#"Here's what I found:

```json
{
    "summary": "Analyzed the codebase",
    "artifacts": [],
    "decisions": ["Use Rust"],
    "nextSteps": [],
    "details": null
}
```

Hope this helps!"#;

        let result = StructuredAgentOutput::parse(markdown).expect("Should parse JSON from markdown");

        assert_eq!(result.summary, "Analyzed the codebase");
        assert_eq!(result.decisions.len(), 1);
        assert_eq!(result.decisions[0], "Use Rust");
    }

    #[test]
    fn test_parse_fallback() {
        let plain_text = "I analyzed the code and found several issues.";

        let result = StructuredAgentOutput::parse(plain_text).expect("Should fallback gracefully");

        assert_eq!(result.summary, "Agent did not return structured output");
        assert!(result.artifacts.is_empty());
        assert!(result.decisions.is_empty());
        assert!(result.next_steps.is_empty());
        assert_eq!(result.details, Some(plain_text.to_string()));
    }

    #[test]
    fn test_extract_json_from_markdown_valid() {
        let markdown = r#"Some text
```json
{"key": "value"}
```
More text"#;

        let result = extract_json_from_markdown(markdown);

        assert_eq!(result, Some(r#"{"key": "value"}"#.to_string()));
    }

    #[test]
    fn test_extract_json_from_markdown_no_block() {
        let text = "Just plain text without code blocks";

        let result = extract_json_from_markdown(text);

        assert!(result.is_none());
    }

    #[test]
    fn test_extract_json_from_markdown_empty_block() {
        let markdown = r"```json
```";

        let result = extract_json_from_markdown(markdown);

        assert!(result.is_none());
    }

    #[test]
    fn test_extract_json_from_markdown_unclosed() {
        let markdown = r#"```json
{"unclosed": true}
"#;

        let result = extract_json_from_markdown(markdown);

        assert!(result.is_none());
    }

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
