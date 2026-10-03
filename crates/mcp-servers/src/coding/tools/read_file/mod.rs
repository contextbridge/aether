use crate::file_ops::{FileError, read_text_file};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};

const MAX_LINE_BYTES: usize = 2000;
const PAGE_BYTES: usize = 32_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileArgs {
    /// Path to the file to read (must be an existing file)
    #[serde(alias = "file_path")]
    pub file_path: String,
    /// Starting line number to read from (1-indexed). If not specified, starts from line 1.
    pub offset: Option<usize>,
    /// Maximum number of lines to read.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
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
    let end_idx = args.limit.map_or(total_lines, |limit| (start_idx + limit).min(total_lines));
    let mut lines_with_numbers = Vec::new();
    let mut content_bytes = 0;
    for (line_num, line) in (offset..).zip(&all_lines[start_idx..end_idx]) {
        let numbered = number_line(line_num, line);
        content_bytes += numbered.len() + 1;
        if content_bytes > PAGE_BYTES {
            break;
        }
        lines_with_numbers.push(numbered);
    }
    let lines_shown = lines_with_numbers.len();
    let next_offset = (start_idx + lines_shown < total_lines).then_some(offset + lines_shown);

    let formatted_content = lines_with_numbers.join("\n");

    let display_meta = ToolDisplayMeta::new("Read file", format!("{}, {total_lines} lines", basename(&args.file_path)));

    Ok(ReadFileResult {
        status: "success".to_string(),
        file_path: args.file_path,
        content: formatted_content,
        total_lines,
        lines_shown,
        next_offset,
        size: content.len(),
        raw_content: content,
        meta: Some(display_meta.into()),
    })
}

fn number_line(line_num: usize, line: &str) -> String {
    if line.len() > MAX_LINE_BYTES {
        let prefix = &line[..line.floor_char_boundary(MAX_LINE_BYTES)];
        format!("{line_num:5}\t{prefix}... [truncated, {} bytes total]", line.len())
    } else {
        format!("{line_num:5}\t{line}")
    }
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
        assert_eq!(result.next_offset, None);
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
        assert_eq!(result.next_offset, Some(4));
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
    async fn test_read_file_stops_at_byte_budget_and_continues_from_next_offset() {
        let workspace = TestWorkspace::new().file("test_byte_budget.txt", vec!["x".repeat(100); 1000].join("\n"));
        let file_path = workspace.path_string("test_byte_budget.txt");

        let first =
            read_file_contents(ReadFileArgs { file_path: file_path.clone(), offset: None, limit: None }).await.unwrap();
        assert!(first.lines_shown < first.total_lines);
        let next_offset = first.next_offset.expect("lines should remain");
        assert_eq!(next_offset, 1 + first.lines_shown);

        let second =
            read_file_contents(ReadFileArgs { file_path, offset: Some(next_offset), limit: None }).await.unwrap();
        assert!(second.content.starts_with(&format!("{next_offset:5}\t")));
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
