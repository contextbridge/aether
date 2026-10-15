use crate::common::{McpClientExt, TestResult, connect, load_skills_input, skills_server};
use clap::Parser;
use mcp_servers::skills::tools::ListSkillsInput;
use mcp_servers::skills::{SkillsMcp, SkillsMcpArgs};
use mcp_servers::testing::TestWorkspace;
use mcp_utils::client::{ClientOptions, McpClient};
use serde_json::{Map, Value, json};
use std::fs::canonicalize;

const USER_ONLY: &str = "---\ndescription: User only\nuser-invocable: true\nagent-invocable: false\n---\n# User";
const RULE_ONLY: &str =
    "---\ndescription: Rule only\nagent-invocable: false\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\n";

#[tokio::test]
async fn list_skills_returns_agent_invocable_skills_sorted_by_name() -> TestResult {
    let workspace = TestWorkspace::new()
        .file("skills/zeta/SKILL.md", "---\ndescription: Zeta\nagent-invocable: true\ntags:\n  - systems\n---\n# Zeta")
        .file("skills/user-only/SKILL.md", USER_ONLY)
        .file(
            "skills/flat.md",
            "---\nname: alpha-flat\ndescription: Flat\nagent-invocable: true\ntags:\n  - flat\n---\n",
        )
        .file("skills/rule-only.md", RULE_ONLY);
    let client = serve(&workspace).await;

    let listed = client.call("list_skills", ListSkillsInput::default()).await?;

    assert_eq!(
        listed["skills"],
        json!([
            { "name": "alpha-flat", "description": "Flat", "tags": ["flat"] },
            { "name": "zeta", "description": "Zeta", "tags": ["systems"] },
        ])
    );
    assert_eq!(listed["_meta"]["display"], json!({ "title": "Skills", "value": "2 skills" }));
    Ok(())
}

#[tokio::test]
async fn instructions_point_to_list_skills_instead_of_embedding_the_catalog() {
    let workspace = TestWorkspace::new().file("skills/unique-skill/SKILL.md", skill("Unique", "# Unique"));
    let client = serve(&workspace).await;

    let instructions = client.instructions().unwrap_or_default();

    assert!(instructions.contains("list_skills") && instructions.contains("get_skills"), "{instructions}");
    assert!(!instructions.contains("unique-skill"), "{instructions}");
}

#[tokio::test]
async fn get_skills_loads_directory_skills_and_their_auxiliary_files() -> TestResult {
    let skill_md = "---\nname: rust\ndescription: Rust\nagent-invocable: true\n---\n# Rust";
    let workspace = TestWorkspace::new()
        .file("skills/dir-name/SKILL.md", skill_md)
        .file("skills/dir-name/traits.md", "# Traits")
        .file("skills/dir-name/references/REF.md", "# Ref")
        .file("skills/dir-name/.hidden", "ignored");
    let client = serve(&workspace).await;

    assert_eq!(
        get_skill(&client, "rust", None).await?,
        json!({
            "name": "rust",
            "path": "SKILL.md",
            "content": skill_md,
            "availableFiles": ["references/REF.md", "traits.md"],
        })
    );
    assert_eq!(
        get_skill(&client, "rust", Some("traits.md")).await?,
        json!({ "name": "rust", "path": "traits.md", "content": "# Traits" })
    );
    assert_eq!(
        get_skill(&client, "rust", Some("references/REF.md")).await?,
        json!({ "name": "rust", "path": "references/REF.md", "content": "# Ref" })
    );
    Ok(())
}

#[tokio::test]
async fn get_skills_loads_flat_prompts_by_frontmatter_name() -> TestResult {
    let rules = "---\nname: rust-rules\ndescription: Rust rules\nagent-invocable: true\n---\nUse Rust conventions.";
    let workspace = TestWorkspace::new().file("skills/rule-file.md", rules);
    let client = serve(&workspace).await;

    assert_eq!(
        get_skill(&client, "rust-rules", None).await?,
        json!({ "name": "rust-rules", "path": "rule-file.md", "content": rules })
    );
    Ok(())
}

#[tokio::test]
async fn get_skills_answers_every_request_in_order() -> TestResult {
    let workspace = TestWorkspace::new()
        .file("skills/rust/SKILL.md", skill("Rust", "# Rust"))
        .file("skills/rust/traits.md", "# Traits")
        .file("skills/python/SKILL.md", skill("Python", "# Python"));
    let client = serve(&workspace).await;

    let requests = [("rust", None), ("missing", None), ("rust", Some("traits.md")), ("python", None)];
    let output = client.call("get_skills", load_skills_input(&requests)).await?;

    let answered: Vec<_> = output["files"]
        .as_array()
        .expect("files array")
        .iter()
        .map(|file| (file["name"].as_str().unwrap(), file["path"].as_str().unwrap(), file["error"].is_string()))
        .collect();
    assert_eq!(
        answered,
        [
            ("rust", "SKILL.md", false),
            ("missing", "SKILL.md", true),
            ("rust", "traits.md", false),
            ("python", "SKILL.md", false)
        ]
    );
    Ok(())
}

