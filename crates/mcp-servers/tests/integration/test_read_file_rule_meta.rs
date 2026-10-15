use crate::common::{CodingWorkspace, McpClientExt, TestResult};
use mcp_servers::coding::CodingMcp;
use mcp_servers::testing::TestWorkspace;
use serde_json::json;

#[tokio::test]
async fn read_file_appends_rules_matched_from_configured_rules_dirs() -> TestResult {
    let files = TestWorkspace::new()
        .file(
            ".claude/rules/writing-rust.md",
            "---\ndescription: Rust conventions\npaths:\n  - \"src/**/*.rs\"\n---\nRust best practices.\n",
        )
        .file("src/main.rs", "fn main() {}\n");
    let workspace =
        CodingWorkspace::with(files, |root| CodingMcp::new(root).with_rules_dirs(&[root.join(".claude/rules")])).await;

    let result = workspace.client.call("read_file", json!({ "filePath": workspace.path("src/main.rs") })).await?;

    assert_eq!(result["_meta"]["display"]["value"], "main.rs, 1 lines +rules: writing-rust");
    let content = result["content"].as_str().unwrap_or_default();
    assert!(content.contains("<system-reminder>\nRust best practices.\n</system-reminder>"), "{content}");
    Ok(())
}

#[tokio::test]
async fn read_file_does_not_auto_load_rules_without_rules_dirs() -> TestResult {
    let workspace = CodingWorkspace::new().await;
    workspace.write(
        ".aether/skills/writing-rust/SKILL.md",
        "---\ndescription: Rust conventions\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nRust best practices.\n",
    );
    let path = workspace.write("src/main.rs", "fn main() {}\n");

    let result = workspace.client.call("read_file", json!({ "filePath": path })).await?;

    assert_eq!(result["_meta"]["display"]["value"], "main.rs, 1 lines");
    assert!(!result["content"].as_str().unwrap_or_default().contains("<system-reminder>"), "{result}");
    Ok(())
}
