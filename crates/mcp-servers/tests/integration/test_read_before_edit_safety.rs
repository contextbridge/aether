use mcp_servers::DefaultCodingTools;
use mcp_servers::coding::CodingMcp;
use mcp_servers::coding::error::CodingError;
use mcp_servers::coding::tools::edit_file::EditFileArgs;
use mcp_servers::coding::tools::read_file::ReadFileArgs;
use mcp_servers::coding::tools::write_file::WriteFileArgs;
use mcp_servers::file_ops::FileEdit;
use mcp_servers::testing::TestWorkspace;
use std::fs;

fn replace(old: &str, new: &str) -> Vec<FileEdit> {
    vec![FileEdit::new(old, new)]
}

fn workspace_with_test_file() -> (TestWorkspace, CodingMcp<DefaultCodingTools>) {
    let workspace = TestWorkspace::new().file("test.txt", "original content");
    let mcp = CodingMcp::new().with_root_dir(workspace.root().to_path_buf());
    (workspace, mcp)
}

fn read_args(workspace: &TestWorkspace, path: &str) -> ReadFileArgs {
    ReadFileArgs { file_path: workspace.path_string(path), offset: None, limit: None }
}

fn edit_args(workspace: &TestWorkspace, path: &str, edits: Vec<FileEdit>) -> EditFileArgs {
    EditFileArgs { file_path: workspace.path_string(path), edits }
}

fn write_args(workspace: &TestWorkspace, path: &str, content: &str) -> WriteFileArgs {
    WriteFileArgs { file_path: workspace.path_string(path), content: content.to_string() }
}

#[tokio::test]
async fn test_edit_file_without_read_fails() {
    let (workspace, mcp) = workspace_with_test_file();

    let Err(CodingError::NotReadBeforeEdit(path)) =
        mcp.test_edit_file(edit_args(&workspace, "test.txt", replace("original", "modified"))).await
    else {
        panic!("edit_file without a prior read_file should fail the safety check");
    };
    assert_eq!(path, workspace.path_string("test.txt"));
}

#[tokio::test]
async fn test_edit_file_after_read_succeeds() {
    let (workspace, mcp) = workspace_with_test_file();

    mcp.test_read_file(read_args(&workspace, "test.txt")).await.unwrap();

    mcp.test_edit_file(edit_args(&workspace, "test.txt", replace("original", "modified"))).await.unwrap();

    assert_eq!(fs::read_to_string(workspace.path("test.txt")).unwrap(), "modified content");
}

#[tokio::test]
async fn test_write_existing_file_without_read_fails() {
    let (workspace, mcp) = workspace_with_test_file();

    let Err(CodingError::NotReadBeforeOverwrite(path)) =
        mcp.test_write_file(write_args(&workspace, "test.txt", "new content")).await
    else {
        panic!("write_file to an unread existing file should fail the safety check");
    };
    assert_eq!(path, workspace.path_string("test.txt"));
}

#[tokio::test]
async fn test_write_existing_file_after_read_succeeds() {
    let (workspace, mcp) = workspace_with_test_file();

    mcp.test_read_file(read_args(&workspace, "test.txt")).await.unwrap();

    mcp.test_write_file(write_args(&workspace, "test.txt", "new content")).await.unwrap();

    assert_eq!(fs::read_to_string(workspace.path("test.txt")).unwrap(), "new content");
}

#[tokio::test]
async fn test_write_new_file_without_read_succeeds() {
    let workspace = TestWorkspace::new();
    let mcp = CodingMcp::new().with_root_dir(workspace.root().to_path_buf());

    mcp.test_write_file(write_args(&workspace, "new_file.txt", "new file content")).await.unwrap();

    assert_eq!(fs::read_to_string(workspace.path("new_file.txt")).unwrap(), "new file content");
}

#[tokio::test]
async fn test_multiple_files_tracked_independently() {
    let workspace = TestWorkspace::new().file("file1.txt", "content 1").file("file2.txt", "content 2");
    let mcp = CodingMcp::new().with_root_dir(workspace.root().to_path_buf());

    mcp.test_read_file(read_args(&workspace, "file1.txt")).await.unwrap();

    assert!(mcp.test_edit_file(edit_args(&workspace, "file1.txt", replace("1", "one"))).await.is_ok());
    assert!(mcp.test_edit_file(edit_args(&workspace, "file2.txt", replace("2", "two"))).await.is_err());
}

#[tokio::test]
async fn test_failed_read_doesnt_track_file() {
    let workspace = TestWorkspace::new();
    let mcp = CodingMcp::new().with_root_dir(workspace.root().to_path_buf());

    assert!(mcp.test_read_file(read_args(&workspace, "doesnt_exist.txt")).await.is_err());

    fs::write(workspace.path("doesnt_exist.txt"), "content").unwrap();

    assert!(
        mcp.test_edit_file(edit_args(&workspace, "doesnt_exist.txt", replace("content", "modified"))).await.is_err(),
        "a failed read must not start tracking the file"
    );
}
