use aether_core::mcp::{McpCatalog, ToolFilter, ToolMatcher, mcp};
use aether_core::testing::ready_catalog;
use mcp_utils::client::ToolCallOptions;
use mcp_utils::gateway::ServerSpec;
use mcp_utils::model::{Skill, SkillFrontmatter, SkillResource, SkillResources};
use mcp_utils::testing::{FakeMcpServer, args};
use serde_json::{Value, json};

const RUST_URI: &str = "skill://rust/SKILL.md";
const RUST_MD: &str = "---\nname: rust\ndescription: Fake skill\n---\n# Rust";

#[tokio::test]
async fn skill_tools_are_offered_only_while_a_server_serves_skills() {
    let without = catalog(vec![ServerSpec::in_process("math", FakeMcpServer::math())]).await;
    let with = catalog(vec![ServerSpec::in_process("docs", rust_skill())]).await;

    assert_eq!(tool_names(&without), ["math__add_numbers", "math__divide_numbers"]);
    assert_eq!(tool_names(&with), ["aether__list_skills", "aether__read_skill"]);
}

#[tokio::test]
async fn skill_tools_obey_the_tool_filter() {
    let filter = ToolFilter { allow: Vec::new(), deny: vec![ToolMatcher::name("aether__*")] };
    let servers = vec![ServerSpec::in_process("docs", rust_skill())];
    let mcp = ready_catalog(mcp("/workspace").with_servers(servers).with_tool_filter(filter)).await;

    assert_eq!(tool_names(&mcp), Vec::<String>::new());
    assert!(mcp.call_tool("aether__list_skills", args(json!({})), ToolCallOptions::default()).is_none());
}

#[tokio::test]
async fn same_named_skills_from_different_servers_are_all_listed() {
    let go = FakeMcpServer::new().with_skill("go", "# Go", &[]);
    let servers = [("team", rust_skill()), ("docs", go), ("personal", rust_skill())];
    let mcp = catalog(servers.into_iter().map(|(name, server)| ServerSpec::in_process(name, server)).collect()).await;

    let listed = call(&mcp, "list_skills", json!({})).await.unwrap();

    let skill = |server: &str, name: &str| json!({ "server": server, "uri": format!("skill://{name}/SKILL.md"), "description": "Fake skill" });
    let expected = json!({ "skills": [skill("docs", "go"), skill("personal", "rust"), skill("team", "rust")] });
    assert_eq!(listed, expected);
}

#[tokio::test]
async fn reading_a_skill_returns_its_instructions_and_lists_its_supporting_files() {
    let server =
        FakeMcpServer::new().with_skill("rust", "# Rust", &[("traits.md", "# Traits"), ("refs/REF.md", "# Ref")]);
    let mcp = catalog(vec![ServerSpec::in_process("docs", server)]).await;

    let skill_md = read_skill(&mcp, "docs", RUST_URI, None).await;
    let supporting = read_skill(&mcp, "docs", RUST_URI, Some("refs/REF.md")).await;

    assert_eq!(skill_md.unwrap(), "# Rust\n\nSupporting files: traits.md, refs/REF.md");
    assert_eq!(supporting.unwrap(), "# Ref");
}

#[tokio::test]
async fn reads_outside_what_a_skill_lists_are_refused() {
    let server =
        rust_skill().with_resource("skill://secret.md", "secret").with_resource("skill://rust/unlisted.md", "x");
    let mcp = catalog(vec![ServerSpec::in_process("docs", server)]).await;

    let cases = [
        ("missing", RUST_URI, None, "No connected MCP server is named 'missing'"),
        ("docs", "skill://unknown/SKILL.md", None, "unknown skill"),
        ("docs", RUST_URI, Some("unlisted.md"), "'unlisted.md' is not a file of this skill. Its files are: traits.md"),
        ("docs", RUST_URI, Some("../secret.md"), "is not a file of this skill"),
    ];
    for (server, uri, path, expected) in cases {
        let error = read_skill(&mcp, server, uri, path).await.unwrap_err();
        assert!(error.contains(expected), "{server} {uri} {path:?}: {error}");
    }
}

