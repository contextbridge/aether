use crate::common::{TestClient, TestResult, test_error};
use aether_auth::FakeOAuthCredentialStore;
use aether_core::agent_spec::McpConfigSource;
use aether_core::core::AgentDeps;
use aether_core::mcp::mcp;
use aether_core::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use aether_project::{AetherSettings, AetherSettingsSource, AgentCatalog, SettingsFileSource};
use mcp_servers::McpBuilderExt;
use mcp_servers::subagents::SubAgentsMcp;
use mcp_servers::subagents::tools::{SpawnSubAgentsInput, SubAgentTask};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn spawn_input(tasks: &[(&str, &str)]) -> SpawnSubAgentsInput {
    SpawnSubAgentsInput {
        tasks: tasks
            .iter()
            .map(|(agent_name, prompt)| SubAgentTask {
                agent_name: (*agent_name).to_string(),
                prompt: (*prompt).to_string(),
            })
            .collect(),
    }
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
    let mcp = TestClient::start(|| {
        let deps = AgentDeps::new(Arc::new(FakeOAuthCredentialStore::new()), None)
            .with_agent_registry(test_registry(temp_dir.path()));
        SubAgentsMcp::embedded(temp_dir.path().to_path_buf(), deps)
    })
    .await?;

    let parsed = mcp.call("spawn_subagent", spawn_input(&[("explorer", "Read README.md")])).await?;

    let error = parsed["results"][0]["error"].as_str().expect("Expected sub-agent error");
    assert!(error.contains("No Codex OAuth credentials found"));
    Ok(())
}

#[tokio::test]
async fn test_spawn_subagents_empty_tasks() -> TestResult {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mcp = TestClient::start(|| create_test_server(temp_dir.path())).await?;

    let parsed = mcp.call("spawn_subagent", spawn_input(&[])).await?;

    let results = parsed["results"].as_array().expect("Expected results array");
    assert_eq!(results.len(), 0);
    assert_eq!(parsed["successCount"], 0);
    assert_eq!(parsed["errorCount"], 0);
    Ok(())
}

#[tokio::test]
async fn test_spawn_subagent_errors_when_no_invocable_agents() -> TestResult {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let mcp = TestClient::start(|| create_test_server(temp_dir.path())).await?;

    let result = mcp.call_raw("spawn_subagent", spawn_input(&[("any-agent", "Do something")])).await?;

    let text = result.content.first().and_then(|c| c.as_text()).expect("Expected text content in error response");
    assert!(
        text.text.contains("No agent-invocable sub-agents are registered"),
        "Error message should explain no agents are registered, got: {}",
        text.text,
    );
    Ok(())
}

#[tokio::test]
async fn test_spawn_subagent_agent_not_found() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let mcp = TestClient::start(|| create_test_server(temp_dir.path())).await?;

    let parsed = mcp.call("spawn_subagent", spawn_input(&[("nonexistent-agent", "Do something")])).await?;

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
async fn test_spawn_subagents_task_id_assignment() -> TestResult {
    let temp_dir = create_project_with_invocable_agent();
    let mcp = TestClient::start(|| create_test_server(temp_dir.path())).await?;

    let parsed = mcp
        .call("spawn_subagent", spawn_input(&[("missing-agent-a", "First task"), ("missing-agent-b", "Second task")]))
        .await?;

    let results = parsed["results"].as_array().expect("Expected results array");
    assert_eq!(results.len(), 2);

    assert_eq!(results[0]["taskId"], "task_0");
    assert_eq!(results[1]["taskId"], "task_1");
    Ok(())
}

async fn call_subagent_through_manager(
    project_root: &Path,
    deps: AgentDeps,
    input: SpawnSubAgentsInput,
) -> TestResult<String> {
    let mut spawn = mcp(project_root)
        .with_builtin_servers(deps)
        .from_mcp_config_sources(&[McpConfigSource::Json(
            r#"{"servers":{"subagents":{"type":"in-memory","args":[]}}}"#.to_string(),
        )])
        .await?
        .spawn()
        .await?;

    let snapshot = spawn.block_until_ready().await.ok_or_else(|| test_error("MCP bootstrap aborted"))?;
    let tool = snapshot
        .tool_definitions
        .iter()
        .find(|tool| tool.name.ends_with("spawn_subagent"))
        .ok_or_else(|| test_error("spawn_subagent tool missing"))?;
    let request = llm::ToolCallRequest {
        id: "runtime-registry-test".to_string(),
        name: tool.name.clone(),
        arguments: serde_json::to_string(&input)?,
    };

    let (event_tx, mut event_rx) = mpsc::channel(4);
    spawn
        .command_tx
        .send(McpCommand::ExecuteTool { request, trace_context: None, timeout: Duration::MAX, tx: event_tx })
        .await?;

    while let Some(event) = event_rx.recv().await {
        if let ToolExecutionEvent::Complete { result, .. } = event {
            let result = result.map_err(|error| test_error(error.error))?;
            return Ok(result.result);
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
        .with_builtin_servers(deps)
        .from_mcp_config_sources(&[McpConfigSource::Json(
            r#"{"servers":{"subagents":{"type":"in-memory","args":[]}}}"#.to_string(),
        )])
        .await
        .expect("Failed to configure subagents MCP")
        .spawn()
        .await
        .expect("Failed to spawn MCP manager");

    let snapshot = spawn.block_until_ready().await.expect("MCP bootstrap aborted");
    snapshot.instructions.get("subagents").expect("Missing subagents instructions").clone()
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
