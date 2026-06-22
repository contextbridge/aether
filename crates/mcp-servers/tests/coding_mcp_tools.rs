mod common;

use common::{TestClient, TestResult, test_error};
use mcp_servers::coding::CodingMcp;
use mcp_servers::coding::tools::ast_grep::AstGrepInput;
use mcp_servers::coding::tools::bash::BashInput;
use mcp_servers::coding::tools::edit_file::EditFileArgs;
use mcp_servers::coding::tools::find::FindInput;
use mcp_servers::coding::tools::grep::GrepInput;
use mcp_servers::coding::tools::list_files::ListFilesArgs;
use mcp_servers::coding::tools::read_file::ReadFileArgs;
use mcp_servers::coding::tools::write_file::WriteFileArgs;
use mcp_servers::file_ops::FileEdit;
use std::fs::{self, canonicalize};

#[tokio::test]
async fn test_read_file_tool() -> TestResult {
    let mcp = TestClient::start(CodingMcp::new).await?;
    let test_content = "Hello, World!\nThis is a test file.";
    tokio::fs::write("/tmp/test_read_file.txt", test_content).await?;

    let parsed = mcp
        .call("read_file", ReadFileArgs { file_path: "/tmp/test_read_file.txt".to_string(), ..Default::default() })
        .await?;

    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["content"], "    1\tHello, World!\n    2\tThis is a test file.");
    assert_eq!(parsed["totalLines"], 2);
    assert_eq!(parsed["linesShown"], 2);

    let _ = tokio::fs::remove_file("/tmp/test_read_file.txt").await;
    Ok(())
}

#[tokio::test]
async fn test_write_file_tool() -> TestResult {
    let mcp = TestClient::start(CodingMcp::new).await?;
    let test_content = "This is test content written by the tool.";
    let test_path = "/tmp/test_write_file.txt";

    let parsed = mcp
        .call("write_file", WriteFileArgs { file_path: test_path.to_string(), content: test_content.to_string() })
        .await?;

    assert!(parsed["message"].as_str().unwrap().contains("Successfully wrote"));
    assert_eq!(parsed["bytesWritten"], test_content.len());
    assert_eq!(parsed["filePath"], test_path);

    let file_content = tokio::fs::read_to_string(test_path).await?;
    assert_eq!(file_content, test_content);

    let _ = tokio::fs::remove_file(test_path).await;
    Ok(())
}

#[tokio::test]
async fn test_bash_tool() -> TestResult {
    let mcp = TestClient::start(CodingMcp::new).await?;

    let parsed =
        mcp.call("bash", BashInput { command: "echo 'Hello from bash'".to_string(), ..Default::default() }).await?;

    assert_eq!(parsed["exitCode"], 0);
    assert!(parsed["output"].as_str().unwrap().contains("Hello from bash"));
    assert_eq!(parsed["killed"], false);
    Ok(())
}

#[tokio::test]
async fn test_edit_file_tool() -> TestResult {
    let mcp = TestClient::start(CodingMcp::new).await?;
    let test_path = "/tmp/test_edit_file.txt";
    let initial_content = "Hello, World!\nThis is a test.";
    tokio::fs::write(test_path, initial_content).await?;

    mcp.call("read_file", ReadFileArgs { file_path: test_path.to_string(), ..Default::default() }).await?;
    let parsed = mcp
        .call(
            "edit_file",
            EditFileArgs { file_path: test_path.to_string(), edits: vec![FileEdit::new("World", "Rust")] },
        )
        .await?;

    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["replacementsMade"], 1);
    let file_content = tokio::fs::read_to_string(test_path).await?;
    assert_eq!(file_content, "Hello, Rust!\nThis is a test.");

    tokio::fs::write(test_path, "test test test").await?;
    mcp.call("read_file", ReadFileArgs { file_path: test_path.to_string(), ..Default::default() }).await?;
    let parsed = mcp
        .call(
            "edit_file",
            EditFileArgs {
                file_path: test_path.to_string(),
                edits: vec![FileEdit {
                    old_string: "test".to_string(),
                    new_string: "TEST".to_string(),
                    replace_all: true,
                }],
            },
        )
        .await?;

    assert_eq!(parsed["replacementsMade"], 3);
    let file_content = tokio::fs::read_to_string(test_path).await?;
    assert_eq!(file_content, "TEST TEST TEST");

    let _ = tokio::fs::remove_file(test_path).await;
    Ok(())
}

