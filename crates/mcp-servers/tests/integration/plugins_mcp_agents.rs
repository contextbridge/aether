use crate::common::{TestClient, TestResult};
use aether_auth::FakeOAuthCredentialStore;
use aether_core::core::AgentDeps;
use aether_project::{AetherSettings, AetherSettingsSource, AgentCatalog, SettingsFileSource};
use mcp_servers::subagents::SubAgentsMcp;
use mcp_servers::subagents::tools::{SpawnSubAgentsInput, SubAgentTask};
use std::fs;
use std::path::Path;
use std::sync::Arc;
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
    }
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
        create_test_server(temp_dir.path())
            .with_agent_deps(AgentDeps::new(Arc::new(FakeOAuthCredentialStore::new()), None))
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

fn create_test_server(test_dir: &Path) -> SubAgentsMcp {
    let settings = AetherSettings::load(
        test_dir,
        [AetherSettingsSource::OptionalFile(SettingsFileSource::new(".aether/settings.json", test_dir))],
    )
    .expect("Failed to load project settings");
    let catalog = if settings.agents.is_empty() {
        AgentCatalog::empty(test_dir.to_path_buf())
    } else {
        AgentCatalog::from_settings(test_dir, settings).expect("Failed to create agent catalog")
    };
    SubAgentsMcp::new(catalog, test_dir.to_path_buf())
}
