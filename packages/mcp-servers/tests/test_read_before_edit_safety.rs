use mcp_servers::coding::CodingMcp;
use mcp_servers::coding::tools::edit_file::EditFileArgs;
use mcp_servers::coding::tools::read_file::ReadFileArgs;
use mcp_servers::coding::tools::write_file::WriteFileArgs;
use mcp_servers::file_ops::FileEdit;
use std::fs;
use tempfile::TempDir;

fn replace(old: &str, new: &str) -> Vec<FileEdit> {
    vec![FileEdit::new(old, new)]
}

/// Test that editing a file without reading it first fails
#[tokio::test]
async fn test_edit_file_without_read_fails() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.txt");
    fs::write(&test_file, "original content").unwrap();

    let mcp = CodingMcp::new();

    // Try to edit without reading first
    let edit_args =
        EditFileArgs { file_path: test_file.to_string_lossy().to_string(), edits: replace("original", "modified") };

    let result = mcp.test_edit_file(edit_args).await;

    assert!(result.is_err());
    if let Err(err) = result {
        assert!(err.contains("Safety check failed"));
        assert!(err.contains("must use read_file"));
    }
}

/// Test that editing a file after reading it succeeds
#[tokio::test]
async fn test_edit_file_after_read_succeeds() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.txt");
    fs::write(&test_file, "original content").unwrap();

    let mcp = CodingMcp::new();

    // First read the file
    let read_args = ReadFileArgs { file_path: test_file.to_string_lossy().to_string(), offset: None, limit: None };
    let read_result = mcp.test_read_file(read_args).await;
    assert!(read_result.is_ok());

    // Now edit should succeed
    let edit_args =
        EditFileArgs { file_path: test_file.to_string_lossy().to_string(), edits: replace("original", "modified") };

    let result = mcp.test_edit_file(edit_args).await;
    assert!(result.is_ok());

    // Verify the edit was applied
    let content = fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "modified content");
}

/// Test that writing to an existing file without reading it first fails
#[tokio::test]
async fn test_write_existing_file_without_read_fails() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.txt");
    fs::write(&test_file, "original content").unwrap();

    let mcp = CodingMcp::new();

    // Try to write without reading first
    let write_args =
        WriteFileArgs { file_path: test_file.to_string_lossy().to_string(), content: "new content".to_string() };

    let result = mcp.test_write_file(write_args).await;

    assert!(result.is_err());
    if let Err(err) = result {
        assert!(err.contains("Safety check failed"));
        assert!(err.contains("already exists"));
    }
}

/// Test that writing to an existing file after reading it succeeds
#[tokio::test]
async fn test_write_existing_file_after_read_succeeds() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.txt");
    fs::write(&test_file, "original content").unwrap();

    let mcp = CodingMcp::new();

    // First read the file
    let read_args = ReadFileArgs { file_path: test_file.to_string_lossy().to_string(), offset: None, limit: None };
    let read_result = mcp.test_read_file(read_args).await;
    assert!(read_result.is_ok());

    // Now write should succeed
    let write_args =
        WriteFileArgs { file_path: test_file.to_string_lossy().to_string(), content: "new content".to_string() };

    let result = mcp.test_write_file(write_args).await;
    assert!(result.is_ok());

    // Verify the write was applied
    let content = fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "new content");
}

/// Test that writing to a new file (that doesn't exist) succeeds without read
#[tokio::test]
async fn test_write_new_file_without_read_succeeds() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("new_file.txt");

    let mcp = CodingMcp::new();

    // Write to a file that doesn't exist - should succeed
    let write_args =
        WriteFileArgs { file_path: test_file.to_string_lossy().to_string(), content: "new file content".to_string() };

    let result = mcp.test_write_file(write_args).await;
    assert!(result.is_ok());

    // Verify the file was created
    assert!(test_file.exists());
    let content = fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "new file content");
}

/// Test that reading tracks multiple files independently
#[tokio::test]
async fn test_multiple_files_tracked_independently() {
    let temp_dir = TempDir::new().unwrap();
    let file1 = temp_dir.path().join("file1.txt");
    let file2 = temp_dir.path().join("file2.txt");
    fs::write(&file1, "content 1").unwrap();
    fs::write(&file2, "content 2").unwrap();

    let mcp = CodingMcp::new();

    // Read file1
    let read_args = ReadFileArgs { file_path: file1.to_string_lossy().to_string(), offset: None, limit: None };
    mcp.test_read_file(read_args).await.unwrap();

    // Edit file1 should succeed
    let edit_args = EditFileArgs { file_path: file1.to_string_lossy().to_string(), edits: replace("1", "one") };
    assert!(mcp.test_edit_file(edit_args).await.is_ok());

    // Edit file2 should fail (not read yet)
    let edit_args = EditFileArgs { file_path: file2.to_string_lossy().to_string(), edits: replace("2", "two") };
    assert!(mcp.test_edit_file(edit_args).await.is_err());
}

/// Test that reading a file that doesn't exist doesn't track it
#[tokio::test]
async fn test_failed_read_doesnt_track_file() {
    let temp_dir = TempDir::new().unwrap();
    let nonexistent_file = temp_dir.path().join("doesnt_exist.txt");

    let mcp = CodingMcp::new();

    // Try to read a file that doesn't exist
    let read_args =
        ReadFileArgs { file_path: nonexistent_file.to_string_lossy().to_string(), offset: None, limit: None };
    let result = mcp.test_read_file(read_args).await;
    assert!(result.is_err());

    // Create the file now
    fs::write(&nonexistent_file, "content").unwrap();

    // Edit should still fail because the read failed
    let edit_args = EditFileArgs {
        file_path: nonexistent_file.to_string_lossy().to_string(),
        edits: replace("content", "modified"),
    };
    let result = mcp.test_edit_file(edit_args).await;
    assert!(result.is_err());
}
