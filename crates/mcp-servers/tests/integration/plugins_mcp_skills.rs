use crate::common::{TestClient, TestResult, load_skills_input, skills_server};
use mcp_servers::skills::tools::ListSkillsInput;
use mcp_servers::testing::{TestWorkspace, skill};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use utils::MarkdownFile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFrontmatter {
    pub description: Option<String>,
}

#[tokio::test]
async fn test_load_from_nested_directories() {
    let workspace = TestWorkspace::new()
        .skill("skill-1", |s| s.description("First skill").body("This is skill 1 content"))
        .skill("skill-2", |s| s.description("Second skill").body("This is skill 2 content"))
        .file("illegal-flat-skill.md", "This should be ignored");

    let skills_with_dirs: Vec<(PathBuf, MarkdownFile<TestFrontmatter>)> =
        MarkdownFile::from_nested_dirs(workspace.root(), "SKILL.md").await.expect("Failed to load skills");

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
    let workspace = TestWorkspace::new()
        .file(
            "skills/skill-1/SKILL.md",
            skill()
                .description("First skill for testing")
                .body("# Skill 1\n\nThis is the content for skill 1.")
                .content(),
        )
        .file(
            "skills/skill-2/SKILL.md",
            skill().description("Second skill").body("# Skill 2\n\nThis is the content for skill 2.").content(),
        )
        .file(
            "skills/skill-3/SKILL.md",
            skill().description("Third skill").body("# Skill 3\n\nThis is skill 3.").content(),
        );

    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

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
    let workspace = TestWorkspace::new()
        .file("skills/zeta/SKILL.md", skill().description("Zeta").tag("systems").body("# Zeta").content())
        .file(
            "skills/user-only/SKILL.md",
            skill().description("User only").user_invocable(true).agent_invocable(false).body("# User only").content(),
        )
        .file(
            "skills/flat-agent.md",
            skill().name("alpha-flat").description("Flat skill").tag("flat").body("# Flat").content(),
        )
        .file(
            "skills/rule-only.md",
            skill().description("Rule only").agent_invocable(false).read_trigger("**/*.rs").body("# Rule").content(),
        );

    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

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
    let workspace = TestWorkspace::new()
        .file("skills/skill-1/SKILL.md", skill().description("First skill").body("# Skill 1\n\nContent.").content())
        .file("skills/skill-2/SKILL.md", skill().description("Second skill").body("# Skill 2\n\nContent.").content());

    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

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
    let workspace = TestWorkspace::new()
        .file("skills/allowed/SKILL.md", skill().description("Allowed").body("# Allowed").content())
        .file(
            "skills/user-only/SKILL.md",
            skill().description("User only").user_invocable(true).agent_invocable(false).body("# User only").content(),
        )
        .file(
            "skills/rule-only.md",
            skill().description("Rule only").agent_invocable(false).read_trigger("**/*.rs").body("# Rule").content(),
        );

    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

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
    let workspace = TestWorkspace::new()
        .file(
            "skills/test-skill/SKILL.md",
            skill().description("Test skill").body("# Main\n\nSee [traits](./traits.md).").content(),
        )
        .file("skills/test-skill/traits.md", "# Traits\n\nTraits content here.")
        .file("skills/test-skill/references/REF.md", "# Reference\n\nReference content.");

    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

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
    let workspace =
        TestWorkspace::new().file("skills/test-skill/SKILL.md", skill().description("Test").body("# Test").content());

    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

    let parsed = mcp.call("get_skills", load_skills_input(&[("test-skill", Some("../other-skill/SKILL.md"))])).await?;
    let file = &parsed["files"][0];

    assert!(file["error"].as_str().unwrap().contains("traversal"));
    Ok(())
}

#[tokio::test]
async fn test_reject_absolute_path() -> TestResult {
    let workspace =
        TestWorkspace::new().file("skills/test-skill/SKILL.md", skill().description("Test").body("# Test").content());

    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

    let parsed = mcp.call("get_skills", load_skills_input(&[("test-skill", Some("/etc/passwd"))])).await?;
    let file = &parsed["files"][0];

    assert!(file["error"].as_str().unwrap().contains("Absolute"));
    Ok(())
}

#[tokio::test]
async fn list_skills_input_schema_has_properties_object() -> TestResult {
    let workspace = TestWorkspace::new();
    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

    let tools = mcp.raw().peer().list_all_tools().await?;
    let tool = tools.into_iter().find(|tool| tool.name.as_ref() == "list_skills").expect("list_skills tool present");

    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    let properties = schema.get("properties").expect("object schema must include a properties key");
    assert!(properties.is_object(), "properties must be an object, got: {properties}");
    Ok(())
}
