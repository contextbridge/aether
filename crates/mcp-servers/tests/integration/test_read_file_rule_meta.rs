use mcp_servers::coding::CodingMcp;
use mcp_servers::coding::tools::read_file::ReadFileArgs;
use mcp_servers::testing::TestWorkspace;

#[tokio::test]
async fn test_read_file_meta_includes_matched_rule_names_from_configured_rules_dirs() {
    let workspace = TestWorkspace::new()
        .file(
            ".claude/rules/writing-rust.md",
            "---\ndescription: Rust conventions\npaths:\n  - \"src/**/*.rs\"\n---\nRust best practices.\n",
        )
        .file("src/main.rs", "fn main() {}\n");
    let mcp = CodingMcp::new()
        .with_rules_dirs(vec![workspace.path(".claude/rules")])
        .with_root_dir(workspace.root().to_path_buf());

    let result = mcp
        .test_read_file(ReadFileArgs { file_path: workspace.path_string("src/main.rs"), offset: None, limit: None })
        .await
        .unwrap();

    let meta = result.0.meta.as_ref().expect("_meta should be set when rules match");
    assert_eq!(meta.display.title, "Read file");
    assert!(
        meta.display.value.contains("+rules: writing-rust"),
        "expected '+rules: writing-rust' in display value, got: {}",
        meta.display.value
    );
    assert!(
        result.0.content.contains("<system-reminder>\nRust best practices.\n</system-reminder>"),
        "expected injected system reminder in read_file output"
    );
}

#[tokio::test]
async fn test_read_file_does_not_auto_load_rules_without_rules_dirs() {
    let workspace = TestWorkspace::new()
        .file(
            ".aether/skills/writing-rust/SKILL.md",
            "---\ndescription: Rust conventions\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nRust best practices.\n",
        )
        .file("src/main.rs", "fn main() {}\n");
    let mcp = CodingMcp::new().with_root_dir(workspace.root().to_path_buf());

    let result = mcp
        .test_read_file(ReadFileArgs { file_path: workspace.path_string("src/main.rs"), offset: None, limit: None })
        .await
        .unwrap();

    let meta = result.0.meta.as_ref().expect("_meta should always be set");
    assert!(
        !meta.display.value.contains("+rules:"),
        "expected no '+rules:' in display value, got: {}",
        meta.display.value
    );
    assert!(
        !result.0.content.contains("<system-reminder>"),
        "expected no injected reminders when no --rules-dir is configured"
    );
}
