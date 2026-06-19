use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use crate::coding::error::ListFilesError;
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListFilesArgs {
    /// Directory path to list (defaults to current directory if not provided)
    pub path: Option<String>,
    /// Include hidden files (starting with .)
    #[serde(alias = "include_hidden")]
    pub include_hidden: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum FileType {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub file_type: FileType,
    pub size: u64,
    pub permissions: String,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListFilesResult {
    pub status: String,
    pub directory: String,
    pub files: Vec<FileInfo>,
    pub total_count: usize,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub async fn list_files(args: ListFilesArgs) -> Result<ListFilesResult, ListFilesError> {
    use std::os::unix::fs::PermissionsExt;

    let target_path = args.path.as_deref().filter(|p| !p.is_empty()).unwrap_or(".");
    let include_hidden = args.include_hidden.unwrap_or(false);

    let mut files = Vec::new();

    // Read directory entries
    let mut entries =
        tokio::fs::read_dir(target_path).await.map_err(|e| ListFilesError::ReadDirFailed(e.to_string()))?;

    while let Some(entry) = entries.next_entry().await.map_err(|e| ListFilesError::ReadEntryFailed(e.to_string()))? {
        let path = entry.path();
        let entry_file_type = entry.file_type().await.map_err(|e| ListFilesError::MetadataFailed(e.to_string()))?;
        let metadata = entry.metadata().await.map_err(|e| ListFilesError::MetadataFailed(e.to_string()))?;

        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files unless requested
        if !include_hidden && name.starts_with('.') {
            continue;
        }

        let file_type = if entry_file_type.is_symlink() {
            FileType::Symlink
        } else if metadata.is_dir() {
            FileType::Directory
        } else {
            FileType::File
        };

        let permissions = format!("{:o}", metadata.permissions().mode() & 0o777);

        let modified =
            metadata.modified().ok().and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok()).and_then(
                |duration| {
                    let secs = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
                    chrono::DateTime::from_timestamp(secs, 0).map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                },
            );

        files.push(FileInfo {
            name,
            path: path.to_string_lossy().to_string(),
            file_type,
            size: metadata.len(),
            permissions,
            modified,
        });
    }

    // Sort by name for consistent output
    files.sort_by(|a, b| a.name.cmp(&b.name));

    let total_count = files.len();

    let display_meta = ToolDisplayMeta::new("List files", format!("{} ({total_count} items)", basename(target_path)));

    Ok(ListFilesResult {
        status: "success".to_string(),
        directory: target_path.to_string(),
        files,
        total_count,
        meta: Some(display_meta.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[tokio::test]
    async fn list_files_reports_symlink_for_directory_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let sub_dir = temp_dir.path().join("real_dir");
        fs::create_dir(&sub_dir).unwrap();
        let link_path = temp_dir.path().join("link_dir");
        symlink(&sub_dir, &link_path).unwrap();

        let result = list_files(ListFilesArgs {
            path: Some(temp_dir.path().to_string_lossy().to_string()),
            include_hidden: Some(true),
        })
        .await
        .unwrap();

        let file_info =
            result.files.iter().find(|file| file.name == "link_dir").expect("directory symlink should be returned");
        assert!(matches!(file_info.file_type, FileType::Symlink));
    }

    #[tokio::test]
    async fn list_files_reports_symlink_type() {
        let temp_dir = TempDir::new().unwrap();
        let target_path = temp_dir.path().join("target.txt");
        let link_path = temp_dir.path().join("link.txt");
        fs::write(&target_path, "hello").unwrap();
        symlink(&target_path, &link_path).unwrap();

        let result = list_files(ListFilesArgs {
            path: Some(temp_dir.path().to_string_lossy().to_string()),
            include_hidden: Some(true),
        })
        .await
        .unwrap();

        let file_info = result.files.iter().find(|file| file.name == "link.txt").expect("symlink should be returned");
        assert!(matches!(file_info.file_type, FileType::Symlink));
    }

    #[test]
    fn list_files_args_accepts_snake_case_include_hidden() {
        let args: ListFilesArgs = serde_json::from_value(serde_json::json!({
            "path": "/tmp",
            "include_hidden": true
        }))
        .unwrap();

        assert_eq!(args.path, Some("/tmp".to_string()));
        assert_eq!(args.include_hidden, Some(true));
    }

    #[tokio::test]
    async fn list_files_handles_empty_path_as_current_directory() {
        let result = list_files(ListFilesArgs { path: Some(String::new()), include_hidden: None }).await.unwrap();
        assert_eq!(result.directory, ".");
    }
}
