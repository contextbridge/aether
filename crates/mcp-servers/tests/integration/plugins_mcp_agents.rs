use crate::common::{
    TestResult, await_terminal_task, call_task, cancels_prompts, connect, create_test_files, taskless, test_error,
    tool_request,
};
use aether_auth::FakeOAuthCredentialStore;
use aether_core::core::AgentDeps;
use aether_core::events::{AgentEvent, AgentObserver, McpRequestInstrumentation, ObserverFactory, TraceContext};
use aether_core::mcp::{McpBuilder, McpConfig, ToolFilter};
use aether_core::testing::ready_catalog;
use aether_project::{AetherSettings, AetherSettingsSource, AgentCatalog, SettingsFileSource};
use futures::StreamExt;
use mcp_servers::agent_mcp;
use mcp_servers::subagents::SubAgentsMcp;
use mcp_servers::subagents::tools::{SpawnSubAgentsInput, SubAgentTask};
use mcp_utils::client::{ClientOptions, McpClient, ToolCallEvent, ToolCallOptions};
use mcp_utils::testing::args;
use rmcp::model::{CallToolResponse, CallToolResult, CancelTaskParams, GetTaskParams, TaskPayload};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

fn spawn_input(tasks: &[(&str, &str)]) -> SpawnSubAgentsInput {
    SpawnSubAgentsInput {
        tasks: tasks
            .iter()
            .map(|(agent_name, prompt)| SubAgentTask {
                agent_name: (*agent_name).to_string(),
                prompt: (*prompt).to_string(),
            })
            .collect(),
        run_in_background: false,
    }
}

fn background_spawn_input(tasks: &[(&str, &str)]) -> SpawnSubAgentsInput {
    SpawnSubAgentsInput { run_in_background: true, ..spawn_input(tasks) }
}

const RUNTIME_PROVIDER_URL: &str = "http://127.0.0.1:1";

#[tokio::test]
async fn spawn_uses_runtime_registry_and_provider_overrides() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let settings = AetherSettings::load(
        temp_dir.path(),
        [AetherSettingsSource::Json(
            r#"{
  "agents": [{
    "name": "runtime-explorer",
    "description": "Runtime supplied explorer",
    "model": "anthropic:claude-sonnet-4-5",
    "agentInvocable": true,
    "prompts": [{"type":"text","text":"Use runtime settings."}]
  }]
}"#
            .to_string(),
        )],
    )?;
    let mut provider = llm::ProviderConnectionOverride::url(RUNTIME_PROVIDER_URL);
    provider.merge(llm::ProviderConnectionOverride::auth(llm::ProviderAuthMode::None));
    let overrides =
        llm::ProviderConnectionOverrides::new(std::collections::BTreeMap::from([("anthropic".to_string(), provider)]));
    let catalog = AgentCatalog::from_settings(temp_dir.path(), settings)?.with_provider_connections(overrides);
    let deps = AgentDeps::default().with_agent_registry(catalog.registry().clone());
    let result = call_subagent_through_manager(
        temp_dir.path(),
        deps,
        spawn_input(&[("runtime-explorer", "Do something"), ("coder", "Do something")]),
    )
    .await?;

    assert!(result.contains(RUNTIME_PROVIDER_URL), "provider override missing from result: {result}");
    assert!(result.contains("Agent 'coder' not found"), "workspace-only agent should be rejected: {result}");
    Ok(())
}

#[tokio::test]
async fn embedded_subagents_use_runtime_catalog_instead_of_checkout_settings() {
    let temp_dir = create_project_with_invocable_agent();
    let runtime_settings = AetherSettings::load(
        temp_dir.path(),
        [AetherSettingsSource::Json(
            r#"{
  "agents": [
    {
      "name": "runtime-explorer",
      "description": "Runtime supplied explorer",
      "model": "anthropic:claude-sonnet-4-5",
      "agentInvocable": true,
      "prompts": [{"type":"text","text":"Use runtime settings."}]
    }
  ]
}"#
            .to_string(),
        )],
    )
    .expect("Failed to load runtime settings");
    let runtime_catalog =
        AgentCatalog::from_settings(temp_dir.path(), runtime_settings).expect("Failed to create runtime catalog");

    let instructions = subagent_instructions(temp_dir.path(), runtime_catalog).await;

    assert!(instructions.contains("runtime-explorer"), "runtime catalog agent missing: {instructions}");
    assert!(!instructions.contains("coder"), "workspace settings leaked into instructions: {instructions}");
}

