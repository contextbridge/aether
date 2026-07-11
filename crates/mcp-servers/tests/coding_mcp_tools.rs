mod common;

use common::{CodingWorkspace, TestResult, test_error};
use mcp_servers::coding::tools::ast_grep::AstGrepInput;
use mcp_servers::coding::tools::bash::BashInput;
use mcp_servers::coding::tools::find::FindInput;
use mcp_servers::coding::tools::grep::GrepInput;
use mcp_servers::coding::tools::list_files::ListFilesArgs;
use mcp_servers::coding::tools::read_file::ReadFileArgs;
use std::fs::canonicalize;

#[tokio::test]
async fn read_file_supports_paging_and_snake_case_arguments() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let path = workspace.write("notes.txt", "line 1\nline 2\nline 3\nline 4\nline 5")?;
    let result =
        workspace.client.call("read_file", serde_json::json!({ "file_path": path, "offset": 2, "limit": 2 })).await?;
    assert_eq!(result["status"], "success");
    assert_eq!(result["content"], "    2\tline 2\n    3\tline 3");
    assert_eq!(result["totalLines"], 5);
    assert_eq!(result["linesShown"], 2);
    assert_eq!(result["offset"], 2);
    assert_eq!(result["limit"], 2);
    Ok(())
}

#[tokio::test]
async fn read_file_truncates_lines_and_applies_default_limit() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let long_path = workspace.write("long.txt", &format!("short\n{}", "x".repeat(2500)))?;
    let result = workspace
        .client
        .call("read_file", ReadFileArgs { file_path: long_path.to_string_lossy().into(), ..Default::default() })
        .await?;
    assert!(result["content"].as_str().unwrap().contains("[truncated, 2500 chars total]"));
    let content = (1..=2001).map(|line| format!("Line {line}")).collect::<Vec<_>>().join("\n");
    let capped_path = workspace.write("capped.txt", &content)?;
    let result = workspace
        .client
        .call("read_file", ReadFileArgs { file_path: capped_path.to_string_lossy().into(), ..Default::default() })
        .await?;
    assert_eq!(result["totalLines"], 2001);
    assert_eq!(result["linesShown"], 2000);
    assert_eq!(result["limit"], 2000);
    assert!(result["content"].as_str().unwrap().contains(" 2000\tLine 2000"));
    assert!(!result["content"].as_str().unwrap().contains("Line 2001"));
    Ok(())
}

#[tokio::test]
async fn read_file_reports_invalid_and_missing_paths() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let path = workspace.write("present.txt", "present")?;
    let invalid = workspace.client.call_raw("read_file", serde_json::json!({ "filePath": path, "offset": 0 })).await?;
    assert!(invalid.is_error.unwrap_or(false), "invalid offset should fail: {invalid:?}");
    let missing = workspace
        .client
        .call_raw("read_file", serde_json::json!({ "filePath": workspace.path("missing.txt") }))
        .await?;
    assert!(missing.is_error.unwrap_or(false), "missing file should fail: {missing:?}");
    Ok(())
}

#[tokio::test]
async fn write_file_handles_empty_content_and_overwrites() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let path = workspace.path("nested/output.txt");
    let result = workspace.client.call("write_file", serde_json::json!({ "file_path": path, "content": "" })).await?;
    assert_eq!(result["bytesWritten"], 0);
    assert_eq!(result["_meta"]["file_diff"]["old_text"], serde_json::Value::Null);
    assert_eq!(result["_meta"]["file_diff"]["new_text"], "");
    workspace
        .client
        .call("read_file", ReadFileArgs { file_path: path.to_string_lossy().into(), ..Default::default() })
        .await?;
    let result =
        workspace.client.call("write_file", serde_json::json!({ "filePath": path, "content": "new content" })).await?;
    assert_eq!(result["bytesWritten"], 11);
    assert_eq!(result["_meta"]["file_diff"]["new_text"], "new content");
    assert_eq!(workspace.read("nested/output.txt")?, "new content");
    Ok(())
}

