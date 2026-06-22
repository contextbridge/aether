use std::io::ErrorKind;
use std::ops::Range;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs::{create_dir_all, write};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileEdit {
    /// Exact text to find. Must match whitespace (e.g. indentation).
    #[serde(alias = "old_string")]
    pub old_string: String,
    /// Text to substitute for `oldString`.
    #[serde(alias = "new_string")]
    pub new_string: String,
    /// Replace every occurrence instead of just the first.
    #[serde(default, alias = "replace_all")]
    pub replace_all: bool,
}

impl FileEdit {
    pub fn new(old_string: &str, new_string: &str) -> Self {
        Self { old_string: old_string.to_string(), new_string: new_string.to_string(), replace_all: false }
    }
}

#[derive(Debug, Error)]
pub enum FileError {
    #[error("File does not exist: {path}")]
    NotFound { path: String },

    #[error("Failed to read file {path}: {reason}")]
    ReadFailed { path: String, reason: String },

    #[error("Failed to write to file {path}: {reason}")]
    WriteFailed { path: String, reason: String },

    #[error("Failed to create directories for {path}: {reason}")]
    CreateDirFailed { path: String, reason: String },

    #[error("Invalid offset for file {path}: offset must be 1-indexed (start from 1)")]
    InvalidOffset { path: String },

    #[error("No edits supplied for file {path}: provide at least one edit")]
    EmptyEdits { path: String },

    #[error("{} edit(s) failed for file {path}; no changes were written:\n{}", .failures.len(), format_failures(.failures))]
    EditsFailed { path: String, failures: Vec<EditFailure> },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A single edit that could not be applied. `index` is the
/// position of the failing edit in the supplied `edits` array,
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFailure {
    pub index: usize,
    pub kind: EditFailureKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EditFailureKind {
    #[error("string not found: `{pattern}`")]
    PatternNotFound { pattern: String },

    #[error("edit overlaps the edit at index {other}; use non-overlapping edits or combine them")]
    Overlapping { other: usize },
}

pub struct WriteTextFileResult {
    pub path: PathBuf,
    pub bytes_written: usize,
}

#[derive(Debug)]
pub struct ApplyEditsResult {
    pub path: PathBuf,
    pub original_content: String,
    pub updated_content: String,
    pub replacements_made: usize,
}

pub async fn read_text_file(path: &Path) -> Result<String, FileError> {
    tokio::fs::read_to_string(path).await.map_err(|error| match error.kind() {
        ErrorKind::NotFound => FileError::NotFound { path: display_path(path) },
        _ => FileError::ReadFailed { path: display_path(path), reason: error.to_string() },
    })
}

pub async fn write_text_file(path: &Path, content: &str) -> Result<WriteTextFileResult, FileError> {
    if let Some(parent) = path.parent()
        && let Err(error) = create_dir_all(parent).await
    {
        return Err(FileError::CreateDirFailed { path: display_path(path), reason: error.to_string() });
    }

    if let Err(error) = write(path, content).await {
        return Err(FileError::WriteFailed { path: display_path(path), reason: error.to_string() });
    }

    Ok(WriteTextFileResult { path: path.to_path_buf(), bytes_written: content.len() })
}

pub async fn apply_edits(path: &Path, edits: &[FileEdit]) -> Result<ApplyEditsResult, FileError> {
    if edits.is_empty() {
        return Err(FileError::EmptyEdits { path: display_path(path) });
    }

    let original_content = read_text_file(path).await?;
    let resolved = resolve_edits(&original_content, edits)
        .map_err(|failures| FileError::EditsFailed { path: display_path(path), failures })?;

    let updated_content = splice(&original_content, &resolved);
    let replacements_made = resolved.len();

    if let Err(error) = write(path, &updated_content).await {
        return Err(FileError::WriteFailed { path: display_path(path), reason: error.to_string() });
    }

    Ok(ApplyEditsResult { path: path.to_path_buf(), original_content, updated_content, replacements_made })
}

struct ResolvedEdit {
    range: Range<usize>,
    replacement: String,
    edit_index: usize,
}

fn resolve_edits(content: &str, edits: &[FileEdit]) -> Result<Vec<ResolvedEdit>, Vec<EditFailure>> {
    let mut resolved: Vec<ResolvedEdit> = Vec::new();
    let mut failures: Vec<EditFailure> = Vec::new();

    for (index, FileEdit { old_string, new_string, replace_all }) in edits.iter().enumerate() {
        let starts: Vec<usize> = content.match_indices(old_string).map(|(start, _)| start).collect();
        if starts.is_empty() {
            failures
                .push(EditFailure { index, kind: EditFailureKind::PatternNotFound { pattern: old_string.clone() } });
            continue;
        }
        let chosen = if *replace_all { starts.as_slice() } else { &starts[..1] };
        for &start in chosen {
            resolved.push(ResolvedEdit {
                range: start..start + old_string.len(),
                replacement: new_string.clone(),
                edit_index: index,
            });
        }
    }

    resolved.sort_by_key(|edit| edit.range.start);
    let mut furthest: Option<(usize, usize)> = None;
    for edit in &resolved {
        match furthest {
            Some((end, other)) if edit.range.start < end => {
                failures.push(EditFailure { index: edit.edit_index, kind: EditFailureKind::Overlapping { other } });
                if edit.range.end > end {
                    furthest = Some((edit.range.end, edit.edit_index));
                }
            }
            _ => furthest = Some((edit.range.end, edit.edit_index)),
        }
    }

    if failures.is_empty() {
        Ok(resolved)
    } else {
        failures.sort_by_key(|failure| failure.index);
        Err(failures)
    }
}

fn splice(content: &str, resolved: &[ResolvedEdit]) -> String {
    let mut result = String::with_capacity(content.len());
    let mut last = 0;
    for edit in resolved {
        result.push_str(&content[last..edit.range.start]);
        result.push_str(&edit.replacement);
        last = edit.range.end;
    }
    result.push_str(&content[last..]);
    result
}

fn format_failures(failures: &[EditFailure]) -> String {
    failures
        .iter()
        .map(|failure| format!("  edits[{}]: {}", failure.index, failure.kind))
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn applies_batch_against_original_content() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("alpha\nbeta\ngamma\n").await?;
        let result = apply_edits(&path, &[FileEdit::new("alpha", "ALPHA"), FileEdit::new("gamma", "GAMMA")]).await?;

        assert_eq!(result.replacements_made, 2);
        assert_eq!(std::fs::read_to_string(&path)?, "ALPHA\nbeta\nGAMMA\n");
        Ok(())
    }

    #[tokio::test]
    async fn applies_multiple_replacements_without_drift() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("one\ntwo\nthree\nfour\n").await?;
        apply_edits(&path, &[FileEdit::new("one", "ONE"), FileEdit::new("three", "THREE")]).await?;
        assert_eq!(std::fs::read_to_string(&path)?, "ONE\ntwo\nTHREE\nfour\n");
        Ok(())
    }

