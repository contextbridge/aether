use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::file_ops::{ApplyEditsResult, FileEdit, FileError, apply_edits};
use mcp_utils::display_meta::{FileDiff, ToolDisplayMeta, ToolResultMeta, basename};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditFileArgs {
    /// Path to the file to edit
    #[serde(alias = "file_path")]
    pub file_path: String,
    pub edits: Vec<FileEdit>,
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
    let ApplyEditsResult { path, original_content, updated_content, replacements_made } =
        apply_edits(Path::new(&args.file_path), &args.edits).await?;
    let file_path = path.to_string_lossy().into_owned();
    let total_lines = updated_content.lines().count();
    let display_meta = ToolDisplayMeta::new("Edit file", basename(&file_path));
    let file_diff =
        FileDiff { path: file_path.clone(), old_text: Some(original_content), new_text: updated_content.clone() };

    Ok(EditFileResponse {
        status: "success".to_string(),
        file_path,
        total_lines,
        replacements_made,
        content: updated_content,
        meta: Some(ToolResultMeta::with_file_diff(display_meta, file_diff)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_ops::EditFailureKind;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn edit_file_nonexistent_returns_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("missing.txt");

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![FileEdit::new("before", "after")],
        })
        .await;

        assert!(matches!(result, Err(FileError::NotFound { .. })));
    }

    #[tokio::test]
    async fn edit_file_existing_file_without_match_returns_edits_failed() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "hello world")?;

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![FileEdit::new("missing", "replacement")],
        })
        .await;

        let Err(FileError::EditsFailed { failures, .. }) = result else {
            return Err("expected EditsFailed".into());
        };
        assert!(matches!(failures.as_slice(), [f] if matches!(f.kind, EditFailureKind::PatternNotFound { .. })));
        Ok(())
    }

    #[tokio::test]
    async fn edit_file_produces_file_diff_with_full_contents() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("lines.txt");
        let original = "line1\nline2\nline3\nline4\n";
        fs::write(&file_path, original).unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![FileEdit::new("line3", "replaced")],
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
    async fn edit_file_applies_batch_in_one_call() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("lines.txt");
        fs::write(&file_path, "alpha\nbeta\ngamma\n").unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![FileEdit::new("alpha", "ALPHA"), FileEdit::new("gamma", "GAMMA")],
        })
        .await
        .unwrap();

        assert_eq!(result.replacements_made, 2);
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "ALPHA\nbeta\nGAMMA\n");
    }

    #[test]
    fn edit_file_args_accepts_snake_case_fields() {
        let args: EditFileArgs = serde_json::from_value(serde_json::json!({
            "file_path": "/tmp/test.txt",
            "edits": [{ "old_string": "foo", "new_string": "bar", "replace_all": true }]
        }))
        .unwrap();

        assert_eq!(args.file_path, "/tmp/test.txt");
        assert!(matches!(
            args.edits.as_slice(),
            [FileEdit { old_string, new_string, replace_all: true }]
                if old_string == "foo" && new_string == "bar"
        ));
    }
}