#[tokio::test]
async fn get_skills_reports_requests_it_cannot_load() -> TestResult {
    let workspace = TestWorkspace::new()
        .file("skills/dir-skill/SKILL.md", skill("Dir", "# Dir"))
        .file("skills/flat.md", skill("Flat", "# Flat"))
        .file("skills/user-only/SKILL.md", USER_ONLY)
        .file("skills/rule-only.md", RULE_ONLY);
    let client = serve(&workspace).await;

    let cases = [
        ("missing", None, "Skill not found: missing"),
        ("user-only", None, "Skill 'user-only' is not agent-invocable"),
        ("rule-only", None, "Skill 'rule-only' is not agent-invocable"),
        ("flat", Some("details.md"), "does not support relative paths"),
        ("dir-skill", Some("/etc/passwd"), "Absolute paths are not allowed"),
        ("dir-skill", Some("../flat.md"), "Path traversal (..) is not allowed"),
        ("dir-skill", Some("."), "Path is a directory, not a file"),
        ("dir-skill", Some("missing.md"), "File not found"),
    ];
    for (name, path, error) in cases {
        let file = get_skill(&client, name, path).await?;
        assert!(file["content"].is_null(), "{name} {path:?}: {file}");
        assert!(file["error"].as_str().is_some_and(|message| message.contains(error)), "{name} {path:?}: {file}");
    }
    Ok(())
}

#[tokio::test]
async fn later_skill_dirs_extend_and_override_earlier_ones() -> TestResult {
    let workspace = TestWorkspace::new()
        .file("a/rust/SKILL.md", skill("Rust A", "From A"))
        .file("a/go/SKILL.md", skill("Go", "Go"))
        .file("b/rust/SKILL.md", skill("Rust B", "From B"));
    let dirs = ["missing", "a", "b"].map(|dir| workspace.path(dir));
    let client = connect(SkillsMcp::new(workspace.root(), &dirs), ClientOptions::default()).await;

    let listed = client.call("list_skills", ListSkillsInput::default()).await?;

    assert_eq!(
        listed["skills"],
        json!([{ "name": "go", "description": "Go" }, { "name": "rust", "description": "Rust B" }])
    );
    assert_eq!(get_skill(&client, "rust", None).await?["content"], skill("Rust B", "From B"));
    Ok(())
}

#[tokio::test]
async fn skill_files_expand_shell_interpolations() -> TestResult {
    let workspace = TestWorkspace::new()
        .file("skills/expands/SKILL.md", skill("Expands", "value: !`echo ok`"))
        .file("skills/fails/SKILL.md", skill("Fails", "value: !`exit 1`"));
    let client = serve(&workspace).await;

    assert_eq!(get_skill(&client, "expands", None).await?["content"], skill("Expands", "value: ok"));
    assert_eq!(get_skill(&client, "fails", None).await?["content"], skill("Fails", "value: "));
    Ok(())
}

#[tokio::test]
async fn from_args_resolves_dirs_and_runs_prompts_from_the_base_dir() -> TestResult {
    let workspace = TestWorkspace::new()
        .file(".aether/skills/where/SKILL.md", "---\ndescription: Where\nuser-invocable: true\n---\n!`pwd`");
    let args = SkillsMcpArgs::try_parse_from(["--dir", ".aether/skills"])?;
    let client = connect(SkillsMcp::from_args(args, workspace.root()), ClientOptions::default()).await;

    let prompt = client.get_prompt("where", Map::new()).await?;

    let text = prompt.messages[0].content.as_text().map(|content| content.text.trim()).unwrap_or_default();
    assert_eq!(canonicalize(text)?, canonicalize(workspace.root())?);
    assert!(SkillsMcpArgs::try_parse_from(Vec::<&str>::new()).is_err(), "--dir is required");
    Ok(())
}

#[tokio::test]
async fn list_skills_input_schema_has_properties_object() -> TestResult {
    let workspace = TestWorkspace::new();
    let client = serve(&workspace).await;

    let tools = client.list_tools().await?;
    let tool = tools.into_iter().find(|tool| tool.name.as_ref() == "list_skills").expect("list_skills tool present");

    let schema = Value::Object((*tool.input_schema).clone());
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"].is_object(), "properties must be an object: {schema}");
    Ok(())
}

async fn serve(workspace: &TestWorkspace) -> McpClient {
    connect(skills_server(workspace.root()), ClientOptions::default()).await
}

async fn get_skill(client: &McpClient, name: &str, path: Option<&str>) -> TestResult<Value> {
    let output = client.call("get_skills", load_skills_input(&[(name, path)])).await?;
    Ok(output["files"][0].clone())
}

fn skill(description: &str, body: &str) -> String {
    format!("---\ndescription: {description}\nagent-invocable: true\n---\n{body}")
}
