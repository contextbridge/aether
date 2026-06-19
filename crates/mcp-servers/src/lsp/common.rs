//! Common types and utilities shared across LSP tools

use std::collections::HashMap;

use lsp_types::Location;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::error::LspError;

/// A location in source code (file path with range)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocationResult {
    /// The file path
    pub file_path: String,
    /// Start line (1-indexed)
    pub start_line: u32,
    /// Start column (1-indexed)
    pub start_column: u32,
    /// End line (1-indexed)
    pub end_line: u32,
    /// End column (1-indexed)
    pub end_column: u32,
    /// Source code context around this location (when `context_lines` is set)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl LocationResult {
    /// Create from a file path and an LSP `Range` (0-indexed → 1-indexed).
    pub fn from_range(file_path: String, range: &lsp_types::Range) -> Self {
        Self {
            file_path,
            start_line: range.start.line + 1,
            start_column: range.start.character + 1,
            end_line: range.end.line + 1,
            end_column: range.end.character + 1,
            context: None,
        }
    }

    /// Create from an LSP Location
    pub fn from_location(loc: &Location) -> Self {
        Self::from_range(uri_to_path(&loc.uri), &loc.range)
    }
}

/// Re-export from `aether_lspd` for convenience.
pub use aether_lspd::uri_to_path;

/// Find the first word-boundary match of `symbol` in `line`.
///
/// Returns the byte offset of the match, or `None` if not found.
/// A word boundary is defined as: the character before/after the match is
/// not alphanumeric or underscore.
pub fn find_word_boundary_match(line: &str, symbol: &str) -> Option<usize> {
    let mut search_start = 0;
    while let Some(pos) = line[search_start..].find(symbol) {
        let abs_pos = search_start + pos;
        let before_ok =
            abs_pos == 0 || !line[..abs_pos].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after_ok = abs_pos + symbol.len() >= line.len()
            || !line[abs_pos + symbol.len()..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');

        if before_ok && after_ok {
            return Some(abs_pos);
        }
        search_start = abs_pos + 1;
    }
    None
}

/// Find the first line containing a word-boundary match of `symbol`.
///
/// Returns the 1-indexed line number, or `None` if no match is found.
pub fn find_symbol_line(content: &str, symbol: &str) -> Option<u32> {
    #[allow(clippy::cast_possible_truncation)] // line counts won't exceed u32
    content
        .lines()
        .enumerate()
        .find(|(_, line)| find_word_boundary_match(line, symbol).is_some())
        .map(|(idx, _)| idx as u32 + 1)
}

/// Find the column position of a symbol on a specific line.
///
/// # Arguments
/// * `content` - The full file content
/// * `symbol` - The symbol name to find
/// * `line` - Line number (1-indexed)
///
/// # Returns
/// The column position (0-indexed) of the first occurrence of the symbol on that line.
pub fn find_symbol_column(content: &str, symbol: &str, line: u32) -> Result<u32, LspError> {
    let line_idx = line.checked_sub(1).ok_or_else(|| LspError::Transport("Line number must be >= 1".to_string()))?;

    let line_content = content
        .lines()
        .nth(line_idx as usize)
        .ok_or_else(|| LspError::Transport(format!("Line {line} not found in file")))?;

    find_word_boundary_match(line_content, symbol)
        .map(|col| u32::try_from(col).unwrap_or(u32::MAX))
        .ok_or_else(|| LspError::Transport(format!("Symbol '{symbol}' not found on line {line}")))
}

/// Re-export from `aether_lspd` for convenience.
pub use aether_lspd::path_to_uri;

/// Extract numbered context lines around a location range.
///
/// Returns lines formatted as `"  {line_number}\t{content}"`, matching the
/// `read_file` tool output convention.
pub fn extract_context(content: &str, start_line: u32, end_line: u32, context_lines: u32) -> String {
    let lines: Vec<&str> = content.lines().collect();
    #[allow(clippy::cast_possible_truncation)] // line counts won't exceed u32
    let total = lines.len() as u32;
    if total == 0 {
        return String::new();
    }

    // start_line / end_line are 1-indexed
    let from = start_line.saturating_sub(context_lines).max(1);
    let to = (end_line + context_lines).min(total);

    let width = digit_count(to);

    let mut buf = String::new();
    for line_num in from..=to {
        let idx = (line_num - 1) as usize;
        if let Some(line) = lines.get(idx) {
            use std::fmt::Write;
            let _ = writeln!(buf, "{:>width$}\t{}", line_num, line, width = width as usize);
        }
    }

    // Trim the trailing newline
    if buf.ends_with('\n') {
        buf.pop();
    }
    buf
}

/// Enrich a slice of `LocationResult`s with source context.
///
/// Groups locations by file path, reads each file once, then injects context
/// into each location. Errors (missing files, etc.) are silently skipped —
/// the location simply gets no context.
pub async fn enrich_locations(locations: &mut [LocationResult], context_lines: u32) {
    // Group indices by file path
    let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, loc) in locations.iter().enumerate() {
        by_file.entry(loc.file_path.clone()).or_default().push(i);
    }

    for (path, indices) in &by_file {
        let Ok(content) = tokio::fs::read_to_string(path).await else {
            continue;
        };
        for &i in indices {
            let loc = &locations[i];
            let ctx = extract_context(&content, loc.start_line, loc.end_line, context_lines);
            if !ctx.is_empty() {
                locations[i].context = Some(ctx);
            }
        }
    }
}

