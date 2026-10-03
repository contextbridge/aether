use crate::common::{CodingWorkspace, TestResult, test_error};
use aether_core::mcp::tool_bridge::convert_tool_result;
use llm::ToolCallRequest;
use mcp_servers::coding::tools::bash::BashInput;
use mcp_servers::coding::tools::read_file::ReadFileArgs;
use mcp_servers::coding::tools::web_fetch::{HttpResponse, WebFetchInput};
use mcp_servers::testing::FakeHttpClient;
use std::fs::{canonicalize, read_to_string};
use utils::temp_dir::TempDir;
use utils::tool_result_truncator::saved_path;

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
    assert_eq!(result["nextOffset"], 4);
    Ok(())
}

#[tokio::test]
async fn read_file_truncates_long_lines() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let long_path = workspace.write("long.txt", &format!("short\n{}", "x".repeat(2500)))?;
    let result = workspace
        .client
        .call("read_file", ReadFileArgs { file_path: long_path.to_string_lossy().into(), ..Default::default() })
        .await?;
    assert!(result["content"].as_str().unwrap().contains("[truncated, 2500 bytes total]"));
    Ok(())
}

#[tokio::test]
async fn full_read_file_pages_reach_the_llm_whole() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let content = (1..=5_000).map(|n| format!("let line_{n} = \"{}\";", "x".repeat(20))).collect::<Vec<_>>();
    let path = workspace.write("big.rs", &content.join("\n"))?;
    let page = workspace
        .client
        .call_raw("read_file", ReadFileArgs { file_path: path.to_string_lossy().into(), ..Default::default() })
        .await?;

    let request = ToolCallRequest { id: "read".into(), name: "coding__read_file".into(), arguments: "{}".into() };
    let (result, _) =
        convert_tool_result(&request, Ok(page), &TempDir::new()).map_err(|error| test_error(error.error))?;

    assert!(!result.result.contains("bytes omitted"), "{}", result.result);
    assert!(result.result.contains("nextOffset"), "{}", result.result);
    Ok(())
}

#[tokio::test]
async fn long_bash_output_keeps_head_and_tail_and_saves_the_full_output() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let result =
        workspace.client.call("bash", BashInput { command: "seq 1 20000".into(), ..Default::default() }).await?;
    let output = result["output"].as_str().unwrap();
    assert!(output.starts_with("1\n2\n3\n"), "{output}");
    assert!(output.ends_with("19999\n20000\n"), "{output}");
    let saved = saved_path(output).ok_or_else(|| test_error("output should name its saved file"))?;
    assert_eq!(read_to_string(saved)?, (1..=20000).map(|n| format!("{n}\n")).collect::<Vec<_>>().concat());
    assert_eq!(result["exitCode"], 0);
    Ok(())
}

#[tokio::test]
async fn long_web_pages_keep_head_and_tail_and_save_the_full_page() -> TestResult {
    let url = "https://example.com/long.txt";
    let page = (1..=20_000).map(|n| format!("line {n}\n")).collect::<Vec<_>>().concat();
    let response = HttpResponse {
        final_url: url.into(),
        status_code: 200,
        body: page.clone(),
        content_type: Some("text/plain".into()),
    };
    let workspace = CodingWorkspace::with_http(FakeHttpClient::new().with_response(url, response)).await?;

    let result =
        workspace.client.call("web_fetch", WebFetchInput { url: url.into(), prompt: None, timeout: None }).await?;

    let content = result["content"].as_str().unwrap();
    assert!(content.starts_with("line 1\nline 2\n"), "{content}");
    assert!(content.ends_with("line 19999\nline 20000\n"), "{content}");
    let saved = saved_path(content).ok_or_else(|| test_error("content should name its saved file"))?;
    assert_eq!(read_to_string(saved)?, page);
    Ok(())
}

#[tokio::test]
async fn short_output_is_returned_whole() -> TestResult {
    let workspace = CodingWorkspace::new().await?;
    let result = workspace.client.call("bash", BashInput { command: "seq 1 3".into(), ..Default::default() }).await?;
    assert_eq!(result["output"], "1\n2\n3\n");
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
    for retained in ["bash", "read_file", "write_file", "edit_file", "lsp_symbol"] {
        assert!(names.contains(&retained), "missing tool {retained}: {names:?}");
    }
    for removed in ["find", "grep", "ast_grep", "list_files", "lsp_document", "lsp_workspace_search"] {
        assert!(!names.contains(&removed), "unexpected removed tool {removed}: {names:?}");
    }
    Ok(())
}
