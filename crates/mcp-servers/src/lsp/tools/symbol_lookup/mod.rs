//! Consolidated LSP symbol lookup tool
//!
//! This module provides a unified interface for symbol-based LSP operations:
//! - definition: Go to the definition of a symbol
//! - references: Find all references to a symbol
//! - hover: Get type and documentation info for a symbol

use std::path::Path;

use lsp_types::GotoDefinitionResponse;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};

use crate::lsp::common::{DEFAULT_RESULT_LIMIT, truncate_results, uri_to_path};
use crate::lsp::error::LspError;
use crate::lsp::registry::LspRegistry;
use crate::lsp::render::{SourceLine, render_locations};

/// The operation to perform on a symbol
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SymbolLookupOperation {
    /// Go to the definition of the symbol, including into dependency and standard-library source
    Definition,
    /// Find all references to the symbol
    References,
    /// Get hover information (type, documentation) for the symbol
    Hover,
}

/// Input for the `lsp_symbol` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspSymbolInput {
    /// The operation to perform
    pub operation: SymbolLookupOperation,
    /// The file containing the symbol, absolute or relative to the workspace root
    #[serde(alias = "file_path")]
    pub file_path: String,
    /// The symbol name to look up (e.g., "`HashMap`", "spawn", "`LspClient`")
    pub symbol: String,
    /// Optional 1-indexed line hint. A matching line avoids automatic resolution;
    /// stale hints fall back to searching the document.
    #[serde(default)]
    pub line: Option<u32>,
    /// Whether to include the declaration in references results (default: true, only used for references operation)
    #[serde(default = "default_true", alias = "include_declaration")]
    pub include_declaration: bool,
    /// Maximum number of locations to return (default: 50). When results are dropped,
    /// `truncated: true` is included in the response.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Number of context lines to include around returned source locations.
    #[serde(default, alias = "context_lines")]
    pub context_lines: Option<u32>,
}

fn default_true() -> bool {
    true
}

/// Output from the `lsp_symbol` tool
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspSymbolOutput {
    /// The operation that was performed
    pub operation: String,
    /// Locations for definition and references, grouped by file with paths relative to the workspace root:
    /// `path: 3, 18`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<String>,
    /// Hover contents as markdown (for hover operation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hover_contents: Option<String>,
    /// Total count of results (reflects full count before any truncation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
    /// Whether the results were truncated due to `limit`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

impl LspSymbolOutput {
    /// Definition or references output: capped at `limit` and grouped by file.
    async fn locations(
        operation: &str,
        mut locations: Vec<SourceLine>,
        input: &LspSymbolInput,
        project_root: &Path,
    ) -> Self {
        let truncation = truncate_results(&mut locations, input.limit.unwrap_or(DEFAULT_RESULT_LIMIT));
        Self {
            operation: operation.to_string(),
            locations: Some(render_locations(&locations, project_root, input.context_lines).await),
            total_count: Some(truncation.total_count),
            truncated: truncation.truncated,
            ..Self::default()
        }
    }
}

/// Execute the `lsp_symbol` operation
pub async fn execute_lsp_symbol(
    mut input: LspSymbolInput,
    registry: &LspRegistry,
) -> Result<LspSymbolOutput, LspError> {
    input.file_path = registry.resolve_file(&input.file_path)?;
    let resolved = registry.resolve_symbol(&input.file_path, &input.symbol, input.line).await?;
    let root = registry.root_path();
    let mut output = match input.operation {
        SymbolLookupOperation::Definition => {
            let response = resolved.client.goto_definition(resolved.uri, resolved.line, resolved.column).await?;
            LspSymbolOutput::locations("definition", definition_response_to_lines(response), &input, root).await
        }
        SymbolLookupOperation::References => {
            let lsp_locations = resolved
                .client
                .find_references(resolved.uri, resolved.line, resolved.column, input.include_declaration)
                .await?;
            let locations = lsp_locations.iter().map(SourceLine::from_location).collect();
            LspSymbolOutput::locations("references", locations, &input, root).await
        }
        SymbolLookupOperation::Hover => {
            let hover = resolved.client.hover(resolved.uri, resolved.line, resolved.column).await?;
            LspSymbolOutput {
                operation: "hover".to_string(),
                hover_contents: hover.map(|h| format_hover_contents(&h)),
                ..LspSymbolOutput::default()
            }
        }
    };

    #[allow(clippy::used_underscore_binding)]
    {
        output.meta = Some(symbol_display_meta(&input, &output).into());
    }
    Ok(output)
}

/// Convert `GotoDefinitionResponse` to a list of `SourceLine`s
fn definition_response_to_lines(response: GotoDefinitionResponse) -> Vec<SourceLine> {
    match response {
        GotoDefinitionResponse::Scalar(loc) => vec![SourceLine::from_location(&loc)],
        GotoDefinitionResponse::Array(locs) => locs.iter().map(SourceLine::from_location).collect(),
        GotoDefinitionResponse::Link(links) => links
            .iter()
            .map(|link| SourceLine::from_range(uri_to_path(&link.target_uri), &link.target_selection_range))
            .collect(),
    }
}

/// Format hover contents to a readable string
fn format_hover_contents(hover: &lsp_types::Hover) -> String {
    use lsp_types::HoverContents;

    match &hover.contents {
        HoverContents::Scalar(marked) => format_marked_string(marked),
        HoverContents::Array(arr) => arr.iter().map(format_marked_string).collect::<Vec<_>>().join("\n\n"),
        HoverContents::Markup(markup) => markup.value.clone(),
    }
}

fn symbol_display_meta(input: &LspSymbolInput, output: &LspSymbolOutput) -> ToolDisplayMeta {
    let symbol = &input.symbol;
    let file = basename(&input.file_path);
    match input.operation {
        SymbolLookupOperation::Definition => ToolDisplayMeta::new("LSP definition", format!("{symbol} in {file}")),
        SymbolLookupOperation::References => {
            let count = output.total_count.unwrap_or(0);
            ToolDisplayMeta::new("LSP references", format!("{symbol} ({count} refs)"))
        }
        SymbolLookupOperation::Hover => ToolDisplayMeta::new("LSP hover", format!("{symbol} in {file}")),
    }
}

/// Format a single `MarkedString`
fn format_marked_string(marked: &lsp_types::MarkedString) -> String {
    match marked {
        lsp_types::MarkedString::String(s) => s.clone(),
        lsp_types::MarkedString::LanguageString(ls) => {
            format!("```{}\n{}\n```", ls.language, ls.value)
        }
    }
}
