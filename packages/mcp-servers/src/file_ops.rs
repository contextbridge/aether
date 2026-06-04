use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::fs::{create_dir_all, write};

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

    #[error("String replacement failed for file {path}: string '{pattern}' not found")]
    PatternNotFound { path: String, pattern: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct WriteTextFileResult {
    pub path: PathBuf,
    pub bytes_written: usize,
}

pub struct EditTextFileResult {
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

pub async fn edit_text_file(
    path: &Path,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<EditTextFileResult, FileError> {
    let original_content = read_text_file(path).await?;
    let (updated_content, replacements_made) = if replace_all {
        let count = original_content.matches(old_string).count();
        (original_content.replace(old_string, new_string), count)
    } else if original_content.contains(old_string) {
        (original_content.replacen(old_string, new_string, 1), 1)
    } else {
        (original_content.clone(), 0)
    };

    if replacements_made == 0 {
        return Err(FileError::PatternNotFound { path: display_path(path), pattern: old_string.to_string() });
    }

    if let Err(error) = write(path, &updated_content).await {
        return Err(FileError::WriteFailed { path: display_path(path), reason: error.to_string() });
    }

    Ok(EditTextFileResult { path: path.to_path_buf(), original_content, updated_content, replacements_made })
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