#[tokio::test]
async fn test_list_files_tool() -> TestResult {
    let mcp = TestClient::start(CodingMcp::new).await?;
    let test_dir = "/tmp/test_list_files";
    let _ = fs::remove_dir_all(test_dir);
    fs::create_dir_all(test_dir)?;
    fs::write(format!("{test_dir}/file1.txt"), "content1")?;
    fs::write(format!("{test_dir}/file2.rs"), "fn main() {}")?;
    fs::create_dir(format!("{test_dir}/subdir"))?;
    fs::write(format!("{test_dir}/.hidden_file"), "hidden content")?;

    let parsed =
        mcp.call("list_files", ListFilesArgs { path: Some(test_dir.to_string()), ..Default::default() }).await?;

    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["totalCount"], 3);
    let files = parsed["files"].as_array().ok_or_else(|| test_error("Files should be an array"))?;
    let file_names: Vec<String> = files.iter().map(|f| f["name"].as_str().unwrap().to_string()).collect();
    assert!(file_names.contains(&"file1.txt".to_string()));
    assert!(file_names.contains(&"file2.rs".to_string()));
    assert!(file_names.contains(&"subdir".to_string()));
    assert!(!file_names.contains(&".hidden_file".to_string()));

    let parsed =
        mcp.call("list_files", ListFilesArgs { path: Some(test_dir.to_string()), include_hidden: Some(true) }).await?;
    assert_eq!(parsed["totalCount"], 4);

    let _ = fs::remove_dir_all(test_dir);
    Ok(())
}

#[tokio::test]
async fn test_list_files_uses_workspace_root_when_no_path_given() -> TestResult {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();
    fs::write(temp.path().join("alpha.txt"), "a")?;
    fs::write(temp.path().join("beta.rs"), "b")?;

    let mcp = TestClient::start(|| CodingMcp::new().with_root_dir(workspace)).await?;
    let parsed = mcp.call("list_files", ListFilesArgs::default()).await?;
    let files = parsed["files"].as_array().ok_or_else(|| test_error("Files should be an array"))?;
    let file_names: Vec<String> = files.iter().map(|f| f["name"].as_str().unwrap().to_string()).collect();

    assert!(file_names.contains(&"alpha.txt".to_string()));
    assert!(file_names.contains(&"beta.rs".to_string()));
    Ok(())
}

#[tokio::test]
async fn test_bash_pwd_uses_workspace_root() -> TestResult {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();
    let mcp = TestClient::start(|| CodingMcp::new().with_root_dir(workspace.clone())).await?;

    let parsed = mcp.call("bash", BashInput { command: "pwd".to_string(), ..Default::default() }).await?;
    let pwd = parsed["output"].as_str().ok_or_else(|| test_error("Expected output string"))?.trim();
    assert_eq!(canonicalize(pwd)?, canonicalize(&workspace)?);
    Ok(())
}

#[tokio::test]
async fn test_grep_and_find_use_workspace_root_when_no_path_given() -> TestResult {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();
    fs::write(temp.path().join("root_only.txt"), "needle\n")?;
    fs::write(temp.path().join("other.rs"), "fn main() {}\n")?;

    let mcp = TestClient::start(|| CodingMcp::new().with_root_dir(workspace.clone())).await?;
    let parsed = mcp.call("grep", GrepInput { pattern: "needle".to_string(), ..Default::default() }).await?;
    let matches = parsed["matches"].as_array().ok_or_else(|| test_error("matches should be an array"))?;
    assert!(matches.iter().any(|m| m["file"].as_str().unwrap().ends_with("root_only.txt")));

    let parsed = mcp.call("find", FindInput { pattern: "*.rs".to_string(), ..Default::default() }).await?;
    assert_eq!(parsed["searchPath"].as_str().unwrap(), workspace.to_str().unwrap());
    let matches = parsed["matches"].as_array().ok_or_else(|| test_error("matches should be an array"))?;
    assert!(matches.iter().any(|m| m.as_str().unwrap().ends_with("other.rs")));
    Ok(())
}

#[tokio::test]
async fn test_ast_grep_uses_workspace_root_when_no_path_given() -> TestResult {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();
    fs::write(workspace.join("lib.rs"), "fn target() {}\nfn other() {}\n")?;

    let mcp = TestClient::start(|| CodingMcp::new().with_root_dir(workspace.clone())).await?;
    let parsed = mcp
        .call(
            "ast_grep",
            AstGrepInput { language: "rs".to_string(), pattern: "fn $NAME() {}".to_string(), ..Default::default() },
        )
        .await?;
    let matches = parsed["matches"].as_array().ok_or_else(|| test_error("matches should be an array"))?;
    assert_eq!(parsed["searchPath"].as_str().unwrap(), workspace.to_str().unwrap());
    assert!(matches.iter().any(|m| {
        m["file"].as_str().unwrap().ends_with("lib.rs")
            && m["text"].as_str().unwrap() == "fn target() {}"
            && m["range"]["startLine"].as_u64().unwrap() == 1
            && m["captures"].as_array().unwrap().iter().any(|c| c["name"] == "NAME" && c["text"] == "target")
    }));

    Ok(())
}

#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn test_read_before_edit_safety_with_relative_path_normalization() -> TestResult {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();
    let test_file = temp.path().join("test.txt");
    fs::write(&test_file, "hello world")?;

    let server = CodingMcp::new().with_root_dir(workspace);

    server.test_read_file(ReadFileArgs { file_path: "test.txt".to_string(), ..Default::default() }).await?;

    let result = server
        .test_edit_file(EditFileArgs {
            file_path: "test.txt".to_string(),
            edits: vec![FileEdit::new("hello", "goodbye")],
        })
        .await;

    assert!(result.is_ok());
    Ok(())
}
