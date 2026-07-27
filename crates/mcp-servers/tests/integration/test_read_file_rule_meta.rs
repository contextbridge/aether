use mcp_servers::coding::CodingMcp;
use mcp_servers::coding::tools::read_file::ReadFileArgs;
use std::fs;
use tempfile::TempDir;

#[tokio::test]
async fn test_read_file_meta_includes_matched_rule_names_from_configured_rules_dirs() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    let rules_dir = root.join(".claude").join("rules");
    fs::create_dir_all(&rules_dir).unwrap();
    fs::write(
        rules_dir.join("writing-rust.md"),
        "---\ndescription: Rust conventions\npaths:\n  - \"src/**/*.rs\"\n---\nRust best practices.\n",
    )
    .unwrap();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let rs_file = src_dir.join("main.rs");
    fs::write(&rs_file, "fn main() {}\n").unwrap();

    let mcp = CodingMcp::new().with_rules_dirs(vec![rules_dir]).with_root_dir(root.to_path_buf());

    let result = mcp
        .test_read_file(ReadFileArgs { file_path: rs_file.to_string_lossy().to_string(), offset: None, limit: None })
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
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    let skill_dir = root.join(".aether").join("skills").join("writing-rust");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: Rust conventions\ntriggers:\n  read:\n    - \"**/*.rs\"\n---\nRust best practices.\n",
    )
    .unwrap();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let rs_file = src_dir.join("main.rs");
    fs::write(&rs_file, "fn main() {}\n").unwrap();

    let mcp = CodingMcp::new().with_root_dir(root.to_path_buf());

    let result = mcp
        .test_read_file(ReadFileArgs { file_path: rs_file.to_string_lossy().to_string(), offset: None, limit: None })
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
