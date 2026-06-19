//! LSP tool implementations

pub mod call_hierarchy;
pub mod check_errors;
pub mod document_info;
pub mod rename;
pub mod symbol_lookup;
pub mod workspace_search;

use std::path::Path;

use lsp_types::{DocumentSymbol, DocumentSymbolResponse};

use super::common::{find_symbol_line, path_to_uri};
use super::error::LspError;
use super::registry::LspRegistry;

/// Resolve a symbol's line number using document symbols from the LSP,
/// falling back to a text-based search if the symbol isn't defined in the file
/// (e.g., it's an import or usage site).
///
/// Returns a 1-indexed line number.
pub async fn resolve_symbol_position(file_path: &str, symbol: &str, registry: &LspRegistry) -> Result<u32, LspError> {
    let uri = path_to_uri(Path::new(file_path)).map_err(|e| LspError::Transport(e.to_string()))?;
    let client = registry.require_client(file_path).await?;
    let response = client.document_symbol(uri).await?;

    if let Some(line) = find_in_document_symbol_response(&response, symbol) {
        return Ok(line);
    }

    // Fallback: scan file text for the first word-boundary match.
    // This handles imported/used symbols that aren't in the document symbol tree.
    let content = tokio::fs::read_to_string(file_path).await?;
    find_symbol_line(&content, symbol)
        .ok_or_else(|| LspError::Transport(format!("Symbol '{symbol}' not found in '{file_path}'")))
}

/// Search a `DocumentSymbolResponse` for a symbol by name. Returns 1-indexed line.
fn find_in_document_symbol_response(response: &DocumentSymbolResponse, symbol: &str) -> Option<u32> {
    match response {
        DocumentSymbolResponse::Flat(syms) => {
            syms.iter().find(|s| s.name == symbol).map(|s| s.location.range.start.line + 1)
        }
        DocumentSymbolResponse::Nested(syms) => find_in_nested(syms, symbol),
    }
}

/// Recursively search nested document symbols for a target name.
fn find_in_nested(symbols: &[DocumentSymbol], target: &str) -> Option<u32> {
    for sym in symbols {
        if sym.name == target {
            return Some(sym.selection_range.start.line + 1);
        }
        if let Some(children) = &sym.children
            && let Some(line) = find_in_nested(children, target)
        {
            return Some(line);
        }
    }
    None
}

#[allow(deprecated)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_find_in_document_symbol_response_nested() {
        use lsp_types::{DocumentSymbol, SymbolKind};

        let child = DocumentSymbol {
            name: "inner_fn".to_string(),
            detail: None,
            kind: SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            range: lsp_types::Range {
                start: lsp_types::Position { line: 5, character: 4 },
                end: lsp_types::Position { line: 10, character: 5 },
            },
            selection_range: lsp_types::Range {
                start: lsp_types::Position { line: 5, character: 7 },
                end: lsp_types::Position { line: 5, character: 15 },
            },
            children: None,
        };

        let parent = DocumentSymbol {
            name: "MyStruct".to_string(),
            detail: None,
            kind: SymbolKind::STRUCT,
            tags: None,
            deprecated: None,
            range: lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 0 },
                end: lsp_types::Position { line: 15, character: 1 },
            },
            selection_range: lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 4 },
                end: lsp_types::Position { line: 0, character: 12 },
            },
            children: Some(vec![child]),
        };

        let response = DocumentSymbolResponse::Nested(vec![parent]);

        assert_eq!(find_in_document_symbol_response(&response, "MyStruct"), Some(1));
        assert_eq!(find_in_document_symbol_response(&response, "inner_fn"), Some(6));
        assert_eq!(find_in_document_symbol_response(&response, "nonexistent"), None);
    }

    #[test]
    fn test_find_in_document_symbol_response_flat() {
        #[allow(deprecated)]
        let sym = lsp_types::SymbolInformation {
            name: "my_func".to_string(),
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
        };

        let response = DocumentSymbolResponse::Flat(vec![sym]);

        assert_eq!(find_in_document_symbol_response(&response, "my_func"), Some(11));
        assert_eq!(find_in_document_symbol_response(&response, "other"), None);
    }
}
