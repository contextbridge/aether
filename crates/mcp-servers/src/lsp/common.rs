//! Common types and utilities shared across LSP tools

use lsp_types::{DocumentSymbol, DocumentSymbolResponse};

use super::error::LspError;

/// Number of results returned when a tool's `limit` is not set.
pub const DEFAULT_RESULT_LIMIT: usize = 50;

/// Counts recorded when a result list is capped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncation {
    /// Number of results before the cap
    pub total_count: usize,
    /// `Some(true)` when results were dropped, so it serializes only when set
    pub truncated: Option<bool>,
}

/// Cap `items` at `max`, recording the full count and whether anything was dropped.
pub fn truncate_results<T>(items: &mut Vec<T>, max: usize) -> Truncation {
    let total_count = items.len();
    items.truncate(max);
    Truncation { total_count, truncated: (total_count > max).then_some(true) }
}

/// Visit every symbol in a document-symbol response.
pub fn for_each_document_symbol(
    response: &DocumentSymbolResponse,
    visit: &mut impl FnMut(&str, lsp_types::SymbolKind, &lsp_types::Range, Option<&str>),
) {
    match response {
        DocumentSymbolResponse::Flat(symbols) => {
            for symbol in symbols {
                visit(&symbol.name, symbol.kind, &symbol.location.range, symbol.container_name.as_deref());
            }
        }
        DocumentSymbolResponse::Nested(symbols) => {
            for symbol in symbols {
                visit_nested_document_symbol(symbol, None, visit);
            }
        }
    }
}

/// Find an exact symbol in an LSP document-symbol response and return its 1-indexed line.
pub fn find_document_symbol_line(response: &DocumentSymbolResponse, symbol: &str) -> Option<u32> {
    let mut line = None;
    for_each_document_symbol(response, &mut |name, _, selection_range, _| {
        if line.is_none() && name == symbol {
            line = Some(selection_range.start.line + 1);
        }
    });
    line
}

fn visit_nested_document_symbol(
    symbol: &DocumentSymbol,
    container_name: Option<&str>,
    visit: &mut impl FnMut(&str, lsp_types::SymbolKind, &lsp_types::Range, Option<&str>),
) {
    visit(&symbol.name, symbol.kind, &symbol.selection_range, container_name);
    if let Some(children) = &symbol.children {
        for child in children {
            visit_nested_document_symbol(child, Some(&symbol.name), visit);
        }
    }
}

/// Re-export from `aether_lspd` for convenience.
pub use aether_lspd::uri_to_path;

/// Find the first word-boundary match and return its byte offset.
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
    let line_idx =
        line.checked_sub(1).ok_or_else(|| LspError::InvalidPosition("Line number must be >= 1".to_string()))?;

    let line_content = content
        .lines()
        .nth(line_idx as usize)
        .ok_or_else(|| LspError::InvalidPosition(format!("Line {line} not found in file")))?;

    find_word_boundary_match(line_content, symbol)
        .map(|col| u32::try_from(col).unwrap_or(u32::MAX))
        .ok_or_else(|| LspError::SymbolNotFound(format!("Symbol '{symbol}' not found on line {line}")))
}

/// Re-export from `aether_lspd` for convenience.
pub use aether_lspd::path_to_uri;

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
    fn test_word_boundary_match_basic() {
        assert_eq!(find_word_boundary_match("use std::HashMap;", "HashMap"), Some(9));
    }

    #[test]
    fn test_word_boundary_match_no_partial() {
        assert_eq!(find_word_boundary_match("let x = HashMapExtra;", "HashMap"), None);
    }

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
    fn truncation_records_the_full_count_and_flags_dropped_results() {
        let mut items = vec![1, 2, 3];

        assert_eq!(truncate_results(&mut items, 5), Truncation { total_count: 3, truncated: None });
        assert_eq!(truncate_results(&mut items, 2), Truncation { total_count: 3, truncated: Some(true) });
        assert_eq!(items, [1, 2]);
    }

    #[allow(deprecated)]
    #[test]
    fn test_find_document_symbol_line_nested() {
        let child = DocumentSymbol {
            name: "inner_fn".to_string(),
            detail: None,
            kind: lsp_types::SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            range: lsp_types::Range::default(),
            selection_range: lsp_types::Range {
                start: lsp_types::Position { line: 5, character: 7 },
                end: lsp_types::Position { line: 5, character: 15 },
            },
            children: None,
        };
        let parent = DocumentSymbol {
            name: "MyStruct".to_string(),
            detail: None,
            kind: lsp_types::SymbolKind::STRUCT,
            tags: None,
            deprecated: None,
            range: lsp_types::Range::default(),
            selection_range: lsp_types::Range::default(),
            children: Some(vec![child]),
        };

        let response = DocumentSymbolResponse::Nested(vec![parent]);

        assert_eq!(find_document_symbol_line(&response, "MyStruct"), Some(1));
        assert_eq!(find_document_symbol_line(&response, "inner_fn"), Some(6));
        assert_eq!(find_document_symbol_line(&response, "missing"), None);
    }

    #[allow(deprecated)]
    #[test]
    fn test_find_document_symbol_line_flat() {
        use std::str::FromStr;

        let response = DocumentSymbolResponse::Flat(vec![lsp_types::SymbolInformation {
            name: "my_function".to_string(),
            kind: lsp_types::SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            location: lsp_types::Location {
                uri: lsp_types::Uri::from_str("file:///test.rs").unwrap(),
                range: lsp_types::Range {
                    start: lsp_types::Position { line: 10, character: 0 },
                    end: lsp_types::Position { line: 20, character: 1 },
                },
            },
            container_name: None,
        }]);

        assert_eq!(find_document_symbol_line(&response, "my_function"), Some(11));
        assert_eq!(find_document_symbol_line(&response, "missing"), None);
    }

    #[test]
    fn test_find_symbol_line_first_occurrence_wins() {
        let content = "// AppState is used here\nstruct AppState {}";
        assert_eq!(find_symbol_line(content, "AppState"), Some(1));
    }
}
