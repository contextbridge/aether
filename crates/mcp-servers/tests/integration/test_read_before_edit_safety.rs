use crate::common::{CodingWorkspace, McpClientExt, TestResult, call_tool_error};
use serde_json::{Value, json};
use std::path::Path;

#[tokio::test]
async fn existing_files_must_be_read_before_they_are_edited_or_overwritten() -> TestResult {
    let workspace = CodingWorkspace::new().await;
    let path = workspace.write("test.txt", "original content");

    let edit_error = call_tool_error(&workspace.client, "edit_file", edit(&path, "original", "edited")).await;
    let write_error = call_tool_error(&workspace.client, "write_file", write(&path, "written")).await;

    let path = path.display();
    assert!(edit_error.contains(&format!("You must use read_file on '{path}' before editing it")), "{edit_error}");
    assert!(write_error.contains(&format!("File '{path}' already exists. You must use read_file")), "{write_error}");
    assert_eq!(workspace.read("test.txt")?, "original content");
    Ok(())
}

#[tokio::test]
async fn only_successfully_read_files_unlock_editing() -> TestResult {
    let workspace = CodingWorkspace::new().await;
    let read_path = workspace.write("read.txt", "content");
    let unread_path = workspace.write("unread.txt", "content");
    let late_path = workspace.path("late.txt");

    workspace.client.call("read_file", read(&read_path)).await?;
    call_tool_error(&workspace.client, "read_file", read(&late_path)).await;
    workspace.write("late.txt", "content");

    workspace.client.call("edit_file", edit(&read_path, "content", "edited")).await?;
    for path in [unread_path, late_path] {
        let error = call_tool_error(&workspace.client, "edit_file", edit(&path, "content", "edited")).await;
        assert!(error.contains("before editing it"), "{}: {error}", path.display());
    }
    assert_eq!(workspace.read("read.txt")?, "edited");
    assert_eq!(workspace.read("unread.txt")?, "content");
    Ok(())
}

fn read(path: &Path) -> Value {
    json!({ "filePath": path })
}

fn edit(path: &Path, old: &str, new: &str) -> Value {
    json!({ "filePath": path, "edits": [{ "oldString": old, "newString": new }] })
}

fn write(path: &Path, content: &str) -> Value {
    json!({ "filePath": path, "content": content })
}