fn digit_count(n: u32) -> u32 {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_symbol_column_basic() {
        let content = "fn main() {\n    let x = HashMap::new();\n}";
        assert_eq!(find_symbol_column(content, "HashMap", 2).unwrap(), 12);
    }

    #[test]
    fn test_find_symbol_column_first_line() {
        let content = "use std::collections::HashMap;";
        assert_eq!(find_symbol_column(content, "HashMap", 1).unwrap(), 22);
    }

    #[test]
    fn test_find_symbol_column_word_boundary() {
        let content = "let x = HashMapExtra::new();";
        assert!(find_symbol_column(content, "HashMap", 1).is_err());
    }

    #[test]
    fn test_find_symbol_column_word_boundary_prefix() {
        let content = "let x = MyHashMap::new();";
        assert!(find_symbol_column(content, "HashMap", 1).is_err());
    }

    #[test]
    fn test_find_symbol_column_underscore_boundary() {
        let content = "let hash_map = 1;";
        assert!(find_symbol_column(content, "hash", 1).is_err());
    }

    #[test]
    fn test_find_symbol_column_not_found() {
        let content = "fn main() {}";
        assert!(find_symbol_column(content, "HashMap", 1).is_err());
    }

    #[test]
    fn test_find_symbol_column_line_out_of_range() {
        let content = "fn main() {}";
        assert!(find_symbol_column(content, "main", 99).is_err());
    }

    #[test]
    fn test_find_symbol_column_zero_line() {
        let content = "fn main() {}";
        assert!(find_symbol_column(content, "main", 0).is_err());
    }

    #[test]
    fn test_find_symbol_column_multiple_on_line() {
        let content = "let x = foo + foo;";
        assert_eq!(find_symbol_column(content, "foo", 1).unwrap(), 8);
    }

    #[test]
    fn test_extract_context_basic() {
        let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7";
        // Location at line 4, 1 context line on each side => lines 3-5
        let ctx = extract_context(content, 4, 4, 1);
        assert_eq!(ctx, "3\tline3\n4\tline4\n5\tline5");
    }

    #[test]
    fn test_extract_context_clamps_to_start() {
        let content = "line1\nline2\nline3";
        // Location at line 1, 3 context lines => should clamp to line 1
        let ctx = extract_context(content, 1, 1, 3);
        assert_eq!(ctx, "1\tline1\n2\tline2\n3\tline3");
    }

    #[test]
    fn test_extract_context_clamps_to_end() {
        let content = "line1\nline2\nline3";
        let ctx = extract_context(content, 3, 3, 5);
        assert_eq!(ctx, "1\tline1\n2\tline2\n3\tline3");
    }

    #[test]
    fn test_extract_context_multiline_range() {
        let content = "a\nb\nc\nd\ne\nf";
        // Range lines 2-4, 1 context line => lines 1-5
        let ctx = extract_context(content, 2, 4, 1);
        assert_eq!(ctx, "1\ta\n2\tb\n3\tc\n4\td\n5\te");
    }

    #[test]
    fn test_extract_context_zero_context_lines() {
        let content = "a\nb\nc";
        let ctx = extract_context(content, 2, 2, 0);
        assert_eq!(ctx, "2\tb");
    }

    #[test]
    fn test_extract_context_empty_content() {
        let ctx = extract_context("", 1, 1, 2);
        assert_eq!(ctx, "");
    }

    // --- find_word_boundary_match tests ---

    #[test]
    fn test_word_boundary_match_basic() {
        assert_eq!(find_word_boundary_match("use std::HashMap;", "HashMap"), Some(9));
    }

    #[test]
    fn test_word_boundary_match_no_partial() {
        assert_eq!(find_word_boundary_match("let x = HashMapExtra;", "HashMap"), None);
    }

    // --- find_symbol_line tests ---

    #[test]
    fn test_find_symbol_line_import() {
        let content = "use crate::config::AppState;\n\nfn main() {}";
        assert_eq!(find_symbol_line(content, "AppState"), Some(1));
    }

    #[test]
    fn test_find_symbol_line_definition_on_later_line() {
        let content = "use std::fmt;\n\npub struct AppState {\n    pub name: String,\n}";
        assert_eq!(find_symbol_line(content, "AppState"), Some(3));
    }

    #[test]
    fn test_find_symbol_line_not_found() {
        let content = "fn main() {}\nfn helper() {}";
        assert_eq!(find_symbol_line(content, "AppState"), None);
    }

    #[test]
    fn test_find_symbol_line_ignores_partial_match() {
        let content = "let app_state_extra = 1;\nlet app_state = AppState::new();";
        // Should match line 2 where AppState appears as a whole word
        assert_eq!(find_symbol_line(content, "AppState"), Some(2));
    }

    #[test]
    fn test_find_symbol_line_first_occurrence_wins() {
        let content = "// AppState is used here\nstruct AppState {}";
        assert_eq!(find_symbol_line(content, "AppState"), Some(1));
    }
}
