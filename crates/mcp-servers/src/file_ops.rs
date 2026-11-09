use std::io::ErrorKind;
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

    #[error("These edits failed and must be retried; all other edits were applied:\n{}", format_failures(.failures))]
    EditsFailed { path: String, failures: Vec<EditFailure> },
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
    #[error("oldString not found: `{pattern}`")]
    PatternNotFound { pattern: String },
}

pub struct WriteTextFileResult {
    pub path: PathBuf,
    pub bytes_written: usize,
    pub original_content: Option<String>,
}

#[derive(Debug)]
pub struct ApplyEditsResult {
    pub path: PathBuf,
    pub original_content: String,
    pub updated_content: String,
}

pub async fn read_text_file(path: &Path) -> Result<String, FileError> {
    tokio::fs::read_to_string(path).await.map_err(|error| match error.kind() {
        ErrorKind::NotFound => FileError::NotFound { path: display_path(path) },
        _ => FileError::ReadFailed { path: display_path(path), reason: error.to_string() },
    })
}

pub async fn write_text_file(path: &Path, content: &str) -> Result<WriteTextFileResult, FileError> {
    let original_content = read_text_file(path).await.ok();
    if let Some(parent) = path.parent()
        && let Err(error) = create_dir_all(parent).await
    {
        return Err(FileError::CreateDirFailed { path: display_path(path), reason: error.to_string() });
    }

    if let Err(error) = write(path, content).await {
        return Err(FileError::WriteFailed { path: display_path(path), reason: error.to_string() });
    }

    Ok(WriteTextFileResult { path: path.to_path_buf(), bytes_written: content.len(), original_content })
}

pub async fn apply_edits(path: &Path, edits: &[FileEdit]) -> Result<ApplyEditsResult, FileError> {
    if edits.is_empty() {
        return Err(FileError::EmptyEdits { path: display_path(path) });
    }

    let original_content = read_text_file(path).await?;
    let mut updated_content = original_content.clone();
    let mut failures = Vec::new();

    for (index, edit) in edits.iter().enumerate() {
        match apply_edit(&updated_content, edit) {
            Ok(content) => updated_content = content,
            Err(kind) => failures.push(EditFailure { index, kind }),
        }
    }

    if failures.len() < edits.len()
        && let Err(error) = write(path, &updated_content).await
    {
        return Err(FileError::WriteFailed { path: display_path(path), reason: error.to_string() });
    }

    if !failures.is_empty() {
        return Err(FileError::EditsFailed { path: display_path(path), failures });
    }

    Ok(ApplyEditsResult { path: path.to_path_buf(), original_content, updated_content })
}

fn apply_edit(content: &str, edit: &FileEdit) -> Result<String, EditFailureKind> {
    let FileEdit { old_string, new_string, replace_all } = edit;
    if !content.contains(old_string.as_str()) {
        return Err(EditFailureKind::PatternNotFound { pattern: old_string.clone() });
    }
    Ok(if *replace_all {
        content.replace(old_string.as_str(), new_string)
    } else {
        content.replacen(old_string.as_str(), new_string, 1)
    })
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
    async fn applies_independent_edits_in_one_batch() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("alpha\nbeta\ngamma\n").await?;
        apply_edits(&path, &[FileEdit::new("alpha", "ALPHA"), FileEdit::new("gamma", "GAMMA")]).await?;

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
    async fn replace_all_replaces_every_occurrence() -> Result<(), Box<dyn std::error::Error>> {
        let (_dir, path) = with_file("x x x\n").await?;
        apply_edits(&path, &[FileEdit { old_string: "x".to_string(), new_string: "y".to_string(), replace_all: true }])
            .await?;

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