#[tokio::test]
async fn embedded_subagents_report_no_agents_when_runtime_catalog_is_empty() {
    let temp_dir = create_project_with_invocable_agent();
    let instructions = subagent_instructions(temp_dir.path(), AgentCatalog::empty(temp_dir.path().to_path_buf())).await;
    assert!(instructions.contains("No sub-agents are currently available"), "unexpected instructions: {instructions}");
    assert!(!instructions.contains("coder"), "workspace settings leaked into instructions: {instructions}");
}

#[tokio::test]
async fn test_spawn_agent_with_coding_mcp_from_settings_catalog() {
    let test_files = vec![
        (
            ".aether/settings.json",
            r#"{
  "agents": [
    {
      "name": "coder",
      "description": "A coding agent with file access",
      "model": "anthropic:claude-sonnet-4-5",
      "agentInvocable": true,
      "prompts": [{"type":"file","path":".aether/prompts/coder.md"}],
      "mcps": [{"type":"file","path":".aether/mcp/coder.json"}]
    }
  ]
}"#,
        ),
        (".aether/prompts/coder.md", "You are a coding assistant."),
        (".aether/mcp/coder.json", r#"{"servers": {"coding": {"type": "in-memory"}}}"#),
    ];

    let temp_dir = create_test_files(&test_files);
    let _mcp = connect(create_test_server(temp_dir.path()), ClientOptions::default()).await;
}

#[tokio::test]
async fn test_spawn_subagent_codex_uses_oauth_store() -> TestResult {
    let temp_dir = create_project_with_codex_agent();
    let deps = AgentDeps::new(Arc::new(FakeOAuthCredentialStore::new()), None)
        .with_agent_registry(test_registry(temp_dir.path()));
    let mcp = connect(SubAgentsMcp::new(temp_dir.path().to_path_buf(), deps), cancels_prompts()).await;

    let parsed = call_complete(&mcp, spawn_input(&[("explorer", "Read README.md")])).await?;

    let error = parsed["results"][0]["error"].as_str().expect("Expected sub-agent error");
    assert!(error.contains("No Codex OAuth credentials found"));
    Ok(())
}

#[tokio::test]
async fn spawn_subagent_empty_batch_completes_with_empty_output() -> TestResult {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mcp = connect(create_test_server(temp_dir.path()), taskless()).await;

    let parsed = call_complete(&mcp, spawn_input(&[])).await?;

    let results = parsed["results"].as_array().expect("Expected results array");
    assert_eq!(results.len(), 0);
    assert_eq!(parsed["successCount"], 0);
    assert_eq!(parsed["errorCount"], 0);
    Ok(())
}

#[tokio::test]
async fn test_spawn_subagent_errors_when_no_invocable_agents() -> TestResult {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mcp = connect(create_test_server(temp_dir.path()), cancels_prompts()).await;

    let error = mcp
        .peer()
        .call_tool_once(tool_request("spawn_subagent", spawn_input(&[("any-agent", "Do something")])))
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("No agent-invocable sub-agents are registered"),
        "Error message should explain no agents are registered, got: {error}",
    );
    Ok(())
}

#[tokio::test]
async fn spawn_subagent_defaults_to_foreground_without_tasks_capability() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let mcp = connect(create_test_server(temp_dir.path()), taskless()).await;

    let parsed = call_complete(&mcp, spawn_input(&[("nonexistent-agent", "Do something")])).await?;

    let results = parsed["results"].as_array().expect("Expected results array");
    assert_eq!(results.len(), 1);

    let first_result = &results[0];
    assert_eq!(first_result["status"], "error");
    assert_eq!(first_result["agentName"], "nonexistent-agent");
    assert!(first_result["error"].as_str().unwrap().contains("not found"));

    assert_eq!(parsed["successCount"], 0);
    assert_eq!(parsed["errorCount"], 1);
    Ok(())
}

#[tokio::test]
async fn spawn_subagent_preserves_child_result_order_and_ids() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let mcp = task_client(temp_dir.path()).await;

    let parsed =
        call_complete(&mcp, spawn_input(&[("missing-agent-a", "First task"), ("missing-agent-b", "Second task")]))
            .await?;

    let results = parsed["results"].as_array().expect("Expected results array");
    assert_eq!(results.len(), 2);

    assert_eq!(results[0]["taskId"], "task_0");
    assert_eq!(results[1]["taskId"], "task_1");
    Ok(())
}

