use crate::common::{TestClient, TestResult, load_skills_input, skills_server};
use mcp_servers::skills::SkillsMcp;
use mcp_servers::skills::tools::ListSkillsInput;
use mcp_servers::testing::TestWorkspace;
use rmcp::ServerHandler;

#[tokio::test]
async fn test_instructions_reference_list_skills_and_do_not_embed_catalog_entries() {
    let workspace = TestWorkspace::new()
        .file(
            "skills/agent-skill/SKILL.md",
            "---\ndescription: Agent skill\nagent-invocable: true\nagent_authored: true\n---\nContent.\n",
        )
        .file("skills/human-skill/SKILL.md", "---\ndescription: Human skill\nagent-invocable: true\n---\nContent.\n");

    let server = SkillsMcp::new(&[workspace.path("skills")]);
    let info = server.get_info();
    let instructions = info.instructions.unwrap();

    assert!(instructions.contains("list_skills"));
    assert!(instructions.contains("get_skills"));
    assert!(!instructions.contains("Complete List of Available Skills"));
    assert!(!instructions.contains("human-skill"));
    assert!(!instructions.contains("agent-skill"));
}

#[tokio::test]
async fn test_full_lifecycle() -> TestResult {
    let workspace = TestWorkspace::new().file(
        "skills/curated/SKILL.md",
        "---\ndescription: Curated skill\nagent-invocable: true\n---\n# Curated\n\nHand-written skill.",
    );

    let mcp = TestClient::start(|| skills_server(workspace.root())).await?;

    let parsed = mcp.call("list_skills", ListSkillsInput::default()).await?;
    let skills = parsed["skills"].as_array().unwrap();
    assert!(skills.iter().any(|entry| entry["name"] == "curated"));

    let parsed = mcp.call("get_skills", load_skills_input(&[("curated", None)])).await?;
    assert!(parsed["files"][0]["content"].as_str().unwrap().contains("Hand-written skill."));
    Ok(())
}
