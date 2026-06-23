mod common;

use common::{TestClient, TestResult};
use mcp_servers::skills::SkillsMcp;
use mcp_servers::skills::tools::{ListSkillsInput, LoadSkillsInput, SkillRequest};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use utils::MarkdownFile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFrontmatter {
    pub description: Option<String>,
}

/// Creates test files and directories from a slice of (path, content) pairs
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

fn build_server(test_dir: &Path) -> SkillsMcp {
    SkillsMcp::new(&[test_dir.join("skills")])
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
async fn test_load_from_nested_directories() {
    let test_files = vec![
        ("skill-1/SKILL.md", "---\ndescription: First skill\n---\nThis is skill 1 content"),
        ("skill-2/SKILL.md", "---\ndescription: Second skill\n---\nThis is skill 2 content"),
        ("illegal-flat-skill.md", "This should be ignored"),
    ];

    let temp_dir = create_test_files(&test_files);

    let skills_with_dirs: Vec<(PathBuf, MarkdownFile<TestFrontmatter>)> =
        MarkdownFile::from_nested_dirs(temp_dir.path(), "SKILL.md").await.expect("Failed to load skills");

    assert_eq!(skills_with_dirs.len(), 2);

    let skill_names: Vec<String> = skills_with_dirs
        .iter()
        .filter_map(|(dir, _)| {
            let name = dir.file_name()?.to_string_lossy().to_string();
            Some(name)
        })
        .collect();

    assert!(skill_names.contains(&"skill-1".to_string()));
    assert!(skill_names.contains(&"skill-2".to_string()));
    assert!(!skill_names.contains(&"illegal-flat-skill".to_string()));
}

#[tokio::test]
async fn test_load_skills_tool() -> TestResult {
    let test_files = vec![
        (
            "skills/skill-1/SKILL.md",
            "---\ndescription: First skill for testing\nagent-invocable: true\n---\n# Skill 1\n\nThis is the content for skill 1.",
        ),
        (
            "skills/skill-2/SKILL.md",
            "---\ndescription: Second skill\nagent-invocable: true\n---\n# Skill 2\n\nThis is the content for skill 2.",
        ),
        (
            "skills/skill-3/SKILL.md",
            "---\ndescription: Third skill\nagent-invocable: true\n---\n# Skill 3\n\nThis is skill 3.",
        ),
    ];

    let temp_dir = create_test_files(&test_files);
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed =
        mcp.call("get_skills", load_skills_input(&[("skill-1", None), ("skill-2", None), ("skill-3", None)])).await?;

    let files = parsed["files"].as_array().expect("Expected files array");
    assert_eq!(files.len(), 3);

    let skill1 = files.iter().find(|s| s["name"] == "skill-1").unwrap();
    assert_eq!(skill1["path"], "SKILL.md");
    assert!(skill1["content"].as_str().unwrap().contains("This is the content for skill 1"));

    let skill2 = files.iter().find(|s| s["name"] == "skill-2").unwrap();
    assert!(skill2["content"].as_str().unwrap().contains("This is the content for skill 2."));

    let skill3 = files.iter().find(|s| s["name"] == "skill-3").unwrap();
    assert!(skill3["content"].as_str().unwrap().contains("This is skill 3"));
    Ok(())
}

#[tokio::test]
async fn test_list_skills_only_returns_agent_invocable_entries() -> TestResult {
    let test_files = vec![
        ("skills/zeta/SKILL.md", "---\ndescription: Zeta\nagent-invocable: true\ntags:\n  - systems\n---\n# Zeta"),
        (
            "skills/user-only/SKILL.md",
            "---\ndescription: User only\nuser-invocable: true\nagent-invocable: false\n---\n# User only",
        ),
        (
            "skills/flat-agent.md",
            "---\nname: alpha-flat\ndescription: Flat skill\nagent-invocable: true\ntags:\n  - flat\n---\n# Flat",
        ),
        (
            "skills/rule-only.md",
            "---\ndescription: Rule only\nagent-invocable: false\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\n# Rule",
        ),
    ];

    let temp_dir = create_test_files(&test_files);
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp.call("list_skills", ListSkillsInput::default()).await?;

    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["count"], 2);
    assert_eq!(parsed["message"], "Found 2 skills");

    let skills = parsed["skills"].as_array().expect("Expected skills array");
    let names: Vec<_> = skills.iter().map(|entry| entry["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["alpha-flat", "zeta"]);

    assert!(skills.iter().all(|entry| entry.get("content").is_none()));
    assert!(skills.iter().all(|entry| entry.get("availableFiles").is_none()));
    Ok(())
}

#[tokio::test]
async fn test_load_skills_with_missing() -> TestResult {
    let test_files = vec![
        ("skills/skill-1/SKILL.md", "---\ndescription: First skill\nagent-invocable: true\n---\n# Skill 1\n\nContent."),
        (
            "skills/skill-2/SKILL.md",
            "---\ndescription: Second skill\nagent-invocable: true\n---\n# Skill 2\n\nContent.",
        ),
    ];

    let temp_dir = create_test_files(&test_files);
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp
        .call("get_skills", load_skills_input(&[("skill-1", None), ("nonexistent-skill", None), ("skill-2", None)]))
        .await?;

    let files = parsed["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);

    let skill1 = files.iter().find(|s| s["name"] == "skill-1").unwrap();
    assert!(skill1["content"].is_string());
    assert!(skill1["error"].is_null());

    let skill2 = files.iter().find(|s| s["name"] == "skill-2").unwrap();
    assert!(skill2["content"].is_string());
    assert!(skill2["error"].is_null());

    let missing = files.iter().find(|s| s["name"] == "nonexistent-skill").unwrap();
    assert!(missing["content"].is_null());
    assert!(missing["error"].as_str().unwrap().contains("not found"));
    Ok(())
}

#[tokio::test]
async fn test_get_skills_rejects_non_agent_invocable_prompts() -> TestResult {
    let test_files = vec![
        ("skills/allowed/SKILL.md", "---\ndescription: Allowed\nagent-invocable: true\n---\n# Allowed"),
        (
            "skills/user-only/SKILL.md",
            "---\ndescription: User only\nuser-invocable: true\nagent-invocable: false\n---\n# User only",
        ),
        (
            "skills/rule-only.md",
            "---\ndescription: Rule only\nagent-invocable: false\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\n# Rule",
        ),
    ];

    let temp_dir = create_test_files(&test_files);
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp
        .call("get_skills", load_skills_input(&[("allowed", None), ("user-only", None), ("rule-only", None)]))
        .await?;

    let files = parsed["files"].as_array().expect("Expected files array");

    let allowed = files.iter().find(|entry| entry["name"] == "allowed").unwrap();
    assert!(allowed["content"].is_string());
    assert!(allowed["error"].is_null());

    let user_only = files.iter().find(|entry| entry["name"] == "user-only").unwrap();
    assert!(user_only["content"].is_null());
    assert!(user_only["error"].as_str().unwrap().contains("not agent-invocable"));

    let rule_only = files.iter().find(|entry| entry["name"] == "rule-only").unwrap();
    assert!(rule_only["content"].is_null());
    assert!(rule_only["error"].as_str().unwrap().contains("not agent-invocable"));
    Ok(())
}

#[tokio::test]
async fn test_load_auxiliary_file() -> TestResult {
    let test_files = vec![
        (
            "skills/test-skill/SKILL.md",
            "---\ndescription: Test skill\nagent-invocable: true\n---\n# Main\n\nSee [traits](./traits.md).",
        ),
        ("skills/test-skill/traits.md", "# Traits\n\nTraits content here."),
        ("skills/test-skill/references/REF.md", "# Reference\n\nReference content."),
    ];

    let temp_dir = create_test_files(&test_files);
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp.call("get_skills", load_skills_input(&[("test-skill", None)])).await?;
    let file = &parsed["files"][0];

    let available = file["availableFiles"].as_array().unwrap();
    assert!(available.contains(&serde_json::json!("references/REF.md")));
    assert!(available.contains(&serde_json::json!("traits.md")));

    let parsed_aux = mcp.call("get_skills", load_skills_input(&[("test-skill", Some("traits.md"))])).await?;
    let aux_file = &parsed_aux["files"][0];

    assert_eq!(aux_file["path"], "traits.md");
    assert!(aux_file["content"].as_str().unwrap().contains("Traits content"));
    assert!(aux_file.get("availableFiles").is_none());
    Ok(())
}

#[tokio::test]
async fn test_reject_traversal() -> TestResult {
    let test_files = vec![("skills/test-skill/SKILL.md", "---\ndescription: Test\nagent-invocable: true\n---\n# Test")];

    let temp_dir = create_test_files(&test_files);
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp.call("get_skills", load_skills_input(&[("test-skill", Some("../other-skill/SKILL.md"))])).await?;
    let file = &parsed["files"][0];

    assert!(file["error"].as_str().unwrap().contains("traversal"));
    Ok(())
}

#[tokio::test]
async fn test_reject_absolute_path() -> TestResult {
    let test_files = vec![("skills/test-skill/SKILL.md", "---\ndescription: Test\nagent-invocable: true\n---\n# Test")];

    let temp_dir = create_test_files(&test_files);
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let parsed = mcp.call("get_skills", load_skills_input(&[("test-skill", Some("/etc/passwd"))])).await?;
    let file = &parsed["files"][0];

    assert!(file["error"].as_str().unwrap().contains("Absolute"));
    Ok(())
}

#[tokio::test]
async fn list_skills_input_schema_has_properties_object() -> TestResult {
    let temp_dir = create_test_files(&[]);
    let mcp = TestClient::start(|| build_server(temp_dir.path())).await?;

    let tools = mcp.raw().peer().list_all_tools().await?;
    let tool = tools.into_iter().find(|tool| tool.name.as_ref() == "list_skills").expect("list_skills tool present");

    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    let properties = schema.get("properties").expect("object schema must include a properties key");
    assert!(properties.is_object(), "properties must be an object, got: {properties}");
    Ok(())
}
