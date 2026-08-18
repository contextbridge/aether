use crate::common::{TestClient, TestResult, production_client_info, test_client_info, test_error};
use aether_auth::FakeOAuthCredentialStore;
use aether_core::agent_spec::McpConfigSource;
use aether_core::core::AgentDeps;
use aether_core::events::{AgentEvent, AgentObserver, McpRequestInstrumentation, ObserverFactory, TraceContext};
use aether_core::mcp::mcp;
use aether_core::mcp::tool_bridge::convert_tool_result;
use aether_project::{AetherSettings, AetherSettingsSource, AgentCatalog, SettingsFileSource};
use futures::StreamExt;
use mcp_servers::McpBuilderExt;
use mcp_servers::subagents::SubAgentsMcp;
use mcp_servers::subagents::tools::{SpawnSubAgentsInput, SubAgentTask};
use mcp_utils::client::{CallToolOptions, CancellationToken, ToolCallEvent};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, DetailedTask, GetTaskParams,
    TaskPayload, TaskStatus,
};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
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
    let _mcp = TestClient::start(|| create_test_server(temp_dir.path())).await.unwrap();
}

#[tokio::test]
async fn test_spawn_subagent_codex_uses_oauth_store() -> TestResult {
    let temp_dir = create_project_with_codex_agent();
    let mcp = TestClient::start_with(
        || {
            let deps = AgentDeps::new(Arc::new(FakeOAuthCredentialStore::new()), None)
                .with_agent_registry(test_registry(temp_dir.path()));
            SubAgentsMcp::embedded(temp_dir.path().to_path_buf(), deps)
        },
        production_client_info(),
    )
    .await?;

    let parsed = call_complete(&mcp, spawn_input(&[("explorer", "Read README.md")])).await?;

    let error = parsed["results"][0]["error"].as_str().expect("Expected sub-agent error");
    assert!(error.contains("No Codex OAuth credentials found"));
    Ok(())
}

#[tokio::test]
async fn spawn_subagent_empty_batch_completes_with_empty_output() -> TestResult {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mcp = TestClient::start_with(|| create_test_server(temp_dir.path()), test_client_info()).await?;

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
    let mcp = TestClient::start_with(|| create_test_server(temp_dir.path()), production_client_info()).await?;

    let error =
        mcp.raw().call_tool_once(tool_request(spawn_input(&[("any-agent", "Do something")]))).await.unwrap_err();

    assert!(
        error.to_string().contains("No agent-invocable sub-agents are registered"),
        "Error message should explain no agents are registered, got: {error}",
    );
    Ok(())
}

#[tokio::test]
async fn spawn_subagent_defaults_to_foreground_without_tasks_capability() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let mcp = TestClient::start_with(|| create_test_server(temp_dir.path()), test_client_info()).await?;

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
    let mcp = task_client(temp_dir.path()).await?;

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
    let mcp = task_client(temp_dir.path()).await?;

    let parsed = call_task(&mcp, background_spawn_input(&[("missing-agent", "Do something")])).await?;

    assert_eq!(parsed["results"][0]["status"], "error");
    Ok(())
}

#[tokio::test]
async fn cancelling_background_subagents_cancels_the_batch_without_publishing_a_result() -> TestResult {
    let temp_dir = create_project_with_blocked_agent();
    let mcp = task_client(temp_dir.path()).await?;
    let response =
        mcp.raw().call_tool_once(tool_request(background_spawn_input(&[("blocked", "Wait for MCP startup")]))).await?;
    let CallToolResponse::Task(created) = response else {
        return Err(test_error(format!("expected task response: {response:?}")).into());
    };

    mcp.raw().cancel_task(CancelTaskParams::new(created.task.task_id.clone())).await?;

    let cancelled = await_terminal_task(&mcp, &created.task.task_id).await?;
    assert!(matches!(cancelled.payload, TaskPayload::Cancelled));

    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    let task = mcp.raw().get_task(GetTaskParams::new(created.task.task_id)).await?.task;
    assert!(matches!(task.payload, TaskPayload::Cancelled));
    Ok(())
}

#[tokio::test]
async fn background_instrumentation_finishes_when_the_batch_settles() -> TestResult {
    let temp_dir = create_project_with_blocked_agent();
    let project_root = temp_dir.path().to_path_buf();
    let finishes = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&finishes);
    let mcp = TestClient::start_with(
        move || {
            let mut deps = AgentDeps::default().with_agent_registry(test_registry(&project_root));
            deps.observer_factory = Some(Arc::new(RecordingObserverFactory { finishes: recorded }));
            SubAgentsMcp::embedded(project_root.clone(), deps)
        },
        production_client_info(),
    )
    .await?;

    let response =
        mcp.raw().call_tool_once(tool_request(background_spawn_input(&[("blocked", "Wait for MCP startup")]))).await?;
    let CallToolResponse::Task(created) = response else {
        return Err(test_error(format!("expected task response: {response:?}")).into());
    };
    assert!(finishes.lock().unwrap().is_empty(), "span must stay open while the batch is still running");

    mcp.raw().cancel_task(CancelTaskParams::new(created.task.task_id.clone())).await?;
    await_terminal_task(&mcp, &created.task.task_id).await?;

    assert_eq!(finishes.lock().unwrap().as_slice(), [Some("Sub-agent execution cancelled".to_string())]);
    Ok(())
}

