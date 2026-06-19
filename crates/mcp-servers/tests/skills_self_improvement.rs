use mcp_servers::skills::SkillsMcp;
use mcp_utils::testing::connect;
use rmcp::ServerHandler;
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation};
use std::path::Path;
use tempfile::TempDir;

async fn create_test_client(
    test_dir: &Path,
) -> (
    rmcp::service::RunningService<rmcp::RoleServer, SkillsMcp>,
    rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>,
) {
    let server_service = SkillsMcp::new(&[test_dir.join("skills")], test_dir.join("notes"));
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));

    connect(server_service, client_info).await.expect("Failed to connect MCP server and client")
}

fn call_tool_params(name: &str, args: &serde_json::Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_string()).with_arguments(args.as_object().unwrap().clone())
}

fn parse_tool_result(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    let content = result.content.first().expect("Expected content");
    let text = content.as_text().expect("Expected text content");
    serde_json::from_str(&text.text).expect("Invalid JSON response")
}

#[tokio::test]
async fn test_save_note_creates_new() {
    let temp_dir = TempDir::new().unwrap();

    let (_server, client) = create_test_client(temp_dir.path()).await;

    let result = client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "agent-spec",
                "content": "Core owns AgentSpec type; CLI owns settings.json parsing.",
                "tags": ["aether", "architecture"]
            }),
        ))
        .await
        .expect("save_note failed");

    let parsed = parse_tool_result(&result);
    assert_eq!(parsed["topic"], "agent-spec");
    assert_eq!(parsed["status"], "created");
    assert!(parsed["content"].as_str().unwrap().contains("Core owns AgentSpec"));

    // Verify file on disk
    let note_md = temp_dir.path().join("notes/agent-spec.md");
    assert!(note_md.exists());
    let content = std::fs::read_to_string(&note_md).unwrap();
    assert!(content.contains("topic: agent-spec"));
    assert!(content.contains("- aether"));
    assert!(content.contains("- architecture"));
}

#[tokio::test]
async fn test_save_note_appends_to_existing() {
    let temp_dir = TempDir::new().unwrap();

    let (_server, client) = create_test_client(temp_dir.path()).await;

    // First note
    client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "agent-spec",
                "content": "First learning.",
                "tags": ["aether"]
            }),
        ))
        .await
        .unwrap();

    // Second note on same topic
    let result = client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "agent-spec",
                "content": "Second learning.",
                "tags": ["architecture"]
            }),
        ))
        .await
        .unwrap();

    let parsed = parse_tool_result(&result);
    assert_eq!(parsed["status"], "appended");
    let content = parsed["content"].as_str().unwrap();
    assert!(content.contains("First learning."));
    assert!(content.contains("Second learning."));

    // Verify tags merged on disk
    let file = std::fs::read_to_string(temp_dir.path().join("notes/agent-spec.md")).unwrap();
    assert!(file.contains("- aether"));
    assert!(file.contains("- architecture"));
}

#[tokio::test]
async fn test_save_note_rejects_empty_content() {
    let temp_dir = TempDir::new().unwrap();

    let (_server, client) = create_test_client(temp_dir.path()).await;

    let result = client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "test",
                "content": "   "
            }),
        ))
        .await
        .unwrap();

    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn test_search_notes_by_topic() {
    let temp_dir = TempDir::new().unwrap();

    let (_server, client) = create_test_client(temp_dir.path()).await;

    // Create two notes
    client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "agent-spec",
                "content": "AgentSpec learning.",
                "tags": ["aether"]
            }),
        ))
        .await
        .unwrap();

    client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "testing-conventions",
                "content": "Use Fake prefix.",
                "tags": ["testing"]
            }),
        ))
        .await
        .unwrap();

    // Search by topic substring
    let result = client
        .call_tool(call_tool_params(
            "search_notes",
            &serde_json::json!({
                "query": "agent"
            }),
        ))
        .await
        .unwrap();

    let parsed = parse_tool_result(&result);
    let results = parsed["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["topic"], "agent-spec");
}

