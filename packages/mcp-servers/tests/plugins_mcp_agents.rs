use mcp_servers::subagents::SubAgentsMcp;
use mcp_utils::testing::connect;
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation};
use rmcp::service::RunningService;
use rmcp::{RoleClient, RoleServer};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

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
      "mcp": [{"type":"file","path":".aether/mcp/coder.json"}]
    }
  ]
}"#,
        ),
        (".aether/prompts/coder.md", "You are a coding assistant."),
        (".aether/mcp/coder.json", r#"{"servers": {"coding": {"type": "in-memory"}}}"#),
    ];

    let temp_dir = create_test_files(&test_files);
    let (_server_handle, _client) = create_test_client(temp_dir.path()).await;
}

#[tokio::test]
async fn test_spawn_subagents_empty_tasks() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let (_server_handle, client) = create_test_client(temp_dir.path()).await;

    let mut args = serde_json::Map::new();
    args.insert("tasks".to_string(), serde_json::json!([]));
    let result = client
        .call_tool(CallToolRequestParams::new("spawn_subagent").with_arguments(args))
        .await
        .expect("Failed to call spawn_subagent tool");

    if let Some(content) = result.content.first() {
        if let Some(text_content) = content.as_text() {
            let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");

            let results = parsed["results"].as_array().expect("Expected results array");
            assert_eq!(results.len(), 0);
            assert_eq!(parsed["successCount"], 0);
            assert_eq!(parsed["errorCount"], 0);
        } else {
            panic!("Expected text content");
        }
    } else {
        panic!("Expected content in result");
    }
}

#[tokio::test]
async fn test_spawn_subagent_errors_when_no_invocable_agents() {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let (_server_handle, client) = create_test_client(temp_dir.path()).await;

    let mut args = serde_json::Map::new();
    args.insert("tasks".to_string(), serde_json::json!([{ "agentName": "any-agent", "prompt": "Do something" }]));
    let result = client
        .call_tool(CallToolRequestParams::new("spawn_subagent").with_arguments(args))
        .await
        .expect("Tool call should succeed at protocol level");

    let text = result.content.first().and_then(|c| c.as_text()).expect("Expected text content in error response");
    assert!(
        text.text.contains("No agent-invocable sub-agents are registered"),
        "Error message should explain no agents are registered, got: {}",
        text.text,
    );
}

#[tokio::test]
async fn test_spawn_subagent_agent_not_found() {
    let temp_dir = create_project_with_invocable_agent();
    let (_server_handle, client) = create_test_client(temp_dir.path()).await;

    let mut args = serde_json::Map::new();
    args.insert(
        "tasks".to_string(),
        serde_json::json!([{
            "agentName": "nonexistent-agent",
            "prompt": "Do something"
        }]),
    );
    let result = client
        .call_tool(CallToolRequestParams::new("spawn_subagent").with_arguments(args))
        .await
        .expect("Failed to call spawn_subagent tool");

    if let Some(content) = result.content.first() {
        if let Some(text_content) = content.as_text() {
            let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");

            let results = parsed["results"].as_array().expect("Expected results array");
            assert_eq!(results.len(), 1);

            let first_result = &results[0];
            assert_eq!(first_result["status"], "error");
            assert_eq!(first_result["agentName"], "nonexistent-agent");
            assert!(first_result["error"].as_str().unwrap().contains("not found"));

            assert_eq!(parsed["successCount"], 0);
            assert_eq!(parsed["errorCount"], 1);
        } else {
            panic!("Expected text content");
        }
    } else {
        panic!("Expected content in result");
    }
}

#[tokio::test]
async fn test_spawn_subagents_task_id_assignment() {
    let temp_dir = create_project_with_invocable_agent();
    let (_server_handle, client) = create_test_client(temp_dir.path()).await;

    let mut args = serde_json::Map::new();
    args.insert(
        "tasks".to_string(),
        serde_json::json!([
            {
                "agentName": "missing-agent-a",
                "prompt": "First task"
            },
            {
                "agentName": "missing-agent-b",
                "prompt": "Second task"
            }
        ]),
    );
    let result = client
        .call_tool(CallToolRequestParams::new("spawn_subagent").with_arguments(args))
        .await
        .expect("Failed to call spawn_subagent tool");

    if let Some(content) = result.content.first() {
        if let Some(text_content) = content.as_text() {
            let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");

            let results = parsed["results"].as_array().expect("Expected results array");
            assert_eq!(results.len(), 2);

            let first_result = &results[0];
            assert_eq!(first_result["taskId"], "task_0");

            let second_result = &results[1];
            assert_eq!(second_result["taskId"], "task_1");
        } else {
            panic!("Expected text content");
        }
    } else {
        panic!("Expected content in result");
    }
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

async fn create_test_client(
    test_dir: &Path,
) -> (RunningService<RoleServer, SubAgentsMcp>, RunningService<RoleClient, ClientInfo>) {
    let server_service = SubAgentsMcp::from_project_root(test_dir.to_path_buf())
        .expect("Failed to create SubAgentsMcp from project root");
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));

    let (server_handle, client) =
        connect(server_service, client_info).await.expect("Failed to connect MCP server and client");

    (server_handle, client)
}