#[tokio::test]
async fn spawn_subagent_background_requires_tasks_capability() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let mcp = TestClient::start_with(|| create_test_server(temp_dir.path()), test_client_info()).await?;

    let error = mcp
        .raw()
        .call_tool_once(tool_request(background_spawn_input(&[("missing-agent", "Do something")])))
        .await
        .unwrap_err();

    assert!(error.to_string().to_lowercase().contains("missing required client capability"));
    Ok(())
}

async fn task_client(project_root: &Path) -> TestResult<TestClient<SubAgentsMcp>> {
    TestClient::start_with(|| create_test_server(project_root), production_client_info()).await
}

async fn await_terminal_task(client: &TestClient<SubAgentsMcp>, task_id: &str) -> TestResult<DetailedTask> {
    loop {
        let task = client.raw().get_task(GetTaskParams::new(task_id.to_string())).await?.task;
        if task.status().is_terminal() {
            return Ok(task);
        }
        tokio::task::yield_now().await;
    }
}

fn tool_request(input: SpawnSubAgentsInput) -> CallToolRequestParams {
    let arguments = serde_json::to_value(input).unwrap().as_object().unwrap().clone();
    CallToolRequestParams::new("spawn_subagent").with_arguments(arguments)
}

async fn call_complete(client: &TestClient<SubAgentsMcp>, input: SpawnSubAgentsInput) -> TestResult<serde_json::Value> {
    let response = client.raw().call_tool_once(tool_request(input)).await?;
    let CallToolResponse::Complete(result) = response else {
        return Err(test_error(format!("expected completed response: {response:?}")).into());
    };
    result.structured_content.ok_or_else(|| test_error("completed response had no structured content").into())
}

async fn call_task(client: &TestClient<SubAgentsMcp>, input: SpawnSubAgentsInput) -> TestResult<serde_json::Value> {
    let response = client.raw().call_tool_once(tool_request(input)).await?;
    let CallToolResponse::Task(created) = response else {
        return Err(test_error(format!("expected task response: {response:?}")).into());
    };
    assert_eq!(created.task.status, TaskStatus::Working);

    let detailed = await_terminal_task(client, &created.task.task_id).await?;
    let TaskPayload::Completed { result } = detailed.payload else {
        return Err(test_error(format!("expected completed task: {detailed:?}")).into());
    };
    let result: CallToolResult = serde_json::from_value(serde_json::Value::Object(result))?;
    result.structured_content.ok_or_else(|| test_error("completed task had no structured content").into())
}

async fn call_subagent_through_manager(
    project_root: &Path,
    deps: AgentDeps,
    input: SpawnSubAgentsInput,
) -> TestResult<String> {
    let mut spawn = mcp(project_root)
        .with_agent_deps(deps)
        .with_builtin_servers()
        .from_mcp_config_sources(&[McpConfigSource::Json(
            r#"{"servers":{"subagents":{"type":"in-memory","args":[]}}}"#.to_string(),
        )])?
        .spawn()
        .await?;

    let snapshot = spawn.block_until_ready().await.ok_or_else(|| test_error("MCP bootstrap aborted"))?;
    let tool = snapshot
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name.ends_with("spawn_subagent"))
        .ok_or_else(|| test_error("spawn_subagent tool missing"))?;
    let request = llm::ToolCallRequest {
        id: "runtime-registry-test".to_string(),
        name: tool.name.clone(),
        arguments: serde_json::to_string(&input)?,
    };

    let options = CallToolOptions { timeout: Duration::MAX, meta: None, cancel: CancellationToken::new() };
    let mut events = spawn.handle().call_model_visible(request.name.clone(), &request.arguments, options);

    while let Some(event) = events.next().await {
        match event {
            ToolCallEvent::Complete(outcome) => {
                let (result, _) = convert_tool_result(&request, outcome).map_err(|error| test_error(error.error))?;
                return Ok(result.result);
            }
            ToolCallEvent::TaskComplete { .. } => {
                return Err(test_error("foreground subagent call unexpectedly returned an MCP Task").into());
            }
            _ => {}
        }
    }
    Err(test_error("MCP manager stopped before returning the tool result").into())
}

fn create_test_files(files: &[(&str, &str)]) -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    for (path, content) in files {
        let full_path = temp_dir.path().join(path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|_| panic!("Failed to create directory for {path}"));
        }
        fs::write(&full_path, content).unwrap_or_else(|_| panic!("Failed to write file {path}"));
    }

    temp_dir
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
    let mut spawn = mcp(project_root)
        .with_agent_deps(deps)
        .with_builtin_servers()
        .from_mcp_config_sources(&[McpConfigSource::Json(
            r#"{"servers":{"subagents":{"type":"in-memory","args":[]}}}"#.to_string(),
        )])
        .expect("Failed to configure subagents MCP")
        .spawn()
        .await
        .expect("Failed to spawn MCP manager");

    let snapshot = spawn.block_until_ready().await.expect("MCP bootstrap aborted");
    snapshot.model_instructions().get("subagents").expect("Missing subagents instructions").clone()
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
    SubAgentsMcp::embedded(test_dir.to_path_buf(), deps)
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
