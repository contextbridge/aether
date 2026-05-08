use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::coding::error::FileError;
use crate::coding::tools::file_io::read_text_file;
use crate::coding::tools::line_document::LineDocument;
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};

const MAX_LINE_LENGTH: usize = 2000;
const DEFAULT_LINE_LIMIT: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileArgs {
    /// Path to the file to read (must be an existing file)
    #[serde(alias = "file_path")]
    pub file_path: String,
    /// Starting line number to read from (1-indexed). If not specified, starts from line 1.
    pub offset: Option<usize>,
    /// Maximum number of lines to read. If not specified, reads up to 2000 lines.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileResult {
    pub status: String,
    pub file_path: String,
    pub content: String,
    pub total_lines: usize,
    pub lines_shown: usize,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    pub size: usize,
    /// Raw file content without line numbers (used internally for LSP sync)
    #[serde(skip_serializing)]
    pub raw_content: String,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub async fn read_file_contents(args: ReadFileArgs) -> Result<ReadFileResult, FileError> {
    let content = read_text_file(&args.file_path).await?;

    let document = LineDocument::parse(&content);
    let total_lines = document.line_count();

    let offset = args.offset.unwrap_or(1);

    if offset == 0 {
        return Err(FileError::InvalidOffset { path: args.file_path });
    }

    let start_idx = (offset - 1).min(total_lines);
    let limit = args.limit.unwrap_or(DEFAULT_LINE_LIMIT);
    let end_idx = (start_idx + limit).min(total_lines);
    let selected_lines = &document.lines()[start_idx..end_idx];
    let lines_with_numbers: Vec<String> =
        selected_lines.iter().enumerate().map(|(index, line)| format_numbered_line(offset + index, line)).collect();

    let formatted_content = lines_with_numbers.join("\n");

    let display_meta = ToolDisplayMeta::new("Read file", format!("{}, {total_lines} lines", basename(&args.file_path)));

    Ok(ReadFileResult {
        status: "success".to_string(),
        file_path: args.file_path,
        content: formatted_content,
        total_lines,
        lines_shown: selected_lines.len(),
        offset,
        limit: Some(limit),
        size: content.len(),
        raw_content: content,
        meta: Some(display_meta.into()),
    })
}

fn format_numbered_line(line_number: usize, line: &str) -> String {
    let char_count = line.chars().count();
    if char_count > MAX_LINE_LENGTH {
        let truncated: String = line.chars().take(MAX_LINE_LENGTH).collect();
        format!("{line_number:5}\t{truncated}... [truncated, {char_count} chars total]")
    } else {
        format!("{line_number:5}\t{line}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_file_with_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("test_read_defaults.txt");
        let test_content = "line 1\nline 2\nline 3";

        fs::write(&test_path, test_content).unwrap();

        let result = read_file_contents(ReadFileArgs {
            file_path: test_path.to_string_lossy().to_string(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();

        assert_eq!(result.status, "success");
        assert_eq!(result.total_lines, 3);
        assert_eq!(result.lines_shown, 3);
        assert_eq!(result.offset, 1);
        assert_eq!(result.limit, Some(DEFAULT_LINE_LIMIT));
        assert!(result.content.contains("    1\tline 1"));
        assert!(result.content.contains("    2\tline 2"));
        assert!(result.content.contains("    3\tline 3"));
    }

    #[tokio::test]
    async fn test_read_file_with_offset_and_limit() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("test_offset_limit.txt");
        let test_content = "line 1\nline 2\nline 3\nline 4\nline 5";

        fs::write(&test_path, test_content).unwrap();

        let result = read_file_contents(ReadFileArgs {
            file_path: test_path.to_string_lossy().to_string(),
            offset: Some(2),
            limit: Some(2),
        })
        .await
        .unwrap();

        assert_eq!(result.total_lines, 5);
        assert_eq!(result.lines_shown, 2);
        assert_eq!(result.offset, 2);
        assert_eq!(result.limit, Some(2));
        assert_eq!(result.content, "    2\tline 2\n    3\tline 3");
    }

    #[tokio::test]
    async fn test_read_file_line_truncation() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("test_truncation.txt");
        let short_line = "short";
        let long_line = "é".repeat(2500);
        let test_content = format!("{short_line}\n{long_line}");

        fs::write(&test_path, &test_content).unwrap();

        let result = read_file_contents(ReadFileArgs {
            file_path: test_path.to_string_lossy().to_string(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.content.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("short"));
        assert!(!lines[0].contains("truncated"));
        assert!(lines[1].starts_with("    2\t"));
        assert!(lines[1].contains("truncated"));
        assert!(lines[1].contains("2500 chars total"));
    }

    #[tokio::test]
    async fn test_read_file_default_limit() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("test_default_limit.txt");
        let mut lines = Vec::new();
        for i in 1..=2500 {
            lines.push(format!("Line {i}"));
        }
        let test_content = lines.join("\n");

        fs::write(&test_path, &test_content).unwrap();

        let result = read_file_contents(ReadFileArgs {
            file_path: test_path.to_string_lossy().to_string(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();

        assert_eq!(result.total_lines, 2500);
        assert_eq!(result.lines_shown, DEFAULT_LINE_LIMIT);
        assert_eq!(result.limit, Some(DEFAULT_LINE_LIMIT));
        assert!(result.content.contains("    1\tLine 1"));
        assert!(result.content.contains(" 2000\tLine 2000"));
        assert!(!result.content.contains("Line 2001"));
    }

    #[tokio::test]
    async fn test_read_file_invalid_offset() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("test_invalid_offset.txt");
        let test_content = "line 1";

        fs::write(&test_path, test_content).unwrap();

        let result = read_file_contents(ReadFileArgs {
            file_path: test_path.to_string_lossy().to_string(),
            offset: Some(0),
            limit: None,
        })
        .await;

        assert!(matches!(result, Err(FileError::InvalidOffset { .. })));
    }

    #[tokio::test]
    async fn test_read_file_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let nonexistent_path = temp_dir.path().join("nonexistent_file_xyz123.txt");

        let result = read_file_contents(ReadFileArgs {
            file_path: nonexistent_path.to_string_lossy().to_string(),
            offset: None,
            limit: None,
        })
        .await;

        assert!(matches!(result, Err(FileError::NotFound { .. })));
    }

    #[tokio::test]
    async fn empty_file_has_zero_lines() {
        let temp_dir = TempDir::new().unwrap();
        let test_path = temp_dir.path().join("empty.txt");
        fs::write(&test_path, "").unwrap();

        let result = read_file_contents(ReadFileArgs {
            file_path: test_path.to_string_lossy().to_string(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();

        assert_eq!(result.total_lines, 0);
        assert_eq!(result.lines_shown, 0);
        assert_eq!(result.content, "");
    }

    #[test]
    fn test_read_file_args_accepts_snake_case_file_path() {
        let args: ReadFileArgs = serde_json::from_value(serde_json::json!({
            "file_path": "/tmp/test.txt",
            "offset": 1,
            "limit": 10
        }))
        .unwrap();

        assert_eq!(args.file_path, "/tmp/test.txt");
        assert_eq!(args.offset, Some(1));
        assert_eq!(args.limit, Some(10));
    }
}