#[tokio::test]
async fn spawn_subagent_background_returns_mcp_task() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let mcp = task_client(temp_dir.path()).await;

    let input = background_spawn_input(&[("missing-agent", "Do something")]);
    let result = call_task(&mcp, "spawn_subagent", serde_json::to_value(input)?).await?;

    let parsed = result.structured_content.ok_or_else(|| test_error("completed task had no structured content"))?;
    assert_eq!(parsed["results"][0]["status"], "error");
    Ok(())
}

#[tokio::test]
async fn cancelling_background_subagents_cancels_the_batch_without_publishing_a_result() -> TestResult {
    let temp_dir = create_project_with_blocked_agent();
    let mcp = task_client(temp_dir.path()).await;
    let response = mcp
        .peer()
        .call_tool_once(tool_request("spawn_subagent", background_spawn_input(&[("blocked", "Wait for MCP startup")])))
        .await?;
    let CallToolResponse::Task(created) = response else {
        return Err(test_error(format!("expected task response: {response:?}")).into());
    };

    mcp.peer().cancel_task(CancelTaskParams::new(created.task.task_id.clone())).await?;

    let cancelled = await_terminal_task(&mcp, &created.task.task_id).await?;
    assert!(matches!(cancelled.payload, TaskPayload::Cancelled));

    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    let task = mcp.peer().get_task(GetTaskParams::new(created.task.task_id)).await?.task;
    assert!(matches!(task.payload, TaskPayload::Cancelled));
    Ok(())
}

#[tokio::test]
async fn background_instrumentation_finishes_when_the_batch_settles() -> TestResult {
    let temp_dir = create_project_with_blocked_agent();
    let project_root = temp_dir.path().to_path_buf();
    let finishes = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&finishes);
    let mut deps = AgentDeps::default().with_agent_registry(test_registry(&project_root));
    deps.observer_factory = Some(Arc::new(RecordingObserverFactory { finishes: recorded }));
    let mcp = connect(SubAgentsMcp::new(project_root.clone(), deps), cancels_prompts()).await;

    let response = mcp
        .peer()
        .call_tool_once(tool_request("spawn_subagent", background_spawn_input(&[("blocked", "Wait for MCP startup")])))
        .await?;
    let CallToolResponse::Task(created) = response else {
        return Err(test_error(format!("expected task response: {response:?}")).into());
    };
    assert!(finishes.lock().unwrap().is_empty(), "span must stay open while the batch is still running");

    mcp.peer().cancel_task(CancelTaskParams::new(created.task.task_id.clone())).await?;
    await_terminal_task(&mcp, &created.task.task_id).await?;

    assert_eq!(finishes.lock().unwrap().as_slice(), [Some("Sub-agent execution cancelled".to_string())]);
    Ok(())
}

#[tokio::test]
async fn spawn_subagent_background_requires_tasks_capability() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let mcp = connect(create_test_server(temp_dir.path()), taskless()).await;

    let error = mcp
        .peer()
        .call_tool_once(tool_request("spawn_subagent", background_spawn_input(&[("missing-agent", "Do something")])))
        .await
        .unwrap_err();

    assert!(error.to_string().to_lowercase().contains("missing required client capability"));
    Ok(())
}

async fn task_client(project_root: &Path) -> McpClient {
    connect(create_test_server(project_root), cancels_prompts()).await
}

async fn call_complete(client: &McpClient, input: SpawnSubAgentsInput) -> TestResult<serde_json::Value> {
    let response = client.peer().call_tool_once(tool_request("spawn_subagent", input)).await?;
    let CallToolResponse::Complete(result) = response else {
        return Err(test_error(format!("expected completed response: {response:?}")).into());
    };
    result.structured_content.ok_or_else(|| test_error("completed response had no structured content").into())
}

async fn call_subagent_through_manager(
    project_root: &Path,
    deps: AgentDeps,
    input: SpawnSubAgentsInput,
) -> TestResult<String> {
    let mcp = ready_catalog(subagents_catalog(project_root, deps)).await;
    let arguments = args(serde_json::to_value(&input)?);
    let mut events = mcp
        .call_tool("subagents__spawn_subagent", arguments, ToolCallOptions::default())
        .expect("the subagents server offers spawn_subagent");

    while let Some(event) = events.next().await {
        match event {
            ToolCallEvent::TaskCreated(_) => {
                return Err(test_error("foreground subagent call unexpectedly returned an MCP Task").into());
            }
            ToolCallEvent::Done(result) => return Ok(result_text(&result?)),
            ToolCallEvent::Progress(_) | ToolCallEvent::TaskStatus(_) => {}
        }
    }
    Err(test_error("MCP manager stopped before returning the tool result").into())
}

