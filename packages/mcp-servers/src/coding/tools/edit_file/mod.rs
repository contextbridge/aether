use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs::write;

use crate::coding::error::FileError;
use crate::coding::tools::file_io::read_text_file;
use crate::coding::tools::line_document::LineDocument;
use mcp_utils::display_meta::{FileDiff, ToolDisplayMeta, ToolResultMeta, basename};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditFileArgs {
    /// Path to the file to edit
    #[serde(alias = "file_path")]
    pub file_path: String,
    /// Line-numbered edits to validate and apply atomically
    pub edits: Vec<EditOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum EditOperation {
    SetLine {
        line: usize,
        #[serde(alias = "new_text")]
        new_text: String,
    },
    ReplaceLines {
        #[serde(alias = "start_line")]
        start_line: usize,
        #[serde(alias = "end_line")]
        end_line: usize,
        #[serde(alias = "new_text")]
        new_text: String,
    },
    DeleteLines {
        #[serde(alias = "start_line")]
        start_line: usize,
        #[serde(alias = "end_line")]
        end_line: usize,
    },
    InsertBefore {
        line: usize,
        text: String,
    },
    InsertAfter {
        line: usize,
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditFileResponse {
    pub status: String,
    pub message: String,
    /// Path of the file that was edited
    pub file_path: String,
    /// Total number of lines in the file after editing
    pub total_lines: usize,
    /// Number of requested edit operations applied
    pub edits_applied: usize,
    /// The new file content after editing (used internally for LSP sync)
    #[serde(skip_serializing)]
    pub content: String,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub async fn edit_file_contents(args: EditFileArgs) -> Result<EditFileResponse, FileError> {
    let current_content = read_text_file(&args.file_path).await?;
    let mut document = LineDocument::parse(&current_content);

    let mut resolved_edits = validate_edits(&args.file_path, document.lines(), &args.edits)?;
    reject_overlaps(&args.file_path, &resolved_edits)?;

    resolved_edits.sort_by(|left, right| {
        right
            .start
            .cmp(&left.start)
            .then_with(|| edit_apply_rank(left).cmp(&edit_apply_rank(right)))
            .then_with(|| right.edit_index.cmp(&left.edit_index))
    });

    for edit in resolved_edits {
        document.replace_range(edit.start, edit.end, edit.replacement_lines);
    }

    let total_lines = document.line_count();
    let updated_content = document.join();
    if updated_content == current_content {
        return Err(FileError::InvalidEdit {
            path: args.file_path,
            edit_index: None,
            reason: "edits produced no changes".to_string(),
        });
    }

    if let Err(error) = write(&args.file_path, &updated_content).await {
        return Err(FileError::WriteFailed { path: args.file_path, reason: error.to_string() });
    }

    let display_meta = ToolDisplayMeta::new("Edit file", basename(&args.file_path));
    let file_diff =
        FileDiff { path: args.file_path.clone(), old_text: Some(current_content), new_text: updated_content.clone() };

    Ok(EditFileResponse {
        status: "success".to_string(),
        message: "Edits applied successfully".to_string(),
        file_path: args.file_path,
        total_lines,
        edits_applied: args.edits.len(),
        content: updated_content,
        meta: Some(ToolResultMeta::with_file_diff(display_meta, file_diff)),
    })
}

#[derive(Debug, Clone)]
struct ResolvedEdit {
    edit_index: usize,
    start: usize,
    end: usize,
    replacement_lines: Vec<String>,
}

fn validate_edits(path: &str, lines: &[String], edits: &[EditOperation]) -> Result<Vec<ResolvedEdit>, FileError> {
    if edits.is_empty() {
        return Err(FileError::InvalidEdit {
            path: path.to_string(),
            edit_index: None,
            reason: "edits array must contain at least one edit".to_string(),
        });
    }

    edits.iter().enumerate().map(|(edit_index, edit)| edit.resolve(path, lines, edit_index)).collect()
}

impl EditOperation {
    fn resolve(&self, path: &str, lines: &[String], edit_index: usize) -> Result<ResolvedEdit, FileError> {
        match self {
            EditOperation::SetLine { line, new_text } => {
                let line = validate_line(path, lines, edit_index, *line)?;
                Ok(ResolvedEdit {
                    edit_index,
                    start: line - 1,
                    end: line,
                    replacement_lines: split_edit_text(new_text),
                })
            }
            EditOperation::ReplaceLines { start_line, end_line, new_text } => {
                let (start_line, end_line) = validate_line_range(path, lines, edit_index, *start_line, *end_line)?;
                Ok(ResolvedEdit {
                    edit_index,
                    start: start_line - 1,
                    end: end_line,
                    replacement_lines: split_edit_text(new_text),
                })
            }
            EditOperation::DeleteLines { start_line, end_line } => {
                let (start_line, end_line) = validate_line_range(path, lines, edit_index, *start_line, *end_line)?;
                Ok(ResolvedEdit { edit_index, start: start_line - 1, end: end_line, replacement_lines: Vec::new() })
            }
            EditOperation::InsertBefore { line, text } => {
                let line = validate_line(path, lines, edit_index, *line)?;
                Ok(ResolvedEdit {
                    edit_index,
                    start: line - 1,
                    end: line - 1,
                    replacement_lines: split_edit_text(text),
                })
            }
            EditOperation::InsertAfter { line, text } => {
                let line = validate_line(path, lines, edit_index, *line)?;
                Ok(ResolvedEdit { edit_index, start: line, end: line, replacement_lines: split_edit_text(text) })
            }
        }
    }
}

fn validate_line_range(
    path: &str,
    lines: &[String],
    edit_index: usize,
    start_line: usize,
    end_line: usize,
) -> Result<(usize, usize), FileError> {
    let start_line = validate_line(path, lines, edit_index, start_line)?;
    let end_line = validate_line(path, lines, edit_index, end_line)?;
    if start_line > end_line {
        return Err(FileError::InvalidEdit {
            path: path.to_string(),
            edit_index: Some(edit_index),
            reason: format!("range start line {start_line} must be before or equal to end line {end_line}"),
        });
    }

    Ok((start_line, end_line))
}

fn validate_line(path: &str, lines: &[String], edit_index: usize, line: usize) -> Result<usize, FileError> {
    if line == 0 {
        return Err(FileError::InvalidLine {
            path: path.to_string(),
            edit_index,
            line,
            reason: "line number must be 1 or greater".to_string(),
        });
    }

    if line > lines.len() {
        return Err(FileError::InvalidLine {
            path: path.to_string(),
            edit_index,
            line,
            reason: format!(
                "line {line} is out of range for file with {} lines; use write_file for new content in an empty file",
                lines.len()
            ),
        });
    }

    Ok(line)
}

fn reject_overlaps(path: &str, edits: &[ResolvedEdit]) -> Result<(), FileError> {
    for (left_index, left) in edits.iter().enumerate() {
        for right in edits.iter().skip(left_index + 1) {
            if left.start < left.end && right.start < right.end && ranges_overlap(left, right) {
                return Err(FileError::OverlappingEdits {
                    path: path.to_string(),
                    first_edit_index: left.edit_index,
                    second_edit_index: right.edit_index,
                    reason: "replacement ranges overlap".to_string(),
                });
            }

            if left.start == left.end && insertion_inside_replacement(left, right) {
                return Err(overlap_error(path, left, right));
            }

            if right.start == right.end && insertion_inside_replacement(right, left) {
                return Err(overlap_error(path, right, left));
            }
        }
    }

    Ok(())
}

fn ranges_overlap(left: &ResolvedEdit, right: &ResolvedEdit) -> bool {
    left.start.max(right.start) < left.end.min(right.end)
}

fn insertion_inside_replacement(insertion: &ResolvedEdit, replacement: &ResolvedEdit) -> bool {
    replacement.start < replacement.end && insertion.start > replacement.start && insertion.start < replacement.end
}

fn overlap_error(path: &str, insertion: &ResolvedEdit, replacement: &ResolvedEdit) -> FileError {
    FileError::OverlappingEdits {
        path: path.to_string(),
        first_edit_index: insertion.edit_index,
        second_edit_index: replacement.edit_index,
        reason: "insertion falls inside a replacement range".to_string(),
    }
}

fn edit_apply_rank(edit: &ResolvedEdit) -> usize {
    if edit.start < edit.end { 0 } else { 1 }
}

fn split_edit_text(text: &str) -> Vec<String> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return vec![String::new()];
    }

    LineDocument::parse(text).lines().to_vec()
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
            edits: vec![EditOperation::SetLine { line: 1, new_text: "after".to_string() }],
        })
        .await;

        assert!(matches!(result, Err(FileError::NotFound { .. })));
    }

    #[tokio::test]
    async fn set_line_replaces_one_line() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "one\ntwo\nthree\n").unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![EditOperation::SetLine { line: 2, new_text: "TWO".to_string() }],
        })
        .await
        .unwrap();

        assert_eq!(result.status, "success");
        assert_eq!(result.message, "Edits applied successfully");
        assert_eq!(result.edits_applied, 1);
        assert_eq!(result.total_lines, 3);
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "one\nTWO\nthree\n");
    }

    #[tokio::test]
    async fn set_line_with_null_deletes_one_line() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "one\ntwo\nthree").unwrap();

        edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![EditOperation::DeleteLines { start_line: 2, end_line: 2 }],
        })
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "one\nthree");
    }

    #[tokio::test]
    async fn set_line_with_empty_string_creates_blank_line() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "one\ntwo\nthree").unwrap();

        edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![EditOperation::SetLine { line: 2, new_text: String::new() }],
        })
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "one\n\nthree");
    }

    #[tokio::test]
    async fn replace_lines_replaces_inclusive_range() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "one\ntwo\nthree\nfour").unwrap();

        edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![EditOperation::ReplaceLines { start_line: 2, end_line: 3, new_text: "2\n3".to_string() }],
        })
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "one\n2\n3\nfour");
    }

    #[tokio::test]
    async fn replace_lines_with_null_deletes_range() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "one\ntwo\nthree\nfour").unwrap();

        edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![EditOperation::DeleteLines { start_line: 2, end_line: 3 }],
        })
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "one\nfour");
    }

    #[tokio::test]
    async fn insert_before_and_after_insert_multiline_text() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "one\ntwo").unwrap();

        edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![
                EditOperation::InsertBefore { line: 1, text: "zero\nhalf".to_string() },
                EditOperation::InsertAfter { line: 2, text: "three\nfour\n".to_string() },
            ],
        })
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "zero\nhalf\none\ntwo\nthree\nfour");
    }

    #[tokio::test]
    async fn multiple_edits_apply_bottom_up() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "one\ntwo\nthree\nfour\nfive").unwrap();

        edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![
                EditOperation::SetLine { line: 5, new_text: "FIVE".to_string() },
                EditOperation::SetLine { line: 1, new_text: "ONE".to_string() },
                EditOperation::InsertAfter { line: 3, text: "3.5".to_string() },
            ],
        })
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "ONE\ntwo\nthree\n3.5\nfour\nFIVE");
    }

    #[tokio::test]
    async fn same_index_insertions_preserve_request_order() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        fs::write(&file_path, "one\ntwo").unwrap();

        edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![
                EditOperation::InsertBefore { line: 2, text: "a".to_string() },
                EditOperation::InsertBefore { line: 2, text: "b".to_string() },
            ],
        })
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), "one\na\nb\ntwo");
    }

    #[tokio::test]
    async fn invalid_inputs_fail_without_modifying_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        let original = "one\ntwo\nthree";
        fs::write(&file_path, original).unwrap();

        let cases = vec![
            EditFileArgs { file_path: file_path.to_string_lossy().to_string(), edits: vec![] },
            EditFileArgs {
                file_path: file_path.to_string_lossy().to_string(),
                edits: vec![EditOperation::SetLine { line: 0, new_text: "x".to_string() }],
            },
            EditFileArgs {
                file_path: file_path.to_string_lossy().to_string(),
                edits: vec![EditOperation::SetLine { line: 4, new_text: "x".to_string() }],
            },
            EditFileArgs {
                file_path: file_path.to_string_lossy().to_string(),
                edits: vec![EditOperation::ReplaceLines { start_line: 3, end_line: 2, new_text: "x".to_string() }],
            },
            EditFileArgs {
                file_path: file_path.to_string_lossy().to_string(),
                edits: vec![EditOperation::SetLine { line: 2, new_text: "two".to_string() }],
            },
        ];

        for args in cases {
            assert!(edit_file_contents(args).await.is_err());
            assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
        }
    }

    #[tokio::test]
    async fn overlapping_edits_fail_without_modifying_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        let original = "one\ntwo\nthree\nfour";
        fs::write(&file_path, original).unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![
                EditOperation::ReplaceLines { start_line: 2, end_line: 3, new_text: "2-3".to_string() },
                EditOperation::SetLine { line: 3, new_text: "THREE".to_string() },
            ],
        })
        .await;

        assert!(matches!(result, Err(FileError::OverlappingEdits { first_edit_index: 0, second_edit_index: 1, .. })));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    }

    #[tokio::test]
    async fn insertion_inside_replacement_fails_without_modifying_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("sample.txt");
        let original = "one\ntwo\nthree\nfour";
        fs::write(&file_path, original).unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![
                EditOperation::ReplaceLines { start_line: 2, end_line: 4, new_text: "middle".to_string() },
                EditOperation::InsertAfter { line: 2, text: "inside".to_string() },
            ],
        })
        .await;

        assert!(matches!(result, Err(FileError::OverlappingEdits { first_edit_index: 1, second_edit_index: 0, .. })));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    }

    #[tokio::test]
    async fn line_endings_and_final_newline_are_preserved() {
        for (name, original, expected) in [
            ("lf", "one\ntwo\n", "one\nTWO\n"),
            ("crlf", "one\r\ntwo\r\n", "one\r\nTWO\r\n"),
            ("cr", "one\rtwo\r", "one\rTWO\r"),
            ("no_final_newline", "one\ntwo", "one\nTWO"),
        ] {
            let temp_dir = TempDir::new().unwrap();
            let file_path = temp_dir.path().join(format!("{name}.txt"));
            fs::write(&file_path, original).unwrap();

            edit_file_contents(EditFileArgs {
                file_path: file_path.to_string_lossy().to_string(),
                edits: vec![EditOperation::SetLine { line: 2, new_text: "TWO".to_string() }],
            })
            .await
            .unwrap();

            assert_eq!(fs::read_to_string(&file_path).unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn edit_file_file_diff_has_full_contents_and_correct_path() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("lines.txt");
        let original = "line1\nline2\nline3\nline4\n";
        fs::write(&file_path, original).unwrap();

        let result = edit_file_contents(EditFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            edits: vec![EditOperation::SetLine { line: 3, new_text: "replaced".to_string() }],
        })
        .await
        .unwrap();

        let serialized = serde_json::to_value(&result).unwrap();
        assert_eq!(serialized["status"], "success");
        assert_eq!(serialized["editsApplied"], 1);
        assert!(serialized.get("content").is_none());
        assert!(serialized.get("changedRegion").is_none());

        let diff = result.meta.unwrap().file_diff.unwrap();
        assert_eq!(diff.old_text.as_deref(), Some(original));
        assert!(diff.new_text.contains("replaced"));
        assert!(!diff.new_text.contains("line3"));
        assert_eq!(diff.path, file_path.to_string_lossy().to_string());
    }

    #[test]
    fn edit_file_args_accepts_snake_case_fields() {
        let args: EditFileArgs = serde_json::from_value(serde_json::json!({
            "file_path": "/tmp/test.txt",
            "edits": [
                {"type": "set_line", "line": 1, "new_text": "foo"},
                {"type": "delete_lines", "start_line": 2, "end_line": 3}
            ]
        }))
        .unwrap();

        assert_eq!(args.file_path, "/tmp/test.txt");
        assert_eq!(args.edits.len(), 2);
        match &args.edits[0] {
            EditOperation::SetLine { line, new_text } => {
                assert_eq!(*line, 1);
                assert_eq!(new_text, "foo");
            }
            other => panic!("unexpected edit: {other:?}"),
        }
    }
}
