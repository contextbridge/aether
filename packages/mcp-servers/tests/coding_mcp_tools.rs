use mcp_servers::coding::CodingMcp;
use mcp_utils::testing::connect;
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation};
use std::fs::{self, canonicalize};

#[tokio::test]
async fn test_read_file_tool() {
    // Create server and client
    let server_service = CodingMcp::new();
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));

    let (_server_handle, client) =
        connect(server_service, client_info).await.expect("Failed to connect MCP server and client");

    // Create test file
    let test_content = "Hello, World!\nThis is a test file.";
    tokio::fs::write("/tmp/test_read_file.txt", test_content).await.expect("Failed to create test file");

    // Test read_file tool
    let result = client
        .call_tool(
            CallToolRequestParams::new("read_file").with_arguments(
                serde_json::json!({
                    "filePath": "/tmp/test_read_file.txt"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to call read_file tool");

    // Verify result
    assert!(result.content.len() == 1);
    if let Some(content) = result.content.first() {
        if let Some(text_content) = content.as_text() {
            let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");
            assert_eq!(parsed["status"], "success");

            // Verify line-numbered content format (should read full file by default)
            let expected_formatted = "    1\tHello, World!\n    2\tThis is a test file.";
            assert_eq!(parsed["content"], expected_formatted);
            assert_eq!(parsed["totalLines"], 2);
            assert_eq!(parsed["linesShown"], 2);
        } else {
            panic!("Expected text content");
        }
    } else {
        panic!("Expected content in result");
    }

    // Clean up
    let _ = tokio::fs::remove_file("/tmp/test_read_file.txt").await;
}

#[tokio::test]
async fn test_write_file_tool() {
    // Create server and client
    let server_service = CodingMcp::new();
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));

    let (_server_handle, client) =
        connect(server_service, client_info).await.expect("Failed to connect MCP server and client");

    let test_content = "This is test content written by the tool.";
    let test_path = "/tmp/test_write_file.txt";

    // Test write_file tool with new simplified API
    let result = client
        .call_tool(
            CallToolRequestParams::new("write_file").with_arguments(
                serde_json::json!({
                    "filePath": test_path,
                    "content": test_content
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to call write_file tool");

    // Verify result
    assert!(result.content.len() == 1);
    if let Some(content) = result.content.first() {
        if let Some(text_content) = content.as_text() {
            let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");
            assert!(parsed["message"].as_str().unwrap().contains("Successfully wrote"));
            assert_eq!(parsed["bytesWritten"], test_content.len());
            assert_eq!(parsed["filePath"], test_path);
        } else {
            panic!("Expected text content");
        }
    } else {
        panic!("Expected content in result");
    }

    // Verify file was actually written
    let file_content = tokio::fs::read_to_string(test_path).await.expect("Failed to read written file");
    assert_eq!(file_content, test_content);

    // Clean up
    let _ = tokio::fs::remove_file(test_path).await;
}

#[tokio::test]
async fn test_bash_tool() {
    // Create server and client
    let server_service = CodingMcp::new();
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));

    let (_server_handle, client) =
        connect(server_service, client_info).await.expect("Failed to connect MCP server and client");

    // Test bash tool with a simple command
    let result = client
        .call_tool(
            CallToolRequestParams::new("bash").with_arguments(
                serde_json::json!({
                    "command": "echo 'Hello from bash'"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to call bash tool");

    // Verify result
    assert!(result.content.len() == 1);
    if let Some(content) = result.content.first() {
        if let Some(text_content) = content.as_text() {
            let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");
            assert_eq!(parsed["exitCode"], 0);
            assert!(parsed["output"].as_str().unwrap().contains("Hello from bash"));
            assert_eq!(parsed["killed"], false);
        } else {
            panic!("Expected text content");
        }
    } else {
        panic!("Expected content in result");
    }
}

#[tokio::test]
async fn test_edit_file_tool() {
    // Create server and client
    let server_service = CodingMcp::new();
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));

    let (_server_handle, client) =
        connect(server_service, client_info).await.expect("Failed to connect MCP server and client");

    let test_path = "/tmp/test_edit_file.txt";
    let initial_content = "Hello, World!\nThis is a test.";

    // Create initial file
    tokio::fs::write(test_path, initial_content).await.expect("Failed to create test file");

    // First, read the file (required by safety check)
    client
        .call_tool(
            CallToolRequestParams::new("read_file").with_arguments(
                serde_json::json!({
                    "filePath": test_path
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to read file before edit");

    // Test edit_file tool - replace single occurrence
    let result = client
        .call_tool(
            CallToolRequestParams::new("edit_file").with_arguments(
                serde_json::json!({
                    "filePath": test_path,
                    "oldString": "World",
                    "newString": "Rust"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to call edit_file tool");

    // Verify result
    assert!(result.content.len() == 1);
    if let Some(content) = result.content.first() {
        if let Some(text_content) = content.as_text() {
            let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");
            assert_eq!(parsed["status"], "success");
            assert_eq!(parsed["replacementsMade"], 1);
        } else {
            panic!("Expected text content");
        }
    } else {
        panic!("Expected content in result");
    }

    // Verify file was actually edited
    let file_content = tokio::fs::read_to_string(test_path).await.expect("Failed to read edited file");
    assert_eq!(file_content, "Hello, Rust!\nThis is a test.");

    // Test replace_all flag
    tokio::fs::write(test_path, "test test test").await.expect("Failed to write test file");

    // Read the file again before editing
    client
        .call_tool(
            CallToolRequestParams::new("read_file").with_arguments(
                serde_json::json!({
                    "filePath": test_path
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to read file before second edit");

    let result = client
        .call_tool(
            CallToolRequestParams::new("edit_file").with_arguments(
                serde_json::json!({
                    "filePath": test_path,
                    "oldString": "test",
                    "newString": "TEST",
                    "replaceAll": true
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to call edit_file tool with replace_all");

    if let Some(text_content) = result.content.first().and_then(|c| c.as_text()) {
        let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");
        assert_eq!(parsed["replacementsMade"], 3);
    }

    let file_content = tokio::fs::read_to_string(test_path).await.expect("Failed to read edited file");
    assert_eq!(file_content, "TEST TEST TEST");

    // Clean up
    let _ = tokio::fs::remove_file(test_path).await;
}

#[tokio::test]
async fn test_list_files_tool() {
    // Create server and client
    let server_service = CodingMcp::new();
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));

    let (_server_handle, client) =
        connect(server_service, client_info).await.expect("Failed to connect MCP server and client");

    // Create test directory and files
    let test_dir = "/tmp/test_list_files";
    let _ = fs::remove_dir_all(test_dir); // Clean up any existing directory
    fs::create_dir_all(test_dir).expect("Failed to create test directory");

    // Create some test files
    fs::write(format!("{test_dir}/file1.txt"), "content1").expect("Failed to create test file 1");
    fs::write(format!("{test_dir}/file2.rs"), "fn main() {}").expect("Failed to create test file 2");
    fs::create_dir(format!("{test_dir}/subdir")).expect("Failed to create subdirectory");
    fs::write(format!("{test_dir}/.hidden_file"), "hidden content").expect("Failed to create hidden file");

    // Test list_files tool
    let result = client
        .call_tool(
            CallToolRequestParams::new("list_files").with_arguments(
                serde_json::json!({
                    "path": test_dir
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to call list_files tool");

    // Verify result
    assert!(result.content.len() == 1);
    if let Some(content) = result.content.first() {
        if let Some(text_content) = content.as_text() {
            let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");
            assert_eq!(parsed["status"], "success");
            assert_eq!(parsed["totalCount"], 3); // Should not include hidden file by default

            let files = parsed["files"].as_array().expect("Files should be an array");
            let file_names: Vec<String> = files.iter().map(|f| f["name"].as_str().unwrap().to_string()).collect();

            assert!(file_names.contains(&"file1.txt".to_string()));
            assert!(file_names.contains(&"file2.rs".to_string()));
            assert!(file_names.contains(&"subdir".to_string()));
            assert!(!file_names.contains(&".hidden_file".to_string())); // Hidden file should not be included
        } else {
            panic!("Expected text content");
        }
    } else {
        panic!("Expected content in result");
    }

    // Test including hidden files
    let result_with_hidden = client
        .call_tool(
            CallToolRequestParams::new("list_files").with_arguments(
                serde_json::json!({
                    "path": test_dir,
                    "includeHidden": true
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("Failed to call list_files tool with hidden files");

    if let Some(text_content) = result_with_hidden.content.first().and_then(|c| c.as_text()) {
        let parsed: serde_json::Value = serde_json::from_str(&text_content.text).expect("Invalid JSON response");
        assert_eq!(parsed["totalCount"], 4); // Should include hidden file now
    }

    // Clean up
    let _ = fs::remove_dir_all(test_dir);
}

#[tokio::test]
async fn test_list_files_uses_workspace_root_when_no_path_given() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();

    fs::write(temp.path().join("alpha.txt"), "a")?;
    fs::write(temp.path().join("beta.rs"), "b")?;

    let server_service = CodingMcp::new().with_root_dir(workspace.clone());
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));
    let (_server_handle, client) = connect(server_service, client_info).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("list_files").with_arguments(serde_json::json!({}).as_object().unwrap().clone()),
        )
        .await?;

    let text_content = result.content.first().and_then(|c| c.as_text()).ok_or("Expected text content")?;
    let parsed: serde_json::Value = serde_json::from_str(&text_content.text)?;
    let files = parsed["files"].as_array().ok_or("Files should be an array")?;
    let file_names: Vec<String> = files.iter().map(|f| f["name"].as_str().unwrap().to_string()).collect();

    assert!(file_names.contains(&"alpha.txt".to_string()));
    assert!(file_names.contains(&"beta.rs".to_string()));
    Ok(())
}

#[tokio::test]
async fn test_bash_pwd_uses_workspace_root() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();
    let server_service = CodingMcp::new().with_root_dir(workspace.clone());
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));
    let (_server_handle, client) = connect(server_service, client_info).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("bash").with_arguments(
                serde_json::json!({
                    "command": "pwd"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await?;

    let text_content = result.content.first().and_then(|c| c.as_text()).ok_or("Expected text content")?;
    let parsed: serde_json::Value = serde_json::from_str(&text_content.text)?;
    let pwd = parsed["output"].as_str().ok_or("Expected output string")?.trim();
    assert_eq!(canonicalize(pwd)?, canonicalize(&workspace)?);
    Ok(())
}

#[tokio::test]
async fn test_grep_and_find_use_workspace_root_when_no_path_given() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();
    fs::write(temp.path().join("root_only.txt"), "needle\n")?;
    fs::write(temp.path().join("other.rs"), "fn main() {}\n")?;

    let server_service = CodingMcp::new().with_root_dir(workspace.clone());
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));
    let (_server_handle, client) = connect(server_service, client_info).await?;
    let grep_result = client
        .call_tool(
            CallToolRequestParams::new("grep").with_arguments(
                serde_json::json!({
                    "pattern": "needle"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await?;

    let text_content = grep_result.content.first().and_then(|c| c.as_text()).ok_or("Expected text content")?;
    let parsed: serde_json::Value = serde_json::from_str(&text_content.text)?;
    let matches = parsed["matches"].as_array().ok_or("matches should be an array")?;
    assert!(matches.iter().any(|m| m["file"].as_str().unwrap().ends_with("root_only.txt")));

    let find_result = client
        .call_tool(
            CallToolRequestParams::new("find").with_arguments(
                serde_json::json!({
                    "pattern": "*.rs"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await?;

    let text_content = find_result.content.first().and_then(|c| c.as_text()).ok_or("Expected text content")?;
    let parsed: serde_json::Value = serde_json::from_str(&text_content.text)?;
    assert_eq!(parsed["searchPath"].as_str().unwrap(), workspace.to_str().unwrap());
    let matches = parsed["matches"].as_array().ok_or("matches should be an array")?;

    assert!(matches.iter().any(|m| m.as_str().unwrap().ends_with("other.rs")));
    Ok(())
}

#[tokio::test]
async fn test_ast_grep_uses_workspace_root_when_no_path_given() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().to_path_buf();
    fs::write(workspace.join("lib.rs"), "fn target() {}\nfn other() {}\n")?;

    let server_service = CodingMcp::new().with_root_dir(workspace.clone());
    let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));
    let (_server_handle, client) = connect(server_service, client_info).await?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("ast_grep").with_arguments(
                serde_json::json!({
                    "language": "rs",
                    "pattern": "fn $NAME() {}"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await?;

    let text_content = result.content.first().and_then(|c| c.as_text()).ok_or("Expected text content")?;
    let parsed: serde_json::Value = serde_json::from_str(&text_content.text)?;
    let matches = parsed["matches"].as_array().ok_or("matches should be an array")?;
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
async fn test_read_before_edit_safety_with_relative_path_normalization() {
    use mcp_servers::coding::tools::edit_file::EditFileArgs;
    use mcp_servers::coding::tools::read_file::ReadFileArgs;

    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().to_path_buf();
    let test_file = temp.path().join("test.txt");
    fs::write(&test_file, "hello world").unwrap();

    let server = CodingMcp::new().with_root_dir(workspace);

    server
        .test_read_file(ReadFileArgs { file_path: "test.txt".to_string(), offset: None, limit: None })
        .await
        .expect("read_file should succeed with relative path");

    let result = server
        .test_edit_file(EditFileArgs {
            file_path: "test.txt".to_string(),
            old_string: "hello".to_string(),
            new_string: "goodbye".to_string(),
            replace_all: false,
        })
        .await;

    assert!(result.is_ok());
}
