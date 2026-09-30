use crate::common::{CodingWorkspace, TestResult, test_error};
use mcp_servers::coding::tools::bash::BashInput;
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
    assert!(result["content"].as_str().unwrap().contains("[truncated, 2500 bytes total]"));
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
async fn coding_tool_catalog_uses_bash_for_search_and_directory_listings() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let catalog = workspace.client.raw().list_tools(None).await?;
    let names: Vec<_> = catalog.tools.iter().map(|tool| tool.name.as_ref()).collect();
    for retained in ["bash", "read_file", "write_file", "edit_file", "lsp_workspace_search"] {
        assert!(names.contains(&retained), "missing tool {retained}: {names:?}");
    }
    for removed in ["find", "grep", "ast_grep", "list_files"] {
        assert!(!names.contains(&removed), "unexpected removed tool {removed}: {names:?}");
    }
    Ok(())
}
