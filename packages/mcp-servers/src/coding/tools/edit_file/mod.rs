use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::file_ops::{FileError, edit_text_file};
use mcp_utils::display_meta::{FileDiff, ToolDisplayMeta, ToolResultMeta, basename};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditFileArgs {
    /// Path to the file to edit
    #[serde(alias = "file_path")]
    pub file_path: String,
    /// Exact string to find and replace in the file
    #[serde(alias = "old_string")]
    pub old_string: String,
    /// String to replace it with
    #[serde(alias = "new_string")]
    pub new_string: String,
    /// Replace all occurrences (default: false - replace only first match)
    #[serde(default, alias = "replace_all")]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditFileResponse {
    pub status: String,
    /// Path of the file that was edited
    pub file_path: String,
    /// Total number of lines in the file after editing
    pub total_lines: usize,
    /// Number of replacements made
    pub replacements_made: usize,
    /// The new file content after editing (used internally for LSP sync)
    #[serde(skip_serializing)]
    pub content: String,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub async fn edit_file_contents(args: EditFileArgs) -> Result<EditFileResponse, FileError> {
    let result =
        edit_text_file(Path::new(&args.file_path), &args.old_string, &args.new_string, args.replace_all).await?;
    let file_path = result.path.to_string_lossy().into_owned();
    let total_lines = result.updated_content.lines().count();
    let display_meta = ToolDisplayMeta::new("Edit file", basename(&file_path));
    let file_diff = FileDiff {
        path: file_path.clone(),
        old_text: Some(result.original_content),
        new_text: result.updated_content.clone(),
    };

    Ok(EditFileResponse {
        status: "success".to_string(),
        file_path,
        total_lines,
        replacements_made: result.replacements_made,
        content: result.updated_content,
        meta: Some(ToolResultMeta::with_file_diff(display_meta, file_diff)),
    })
}

#[allow(clippy::used_underscore_binding)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn edit_file_nonexistent_returns_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("missing.txt");

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            old_string: "before".to_string(),
            new_string: "after".to_string(),
            replace_all: false,
        })
        .await;

        assert!(matches!(result, Err(FileError::NotFound { .. })));
    }

    #[tokio::test]
    async fn edit_file_existing_file_without_match_returns_pattern_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "hello world").unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            old_string: "missing".to_string(),
            new_string: "replacement".to_string(),
            replace_all: false,
        })
        .await;

        assert!(matches!(result, Err(FileError::PatternNotFound { .. })));
    }

    #[tokio::test]
    async fn edit_file_produces_file_diff_with_full_contents() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("lines.txt");
        let original = "line1\nline2\nline3\nline4\n";
        fs::write(&file_path, original).unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            old_string: "line3".to_string(),
            new_string: "replaced".to_string(),
            replace_all: false,
        })
        .await
        .unwrap();

        let meta = result.meta.unwrap();
        let diff = meta.file_diff.unwrap();
        assert_eq!(diff.old_text.as_deref(), Some(original));
        assert!(diff.new_text.contains("replaced"));
        assert!(!diff.new_text.contains("line3"));
        assert_eq!(diff.path, file_path.to_string_lossy().to_string());
    }

    #[tokio::test]
    async fn edit_file_file_diff_has_correct_path() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.rs");
        fs::write(&file_path, "hello world").unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            old_string: "hello".to_string(),
            new_string: "goodbye".to_string(),
            replace_all: false,
        })
        .await
        .unwrap();

        let diff = result.meta.unwrap().file_diff.unwrap();
        assert_eq!(diff.path, file_path.to_string_lossy().to_string());
    }

    #[test]
    fn edit_file_args_accepts_snake_case_fields() {
        let args: EditFileArgs = serde_json::from_value(serde_json::json!({
            "file_path": "/tmp/test.txt",
            "old_string": "foo",
            "new_string": "bar",
            "replace_all": true
        }))
        .unwrap();

        assert_eq!(args.file_path, "/tmp/test.txt");
        assert_eq!(args.old_string, "foo");
        assert_eq!(args.new_string, "bar");
        assert!(args.replace_all);
    }
}