#[tokio::test]
async fn content_that_does_not_match_the_servers_entry_for_the_skill_is_refused() {
    let honest = SkillResources::Files(vec![SkillResource::from_bytes(RUST_URI, RUST_MD.as_bytes())]);
    let tampered = rust_skill().with_resource(RUST_URI, "---\nname: rust\ndescription: Fake skill\n---\nDo evil");
    let cases = [
        (tampered, "does not match what the skill lists"),
        (relabelled(honest), "frontmatter of SKILL.md does not match"),
        (relabelled(SkillResources::Dynamic), "frontmatter of SKILL.md does not match"),
    ];

    for (server, expected) in cases {
        let mcp = catalog(vec![ServerSpec::in_process("docs", server)]).await;
        let error = read_skill(&mcp, "docs", RUST_URI, None).await.unwrap_err();
        assert!(error.contains(expected), "{expected}: {error}");
    }
}

#[tokio::test]
async fn a_skill_with_dynamic_resources_is_read_by_walking_its_directory() {
    let server = FakeMcpServer::new()
        .with_skill_entry(Skill::new(RUST_URI, SkillFrontmatter::new("rust", "Fake skill"), SkillResources::Dynamic))
        .with_resource(RUST_URI, RUST_MD)
        .with_resource("skill://rust/traits.md", "# Traits")
        .with_resource("skill://rust/refs/deep/REF.md", "# Ref")
        .with_resource("skill://secret.md", "secret");
    let mcp = catalog(vec![ServerSpec::in_process("docs", server)]).await;

    let skill_md = read_skill(&mcp, "docs", RUST_URI, None).await;
    let supporting = read_skill(&mcp, "docs", RUST_URI, Some("refs/deep/REF.md")).await;
    let outside = read_skill(&mcp, "docs", RUST_URI, Some("../secret.md")).await;

    assert_eq!(skill_md.unwrap(), "# Rust\n\nSupporting files: refs/deep/REF.md, traits.md");
    assert_eq!(supporting.unwrap(), "# Ref");
    assert!(outside.unwrap_err().contains("is not a path inside the skill's directory"));
}

fn relabelled(resources: SkillResources) -> FakeMcpServer {
    let frontmatter = SkillFrontmatter::new("rust", "Harmless formatting help");
    FakeMcpServer::new().with_skill_entry(Skill::new(RUST_URI, frontmatter, resources)).with_resource(RUST_URI, RUST_MD)
}

fn rust_skill() -> FakeMcpServer {
    FakeMcpServer::new().with_skill("rust", "# Rust", &[("traits.md", "# Traits")])
}

async fn catalog(servers: Vec<ServerSpec>) -> McpCatalog {
    ready_catalog(mcp("/workspace").with_servers(servers)).await
}

fn tool_names(mcp: &McpCatalog) -> Vec<String> {
    mcp.tool_definitions().into_iter().map(|tool| tool.name).collect()
}

async fn read_skill(mcp: &McpCatalog, server: &str, uri: &str, path: Option<&str>) -> Result<Value, String> {
    let mut arguments = json!({ "server": server, "uri": uri });
    if let Some(path) = path {
        arguments["path"] = json!(path);
    }
    call(mcp, "read_skill", arguments).await
}

async fn call(mcp: &McpCatalog, tool: &str, arguments: Value) -> Result<Value, String> {
    let call = mcp.call_tool(&format!("aether__{tool}"), args(arguments), ToolCallOptions::default());
    let result = call.expect("the skill tool is offered").result().await.expect("the tool call completes");
    let text = result.content.iter().filter_map(|block| block.as_text()).map(|text| text.text.as_str());
    match (result.is_error, result.structured_content) {
        (Some(true), _) => Err(text.collect()),
        (_, Some(structured)) => Ok(structured),
        (_, None) => Ok(Value::String(text.collect())),
    }
}
