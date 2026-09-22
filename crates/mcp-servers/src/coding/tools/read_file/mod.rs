use crate::file_ops::{FileError, read_text_file};
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

const MAX_LINE_BYTES: usize = 2000;
const DEFAULT_LINE_LIMIT: usize = 2000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
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
    let content = read_text_file(Path::new(&args.file_path)).await?;

    let all_lines: Vec<&str> = content.lines().collect();
    let total_lines = all_lines.len();

    // Default offset to 1 if not provided
    let offset = args.offset.unwrap_or(1);

    // Validate offset is 1-indexed
    if offset == 0 {
        return Err(FileError::InvalidOffset { path: args.file_path });
    }

    let start_idx = (offset - 1).min(total_lines);
    let limit = args.limit.unwrap_or(DEFAULT_LINE_LIMIT);
    let end_idx = (start_idx + limit).min(total_lines);
    let selected_lines: Vec<&str> = all_lines[start_idx..end_idx].to_vec();
    let lines_with_numbers: Vec<String> = selected_lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let line_num = offset + i;
            if line.len() > MAX_LINE_BYTES {
                let prefix = &line[..line.floor_char_boundary(MAX_LINE_BYTES)];
                format!("{line_num:5}\t{prefix}... [truncated, {} bytes total]", line.len())
            } else {
                format!("{line_num:5}\t{line}")
            }
        })
        .collect();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestWorkspace;

    #[tokio::test]
    async fn test_read_file_with_defaults() {
        let workspace = TestWorkspace::new().file("test_read_defaults.txt", "line 1\nline 2\nline 3");

        let result = read_file_contents(ReadFileArgs {
            file_path: workspace.path_string("test_read_defaults.txt"),
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
        let workspace = TestWorkspace::new().file("test_offset_limit.txt", "line 1\nline 2\nline 3\nline 4\nline 5");

        let result = read_file_contents(ReadFileArgs {
            file_path: workspace.path_string("test_offset_limit.txt"),
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
        let long_line = "x".repeat(2500);
        let workspace = TestWorkspace::new().file("test_truncation.txt", format!("short\n{long_line}"));

        let result = read_file_contents(ReadFileArgs {
            file_path: workspace.path_string("test_truncation.txt"),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.content.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("short"));
        assert!(!lines[0].contains("truncated"));
        assert!(lines[1].contains("truncated"));
        assert!(lines[1].contains("2500 bytes total"));
    }

    #[tokio::test]
    async fn test_read_file_multibyte_line_truncation() {
        let long_line = "─".repeat(1000);
        let workspace = TestWorkspace::new().file("test_multibyte_truncation.txt", long_line);

        let result = read_file_contents(ReadFileArgs {
            file_path: workspace.path_string("test_multibyte_truncation.txt"),
            offset: None,
            limit: None,
        })
        .await
        .unwrap();

        assert_eq!(result.content, format!("    1\t{}... [truncated, 3000 bytes total]", "─".repeat(666)));
    }

    #[tokio::test]
    async fn test_read_file_default_limit() {
        let test_content: String = (1..=2500).map(|i| format!("Line {i}")).collect::<Vec<_>>().join("\n");
        let workspace = TestWorkspace::new().file("test_default_limit.txt", test_content);

        let result = read_file_contents(ReadFileArgs {
            file_path: workspace.path_string("test_default_limit.txt"),
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
        let workspace = TestWorkspace::new().file("test_invalid_offset.txt", "line 1");

        let result = read_file_contents(ReadFileArgs {
            file_path: workspace.path_string("test_invalid_offset.txt"),
            offset: Some(0),
            limit: None,
        })
        .await;

        assert!(matches!(result, Err(FileError::InvalidOffset { .. })));
    }

    #[tokio::test]
    async fn test_read_file_nonexistent() {
        let workspace = TestWorkspace::new();

        let result = read_file_contents(ReadFileArgs {
            file_path: workspace.path_string("nonexistent_file_xyz123.txt"),
            offset: None,
            limit: None,
        })
        .await;

        assert!(matches!(result, Err(FileError::NotFound { .. })));
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
