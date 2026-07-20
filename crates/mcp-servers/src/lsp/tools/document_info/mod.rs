//! LSP document info tool
//!
//! This module provides document-level LSP operations:
//! - symbols: Get all symbols in a document

use lsp_types::DocumentSymbolResponse;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};

use crate::lsp::common::{LocationResult, path_to_uri};
use crate::lsp::registry::LspRegistry;
use aether_lspd::symbol_kind_to_string;

/// Input for the `lsp_document` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentInput {
    /// The file path to analyze
    #[serde(alias = "file_path")]
    pub file_path: String,
}

/// A document symbol with hierarchical structure
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbolResult {
    /// The name of the symbol
    pub name: String,
    /// The kind of the symbol (function, class, etc.)
    pub kind: String,
    /// Additional detail about the symbol
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The range of the entire symbol
    pub range: LocationResult,
    /// The range of the symbol name
    pub selection_range: LocationResult,
    /// Child symbols (for hierarchical structure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<DocumentSymbolResult>>,
}

/// Output from the `lsp_document` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentOutput {
    /// The symbols in the document
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbols: Option<Vec<DocumentSymbolResult>>,
    /// Total count of top-level symbols
    pub total_count: usize,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Execute the `lsp_document` operation
pub async fn execute_lsp_document(
    input: LspDocumentInput,
    registry: &LspRegistry,
) -> Result<LspDocumentOutput, String> {
    let uri = path_to_uri(Path::new(&input.file_path)).map_err(|e| e.to_string())?;
    let client = registry.get_or_spawn(Path::new(&input.file_path)).await.map_err(|e| e.to_string())?;
    let response = client.document_symbol(uri).await.map_err(|e| e.to_string())?;

    let symbols = convert_document_symbols(&input.file_path, response);
    let total_count = symbols.len();

    let display_meta =
        ToolDisplayMeta::new("LSP document", format!("{}, {total_count} symbols", basename(&input.file_path)));

    Ok(LspDocumentOutput { symbols: Some(symbols), total_count, meta: Some(display_meta.into()) })
}

/// Convert `DocumentSymbolResponse` to our result format
fn convert_document_symbols(file_path: &str, response: DocumentSymbolResponse) -> Vec<DocumentSymbolResult> {
    match response {
        DocumentSymbolResponse::Flat(symbols) => symbols
            .into_iter()
            .map(|sym| {
                let range = LocationResult::from_range(file_path.to_string(), &sym.location.range);
                DocumentSymbolResult {
                    name: sym.name,
                    kind: symbol_kind_to_string(sym.kind).to_string(),
                    detail: None,
                    range: range.clone(),
                    selection_range: range,
                    children: None,
                }
            })
            .collect(),
        DocumentSymbolResponse::Nested(symbols) => {
            symbols.into_iter().map(|sym| convert_document_symbol(file_path, sym)).collect()
        }
    }
}

/// Convert a single `DocumentSymbol` to our result format (recursive for children)
fn convert_document_symbol(file_path: &str, sym: lsp_types::DocumentSymbol) -> DocumentSymbolResult {
    let range = LocationResult::from_range(file_path.to_string(), &sym.range);
    let selection_range = LocationResult::from_range(file_path.to_string(), &sym.selection_range);

    let children = sym
        .children
        .map(|children| children.into_iter().map(|child| convert_document_symbol(file_path, child)).collect());

    DocumentSymbolResult {
        name: sym.name,
        kind: symbol_kind_to_string(sym.kind).to_string(),
        detail: sym.detail,
        range,
        selection_range,
        children,
    }
}
