use crate::file_ops::{FileError, write_text_file};
use mcp_utils::display_meta::{FileDiff, ToolDisplayMeta, ToolResultMeta, basename};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WriteFileArgs {
    /// The absolute path to the file to write
    #[serde(alias = "file_path")]
    pub file_path: String,
    /// The content to write to the file
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WriteFileResponse {
    /// Success message
    pub message: String,
    /// Number of bytes written
    pub bytes_written: usize,
    /// File path that was written
    pub file_path: String,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub async fn write_file_contents(args: WriteFileArgs) -> Result<WriteFileResponse, FileError> {
    let file_path = Path::new(&args.file_path);
    let result = write_text_file(file_path, &args.content).await?;
    let file_path = result.path.to_string_lossy().into_owned();
    let display_meta = ToolDisplayMeta::new("Write file", basename(&file_path));
    let file_diff = FileDiff { path: file_path.clone(), old_text: None, new_text: args.content };

    Ok(WriteFileResponse {
        message: format!("Successfully wrote {} bytes to {}", result.bytes_written, file_path),
        bytes_written: result.bytes_written,
        file_path,
        meta: Some(ToolResultMeta::with_file_diff(display_meta, file_diff)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn write_file_produces_file_diff_with_no_old_text() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("new_file.rs");

        let content = "fn main() {\n    println!(\"Hello\");\n}\n";
        let result = write_file_contents(WriteFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            content: content.to_string(),
        })
        .await
        .unwrap();

        assert!(file_path.exists());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), content);

        let meta = result.meta.unwrap();
        let diff = meta.file_diff.unwrap();
        assert!(diff.old_text.is_none());
        assert_eq!(diff.new_text, content);
        assert_eq!(diff.path, file_path.to_string_lossy().to_string());
    }

    #[tokio::test]
    async fn write_file_file_diff_has_correct_path() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.rs");

        let result = write_file_contents(WriteFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            content: "let x = 1;".to_string(),
        })
        .await
        .unwrap();

        let diff = result.meta.unwrap().file_diff.unwrap();
        assert_eq!(diff.path, file_path.to_string_lossy().to_string());
    }

    #[tokio::test]
    async fn write_file_handles_empty_content() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("empty.txt");

        let result = write_file_contents(WriteFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            content: String::new(),
        })
        .await
        .unwrap();

        let diff = result.meta.unwrap().file_diff.unwrap();
        assert_eq!(diff.new_text, "");
        assert!(diff.old_text.is_none());
    }

    #[tokio::test]
    async fn write_file_overwrites_existing_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("existing.txt");
        fs::write(&file_path, "old content").unwrap();

        let new_content = "new content\nsecond line\n";
        let result = write_file_contents(WriteFileArgs {
            file_path: file_path.to_string_lossy().to_string(),
            content: new_content.to_string(),
        })
        .await
        .unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), new_content);

        let diff = result.meta.unwrap().file_diff.unwrap();
        assert!(diff.old_text.is_none());
        assert_eq!(diff.new_text, new_content);
    }

    #[test]
    fn write_file_args_accepts_snake_case_file_path() {
        let args: WriteFileArgs = serde_json::from_value(serde_json::json!({
            "file_path": "/tmp/out.txt",
            "content": "hello"
        }))
        .unwrap();

        assert_eq!(args.file_path, "/tmp/out.txt");
        assert_eq!(args.content, "hello");
    }
}