#[tokio::test]
async fn edit_file_supports_batch_edits_and_response_metadata() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let path = workspace.write("source.txt", "alpha\nbeta\ngamma\n")?;
    workspace
        .client
        .call("read_file", ReadFileArgs { file_path: path.to_string_lossy().into(), ..Default::default() })
        .await?;
    let result = workspace
        .client
        .call(
            "edit_file",
            serde_json::json!({
                "file_path": path,
                "edits": [
                    { "old_string": "alpha", "new_string": "ALPHA" },
                    { "old_string": "gamma", "new_string": "GAMMA" }
                ]
            }),
        )
        .await?;
    assert_eq!(result["status"], "success");
    assert_eq!(result["replacementsMade"], 2);
    assert_eq!(result["_meta"]["file_diff"]["old_text"], "alpha\nbeta\ngamma\n");
    assert_eq!(result["_meta"]["file_diff"]["new_text"], "ALPHA\nbeta\nGAMMA\n");
    assert_eq!(workspace.read("source.txt")?, "ALPHA\nbeta\nGAMMA\n");
    Ok(())
}

#[tokio::test]
async fn test_bash_pwd_uses_workspace_root() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let parsed = workspace.client.call("bash", BashInput { command: "pwd".to_string(), ..Default::default() }).await?;
    let pwd = parsed["output"].as_str().ok_or_else(|| test_error("Expected output string"))?.trim();
    assert_eq!(canonicalize(pwd)?, canonicalize(workspace.root())?);
    Ok(())
}

#[tokio::test]
async fn test_list_files_tool() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    workspace.write("file1.txt", "content1")?;
    workspace.write("file2.rs", "fn main() {}")?;
    std::fs::create_dir(workspace.path("subdir"))?;
    workspace.write(".hidden_file", "hidden content")?;
    let parsed = workspace.client.call("list_files", ListFilesArgs::default()).await?;
    assert_eq!(parsed["totalCount"], 3);
    let names: Vec<&str> = parsed["files"]
        .as_array()
        .ok_or_else(|| test_error("Files should be an array"))?
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"file1.txt") && names.contains(&"file2.rs") && names.contains(&"subdir"));
    let parsed =
        workspace.client.call("list_files", ListFilesArgs { include_hidden: Some(true), ..Default::default() }).await?;
    assert_eq!(parsed["totalCount"], 4);
    Ok(())
}

#[tokio::test]
async fn test_grep_and_find_use_workspace_root_when_no_path_given() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    workspace.write("root_only.txt", "needle\n")?;
    workspace.write("other.rs", "fn main() {}\n")?;
    let parsed =
        workspace.client.call("grep", GrepInput { pattern: "needle".to_string(), ..Default::default() }).await?;
    assert!(
        parsed["matches"].as_array().unwrap().iter().any(|m| m["file"].as_str().unwrap().ends_with("root_only.txt"))
    );
    let parsed = workspace.client.call("find", FindInput { pattern: "*.rs".to_string(), ..Default::default() }).await?;
    assert_eq!(parsed["searchPath"], workspace.root().to_str().unwrap());
    assert!(parsed["matches"].as_array().unwrap().iter().any(|m| m.as_str().unwrap().ends_with("other.rs")));
    Ok(())
}

#[tokio::test]
async fn test_ast_grep_uses_workspace_root_when_no_path_given() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    workspace.write("lib.rs", "fn target() {}\nfn other() {}\n")?;
    let parsed = workspace
        .client
        .call(
            "ast_grep",
            AstGrepInput { language: "rs".to_string(), pattern: "fn $NAME() {}".to_string(), ..Default::default() },
        )
        .await?;
    assert_eq!(parsed["searchPath"], workspace.root().to_str().unwrap());
    assert!(parsed["matches"].as_array().unwrap().iter().any(|m| m["text"] == "fn target() {}"));
    Ok(())
}