fn subagents_catalog(project_root: &Path, deps: AgentDeps) -> McpBuilder {
    let config = McpConfig::from_json(r#"{"servers":{"subagents":{"type":"in-memory","args":[]}}}"#).unwrap();
    agent_mcp(project_root, deps, ToolFilter::default()).with_config(config).expect("the subagents config is valid")
}

fn result_text(result: &CallToolResult) -> String {
    match &result.structured_content {
        Some(structured) => structured.to_string(),
        None => result.content.iter().filter_map(|content| Some(content.as_text()?.text.as_str())).collect(),
    }
}

fn create_project_with_invocable_agent() -> TempDir {
    create_test_files(&[
        (
            ".aether/settings.json",
            r#"{
  "agents": [
    {
      "name": "coder",
      "description": "A coding agent",
      "model": "anthropic:claude-sonnet-4-5",
      "agentInvocable": true,
      "prompts": [{"type":"file","path":".aether/prompts/coder.md"}]
    }
  ]
}"#,
        ),
        (".aether/prompts/coder.md", "You are a coding assistant."),
    ])
}

fn create_project_with_blocked_agent() -> TempDir {
    create_test_files(&[(
        ".aether/settings.json",
        r#"{
  "agents": [{
    "name": "blocked",
    "description": "An agent whose MCP runtime never becomes ready",
    "model": "anthropic:claude-sonnet-4-5",
    "agentInvocable": true,
    "prompts": [{"type":"text","text":"Wait."}],
    "mcps": [{"type":"inline","servers":{"blocked":{"type":"stdio","command":"sleep","args":["60"]}}}]
  }]
}"#,
    )])
}

fn create_project_with_codex_agent() -> TempDir {
    create_test_files(&[(
        ".aether/settings.json",
        r#"{
  "agents": [
    {
      "name": "explorer",
      "description": "A Codex sub-agent",
      "model": "codex:gpt-5.4-mini",
      "agentInvocable": true,
      "prompts": [{"type":"text","text":"You are a codebase explorer."}]
    }
  ]
}"#,
    )])
}

/// Bring up the embedded subagents server behind an MCP manager and return the
/// instructions it advertises, which name the agents it will accept.
async fn subagent_instructions(project_root: &Path, catalog: AgentCatalog) -> String {
    let deps = AgentDeps::default().with_agent_registry(catalog.registry().clone());
    let mcp = ready_catalog(subagents_catalog(project_root, deps)).await;
    mcp.instructions().servers.get("subagents").expect("Missing subagents instructions").clone()
}

/// The catalog a standalone server would discover from `test_dir`.
fn test_registry(test_dir: &Path) -> aether_core::core::AgentRegistry {
    let settings = AetherSettings::load(
        test_dir,
        [AetherSettingsSource::OptionalFile(SettingsFileSource::new(".aether/settings.json", test_dir))],
    )
    .expect("Failed to load project settings");
    AgentCatalog::from_settings_or_empty(test_dir, settings).expect("Failed to create agent catalog").registry().clone()
}

fn create_test_server(test_dir: &Path) -> SubAgentsMcp {
    let deps = AgentDeps::default().with_agent_registry(test_registry(test_dir));
    SubAgentsMcp::new(test_dir.to_path_buf(), deps)
}

struct RecordingObserverFactory {
    finishes: Arc<Mutex<Vec<Option<String>>>>,
}

impl ObserverFactory for RecordingObserverFactory {
    fn agent(&self, _agent_name: Option<&str>, _parent: Option<&TraceContext>) -> Box<dyn AgentObserver> {
        Box::new(NoopAgentObserver)
    }

    fn tool_call_request(
        &self,
        _tool_name: &str,
        _parent: Option<&TraceContext>,
    ) -> Box<dyn McpRequestInstrumentation> {
        Box::new(RecordingInstrumentation { finishes: Arc::clone(&self.finishes) })
    }
}

struct NoopAgentObserver;

impl AgentObserver for NoopAgentObserver {
    fn on_event(&mut self, _message: &AgentEvent) {}
}

struct RecordingInstrumentation {
    finishes: Arc<Mutex<Vec<Option<String>>>>,
}

impl McpRequestInstrumentation for RecordingInstrumentation {
    fn trace_context(&self) -> Option<TraceContext> {
        None
    }

    fn finish(self: Box<Self>, error: Option<&str>) {
        self.finishes.lock().unwrap().push(error.map(str::to_string));
    }
}