#[tokio::test]
async fn test_search_notes_by_tag() {
    let temp_dir = TempDir::new().unwrap();

    let (_server, client) = create_test_client(temp_dir.path()).await;

    client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "agent-spec",
                "content": "Learning 1.",
                "tags": ["aether"]
            }),
        ))
        .await
        .unwrap();

    client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "mcp-setup",
                "content": "Learning 2.",
                "tags": ["aether"]
            }),
        ))
        .await
        .unwrap();

    let result = client
        .call_tool(call_tool_params(
            "search_notes",
            &serde_json::json!({
                "query": "aether"
            }),
        ))
        .await
        .unwrap();

    let parsed = parse_tool_result(&result);
    let results = parsed["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn test_search_notes_empty_results() {
    let temp_dir = TempDir::new().unwrap();

    let (_server, client) = create_test_client(temp_dir.path()).await;

    let result = client
        .call_tool(call_tool_params(
            "search_notes",
            &serde_json::json!({
                "query": "nonexistent"
            }),
        ))
        .await
        .unwrap();

    let parsed = parse_tool_result(&result);
    let results = parsed["results"].as_array().unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_instructions_reference_list_skills_and_do_not_embed_catalog_entries() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    let agent_dir = skills_dir.join("agent-skill");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("SKILL.md"),
        "---\ndescription: Agent skill\nagent-invocable: true\nagent_authored: true\n---\nContent.\n",
    )
    .unwrap();

    let human_dir = skills_dir.join("human-skill");
    std::fs::create_dir_all(&human_dir).unwrap();
    std::fs::write(human_dir.join("SKILL.md"), "---\ndescription: Human skill\nagent-invocable: true\n---\nContent.\n")
        .unwrap();

    let server = SkillsMcp::new(&[skills_dir], temp_dir.path().join("notes"));
    let info = server.get_info();
    let instructions = info.instructions.unwrap();

    assert!(instructions.contains("search_notes"));
    assert!(instructions.contains("list_skills"));
    assert!(instructions.contains("get_skills"));
    assert!(!instructions.contains("Complete List of Available Skills"));
    assert!(!instructions.contains("human-skill"));
    assert!(!instructions.contains("agent-skill"));
}

#[tokio::test]
async fn test_full_lifecycle() {
    let temp_dir = TempDir::new().unwrap();

    // Curated skill must exist before the client starts so the catalog picks it up.
    let skills_dir = temp_dir.path().join("skills").join("curated");
    std::fs::create_dir_all(&skills_dir).unwrap();
    std::fs::write(
        skills_dir.join("SKILL.md"),
        "---\ndescription: Curated skill\nagent-invocable: true\n---\n# Curated\n\nHand-written skill.",
    )
    .unwrap();

    let (_server, client) = create_test_client(temp_dir.path()).await;

    // 1. Save a note
    let result = client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "lifecycle-topic",
                "content": "First insight.",
                "tags": ["test"]
            }),
        ))
        .await
        .unwrap();
    let parsed = parse_tool_result(&result);
    assert_eq!(parsed["status"], "created");

    // 2. Append to the same topic
    let result = client
        .call_tool(call_tool_params(
            "save_note",
            &serde_json::json!({
                "topic": "lifecycle-topic",
                "content": "Second insight.",
                "tags": ["lifecycle"]
            }),
        ))
        .await
        .unwrap();
    let parsed = parse_tool_result(&result);
    assert_eq!(parsed["status"], "appended");
    let content = parsed["content"].as_str().unwrap();
    assert!(content.contains("First insight."));
    assert!(content.contains("Second insight."));

    // 3. Search for the note
    let result = client
        .call_tool(call_tool_params(
            "search_notes",
            &serde_json::json!({
                "query": "lifecycle"
            }),
        ))
        .await
        .unwrap();
    let parsed = parse_tool_result(&result);
    let results = parsed["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0]["content"].as_str().unwrap().contains("Second insight."));
    assert!(results[0]["tags"].as_array().unwrap().iter().any(|t| t == "test"));
    assert!(results[0]["tags"].as_array().unwrap().iter().any(|t| t == "lifecycle"));

    // 4. Discover skills, then load curated content
    let result = client.call_tool(call_tool_params("list_skills", &serde_json::json!({}))).await.unwrap();
    let parsed = parse_tool_result(&result);
    let skills = parsed["skills"].as_array().unwrap();
    assert!(skills.iter().any(|entry| entry["name"] == "curated"));

    let result = client
        .call_tool(call_tool_params(
            "get_skills",
            &serde_json::json!({
                "requests": [{ "name": "curated" }]
            }),
        ))
        .await
        .unwrap();
    let parsed = parse_tool_result(&result);
    assert!(parsed["files"][0]["content"].as_str().unwrap().contains("Hand-written skill."));
}
