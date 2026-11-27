use crate::common::{TestResult, connect, skills_server};
use clap::Parser;
use mcp_servers::skills::{SkillsMcp, SkillsMcpArgs};
use mcp_servers::testing::TestWorkspace;
use mcp_utils::client::{ClientOptions, McpClient};
use rmcp::model::{ResourceContents, SkillResource, SkillResources};
use serde_json::{Map, Value, json};
use std::fs::{self, canonicalize};

#[tokio::test]
async fn lists_agent_invocable_directory_skills_with_their_frontmatter() -> TestResult {
    let workspace = TestWorkspace::new()
        .file("skills/zeta/SKILL.md", "---\nname: zeta\ndescription: Zeta\ntags:\n  - systems\n---\n# Zeta")
        .file("skills/alpha/SKILL.md", skill("alpha", "# Alpha"))
        .file(
            "skills/user-only/SKILL.md",
            "---\nname: user-only\ndescription: User only\nuser-invocable: true\nagent-invocable: false\n---\n",
        )
        .file("skills/flat.md", skill("flat", "# Flat"));
    let client = serve(&workspace).await;

    let listed = client.list_skills().await?;

    let listed: Vec<_> = listed.iter().map(|skill| (skill.uri.as_str(), json!(skill.frontmatter))).collect();
    assert_eq!(
        listed,
        [
            ("skill://alpha/SKILL.md", json!({ "name": "alpha", "description": "alpha skill" })),
            ("skill://zeta/SKILL.md", json!({ "name": "zeta", "description": "Zeta", "tags": ["systems"] })),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn skills_that_are_not_agent_invocable_cannot_be_fetched_by_uri() -> TestResult {
    let hidden = "---\nname: hidden\ndescription: Hidden\nagent-invocable: false\n---\n";
    let workspace = TestWorkspace::new().file("skills/hidden/SKILL.md", hidden);
    let client = serve(&workspace).await;

    assert!(client.get_skill("skill://hidden/SKILL.md").await.is_err());
    assert!(client.get_skill("skill://missing/SKILL.md").await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_skill_lists_every_file_it_ships_and_serves_them_verbatim() -> TestResult {
    let skill_md = skill("rust", "value: !`echo not-expanded`");
    let workspace = TestWorkspace::new()
        .file("skills/rust/SKILL.md", &skill_md)
        .file("skills/rust/traits.md", "# Traits")
        .file("skills/rust/references/REF.md", "# Ref")
        .file("skills/rust/.hidden", "ignored");
    let client = serve(&workspace).await;

    let skill = client.get_skill("skill://rust/SKILL.md").await?;

    let SkillResources::Files(files) = &skill.resources else { panic!("expected a file manifest: {skill:?}") };
    let expected = [("SKILL.md", skill_md.as_str()), ("references/REF.md", "# Ref"), ("traits.md", "# Traits")];
    assert_eq!(files.len(), expected.len(), "{files:?}");
    for (file, (path, content)) in files.iter().zip(expected) {
        let uri = format!("skill://rust/{path}");
        assert_eq!(file, &SkillResource::from_bytes(&uri, content.as_bytes()));
        assert_eq!(read_text(&client, &uri).await?, (uri, content.to_string()));
    }
    assert!(client.read_resource("skill://rust/.hidden").await.is_err());
    Ok(())
}

#[tokio::test]
async fn later_skill_dirs_extend_and_override_earlier_ones() -> TestResult {
    let workspace = TestWorkspace::new()
        .file("a/rust/SKILL.md", skill("rust", "From A"))
        .file("a/go/SKILL.md", skill("go", "Go"))
        .file("b/rust/SKILL.md", skill("rust", "From B"));
    let dirs = ["missing", "a", "b"].map(|dir| workspace.path(dir));
    let client = connect(SkillsMcp::new(workspace.root(), &dirs), ClientOptions::default()).await;

    let listed = client.list_skills().await?;

    let uris: Vec<_> = listed.iter().map(|skill| skill.uri.as_str()).collect();
    assert_eq!(uris, ["skill://go/SKILL.md", "skill://rust/SKILL.md"]);
    let rust_md = skill("rust", "From B");
    assert_eq!(
        listed[1].resources,
        SkillResources::Files(vec![SkillResource::from_bytes(uris[1], rust_md.as_bytes())])
    );
    assert_eq!(read_text(&client, "skill://rust/SKILL.md").await?.1, rust_md);
    Ok(())
}

#[tokio::test]
async fn listing_picks_up_skills_edited_since_the_server_started() -> TestResult {
    let workspace = TestWorkspace::new().file("skills/rust/SKILL.md", skill("rust", "Old"));
    let client = serve(&workspace).await;
    assert_eq!(client.list_skills().await?.len(), 1);

    fs::write(workspace.path("skills/rust/SKILL.md"), skill("rust", "New"))?;
    fs::create_dir(workspace.path("skills/go"))?;
    fs::write(workspace.path("skills/go/SKILL.md"), skill("go", "Go"))?;

    assert_eq!(client.list_skills().await?.len(), 2);
    assert_eq!(read_text(&client, "skill://rust/SKILL.md").await?.1, skill("rust", "New"));
    Ok(())
}

#[tokio::test]
async fn from_args_resolves_dirs_and_runs_prompts_from_the_base_dir() -> TestResult {
    let workspace = TestWorkspace::new()
        .file(".aether/skills/where/SKILL.md", "---\ndescription: Where\nuser-invocable: true\n---\n!`pwd`");
    let args = SkillsMcpArgs::try_parse_from(["--dir", ".aether/skills"])?;
    let client = connect(SkillsMcp::from_args(args, workspace.root()), ClientOptions::default()).await;

    let prompt = client.get_prompt("where", Map::new()).await?;

    let text = prompt.messages[0].content.as_text().map_or_default(|content| content.text.trim());
    assert_eq!(canonicalize(text)?, canonicalize(workspace.root())?);
    assert!(SkillsMcpArgs::try_parse_from(Vec::<&str>::new()).is_err(), "--dir is required");
    Ok(())
}

#[tokio::test]
async fn prompt_arguments_are_substituted_after_shell_expansion() -> TestResult {
    let workspace = TestWorkspace::new().file(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nuser-invocable: true\n---\nbranch: !`echo main`\nrequest: $ARGUMENTS",
    );
    let client = serve(&workspace).await;

    let text = prompt_text(&client, "review", json!({ "ARGUMENTS": "!`touch pwned`" })).await?;

    assert_eq!(text, "branch: main\nrequest: !`touch pwned`");
    assert!(!workspace.root().join("pwned").exists(), "argument text must not run as a shell command");
    Ok(())
}

#[tokio::test]
async fn prompt_substitutes_provided_arguments_and_keeps_the_rest() -> TestResult {
    let workspace = TestWorkspace::new().file(
        "skills/review/SKILL.md",
        "---\ndescription: Review\nuser-invocable: true\n---\nReview $1 in $2 with $tool",
    );
    let client = serve(&workspace).await;

    for (arguments, expected) in [
        (json!({ "1": "Rust", "2": "Actix" }), "Review Rust in Actix with $tool"),
        (json!({ "unused": "value" }), "Review $1 in $2 with $tool"),
        (json!({}), "Review $1 in $2 with $tool"),
    ] {
        assert_eq!(prompt_text(&client, "review", arguments.clone()).await?, expected, "{arguments}");
    }
    Ok(())
}

async fn serve(workspace: &TestWorkspace) -> McpClient {
    connect(skills_server(workspace.root()), ClientOptions::default()).await
}

async fn prompt_text(client: &McpClient, name: &str, arguments: Value) -> TestResult<String> {
    let Value::Object(arguments) = arguments else { panic!("prompt arguments must be an object") };
    let prompt = client.get_prompt(name, arguments).await?;
    Ok(prompt.messages[0].content.as_text().map_or_default(|content| content.text.clone()))
}

async fn read_text(client: &McpClient, uri: &str) -> TestResult<(String, String)> {
    match client.read_resource(uri).await?.as_slice() {
        [ResourceContents::TextResourceContents { uri, text, .. }] => Ok((uri.clone(), text.clone())),
        contents => panic!("expected one text resource: {contents:?}"),
    }
}

fn skill(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} skill\n---\n{body}")
}