    #[tokio::test]
    async fn replace_all_counts_every_occurrence() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("x x x\n").await?;
        let result = apply_edits(
            &path,
            &[FileEdit { old_string: "x".to_string(), new_string: "y".to_string(), replace_all: true }],
        )
        .await?;

        assert_eq!(result.replacements_made, 3);
        assert_eq!(std::fs::read_to_string(&path)?, "y y y\n");
        Ok(())
    }

    #[tokio::test]
    async fn empty_new_string_deletes_the_match() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("a\nb\nc\n").await?;
        apply_edits(&path, &[FileEdit::new("b\n", "")]).await?;
        assert_eq!(std::fs::read_to_string(&path)?, "a\nc\n");
        Ok(())
    }

    #[tokio::test]
    async fn overlapping_edits_are_rejected_and_file_unchanged() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("foobar\n").await?;

        let error = apply_edits(&path, &[FileEdit::new("foob", "X"), FileEdit::new("obar", "Y")])
            .await
            .expect_err("overlapping edits should fail");

        let failures = edits_failures(error)?;
        assert!(failures.iter().any(|f| matches!(f.kind, EditFailureKind::Overlapping { .. })));
        assert_eq!(std::fs::read_to_string(&path)?, "foobar\n");
        Ok(())
    }

    #[tokio::test]
    async fn every_overlapping_edit_is_reported() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("abcdef\n").await?;
        let error =
            apply_edits(&path, &[FileEdit::new("abcde", "X"), FileEdit::new("bc", "Y"), FileEdit::new("de", "Z")])
                .await
                .expect_err("overlapping edits should fail");

        let failures = edits_failures(error)?;
        let overlapping: Vec<usize> = failures
            .iter()
            .filter(|f| matches!(f.kind, EditFailureKind::Overlapping { other: 0 }))
            .map(|f| f.index)
            .collect();
        assert_eq!(overlapping, vec![1, 2]);
        Ok(())
    }

    #[tokio::test]
    async fn pattern_not_found_is_rejected_and_file_unchanged() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("hello\n").await?;
        let error =
            apply_edits(&path, &[FileEdit::new("missing", "x")]).await.expect_err("missing pattern should fail");

        let failures = edits_failures(error)?;
        assert_eq!(
            failures,
            vec![EditFailure { index: 0, kind: EditFailureKind::PatternNotFound { pattern: "missing".to_string() } }]
        );
        assert_eq!(std::fs::read_to_string(&path)?, "hello\n");
        Ok(())
    }

    #[tokio::test]
    async fn batch_failures_aggregate_and_write_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("hello\n").await?;
        let error = apply_edits(&path, &[FileEdit::new("missing", "z"), FileEdit::new("absent", "q")])
            .await
            .expect_err("batch with missing patterns should fail");

        let failures = edits_failures(error)?;
        assert_eq!(failures.len(), 2);
        assert!(failures.iter().all(|f| matches!(f.kind, EditFailureKind::PatternNotFound { .. })));
        assert_eq!(std::fs::read_to_string(&path)?, "hello\n");
        Ok(())
    }

    #[tokio::test]
    async fn empty_edits_are_rejected() {
        let (_dir, path) = with_file("hello\n").await.unwrap();
        assert!(matches!(apply_edits(&path, &[]).await, Err(FileError::EmptyEdits { .. })));
    }

    async fn with_file(content: &str) -> Result<(TempDir, PathBuf), Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        let path = dir.path().join("file.txt");
        write_text_file(&path, content).await?;
        Ok((dir, path))
    }

    fn edits_failures(error: FileError) -> Result<Vec<EditFailure>, FileError> {
        match error {
            FileError::EditsFailed { failures, .. } => Ok(failures),
            other => Err(other),
        }
    }
}
