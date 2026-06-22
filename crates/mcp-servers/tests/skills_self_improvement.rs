mod common;

use common::{TestClient, TestResult};
use mcp_servers::skills::SkillsMcp;
use mcp_servers::skills::tools::{ListSkillsInput, LoadSkillsInput, SaveNoteInput, SearchNotesInput, SkillRequest};
use rmcp::ServerHandler;
use std::path::Path;
use tempfile::TempDir;

fn build_server(test_dir: &Path) -> SkillsMcp {
    SkillsMcp::new(&[test_dir.join("skills")], test_dir.join("notes"))
}

fn save_note_input(topic: &str, content: &str, tags: &[&str]) -> SaveNoteInput {
    SaveNoteInput {
        topic: topic.to_string(),
        content: content.to_string(),
        tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
    }
}

fn search_notes_input(query: &str) -> SearchNotesInput {
    SearchNotesInput { query: query.to_string() }
}

fn load_skills_input(requests: &[(&str, Option<&str>)]) -> LoadSkillsInput {
    LoadSkillsInput {
        requests: requests
            .iter()
            .map(|(name, path)| SkillRequest { name: (*name).to_string(), path: path.map(str::to_string) })
            .collect(),
    }
}

#[tokio::test]
async fn test_save_note_creates_new() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp
        .call(
            "save_note",
            save_note_input(
                "agent-spec",
                "Core owns AgentSpec type; CLI owns settings.json parsing.",
                &["aether", "architecture"],
            ),
        )
        .await?;

    assert_eq!(parsed["topic"], "agent-spec");
    assert_eq!(parsed["status"], "created");
    assert!(parsed["content"].as_str().unwrap().contains("Core owns AgentSpec"));

    let note_md = temp_dir.path().join("notes/agent-spec.md");
    assert!(note_md.exists());
    let content = std::fs::read_to_string(&note_md)?;
    assert!(content.contains("topic: agent-spec"));
    assert!(content.contains("- aether"));
    assert!(content.contains("- architecture"));
    Ok(())
}

#[tokio::test]
async fn test_save_note_appends_to_existing() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    mcp.call("save_note", save_note_input("agent-spec", "First learning.", &["aether"])).await?;

    let parsed = mcp.call("save_note", save_note_input("agent-spec", "Second learning.", &["architecture"])).await?;

    assert_eq!(parsed["status"], "appended");
    let content = parsed["content"].as_str().unwrap();
    assert!(content.contains("First learning."));
    assert!(content.contains("Second learning."));

    let file = std::fs::read_to_string(temp_dir.path().join("notes/agent-spec.md"))?;
    assert!(file.contains("- aether"));
    assert!(file.contains("- architecture"));
    Ok(())
}

#[tokio::test]
async fn test_save_note_rejects_empty_content() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let result = mcp.call_raw("save_note", save_note_input("test", "   ", &[])).await?;

    assert!(result.is_error.unwrap_or(false));
    Ok(())
}

#[tokio::test]
async fn test_search_notes_by_topic() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    mcp.call("save_note", save_note_input("agent-spec", "AgentSpec learning.", &["aether"])).await?;
    mcp.call("save_note", save_note_input("testing-conventions", "Use Fake prefix.", &["testing"])).await?;

    let parsed = mcp.call("search_notes", search_notes_input("agent")).await?;
    let results = parsed["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["topic"], "agent-spec");
    Ok(())
}

#[tokio::test]
async fn test_search_notes_by_tag() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    mcp.call("save_note", save_note_input("agent-spec", "Learning 1.", &["aether"])).await?;
    mcp.call("save_note", save_note_input("mcp-setup", "Learning 2.", &["aether"])).await?;

    let parsed = mcp.call("search_notes", search_notes_input("aether")).await?;
    let results = parsed["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    Ok(())
}

#[tokio::test]
async fn test_search_notes_empty_results() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp.call("search_notes", search_notes_input("nonexistent")).await?;
    let results = parsed["results"].as_array().unwrap();
    assert!(results.is_empty());
    Ok(())
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
async fn test_full_lifecycle() -> TestResult {
    let temp_dir = TempDir::new()?;

    let skills_dir = temp_dir.path().join("skills").join("curated");
    std::fs::create_dir_all(&skills_dir)?;
    std::fs::write(
        skills_dir.join("SKILL.md"),
        "---\ndescription: Curated skill\nagent-invocable: true\n---\n# Curated\n\nHand-written skill.",
    )?;

    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp.call("save_note", save_note_input("lifecycle-topic", "First insight.", &["test"])).await?;
    assert_eq!(parsed["status"], "created");

    let parsed = mcp.call("save_note", save_note_input("lifecycle-topic", "Second insight.", &["lifecycle"])).await?;
    assert_eq!(parsed["status"], "appended");
    let content = parsed["content"].as_str().unwrap();
    assert!(content.contains("First insight."));
    assert!(content.contains("Second insight."));

    let parsed = mcp.call("search_notes", search_notes_input("lifecycle")).await?;
    let results = parsed["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0]["content"].as_str().unwrap().contains("Second insight."));
    assert!(results[0]["tags"].as_array().unwrap().iter().any(|t| t == "test"));
    assert!(results[0]["tags"].as_array().unwrap().iter().any(|t| t == "lifecycle"));

    let parsed = mcp.call("list_skills", ListSkillsInput::default()).await?;
    let skills = parsed["skills"].as_array().unwrap();
    assert!(skills.iter().any(|entry| entry["name"] == "curated"));

    let parsed = mcp.call("get_skills", load_skills_input(&[("curated", None)])).await?;
    assert!(parsed["files"][0]["content"].as_str().unwrap().contains("Hand-written skill."));
    Ok(())
}
