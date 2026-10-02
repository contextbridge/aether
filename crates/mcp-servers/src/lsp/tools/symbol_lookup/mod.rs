//! Consolidated LSP symbol lookup tool
//!
//! This module provides a unified interface for symbol-based LSP operations:
//! - definition: Go to the definition of a symbol
//! - references: Find all references to a symbol
//! - hover: Get type and documentation info for a symbol

use lsp_types::GotoDefinitionResponse;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};

use crate::lsp::common::{LocationResult, uri_to_path};
use crate::lsp::error::LspError;
use crate::lsp::registry::LspRegistry;

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
    /// Maximum number of results to return. When set, results are truncated and
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
    /// Location results (for definition, references)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<LocationResult>>,
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
    fn with_locations(operation: &str, locations: Vec<LocationResult>, limit: Option<usize>) -> Self {
        let total_count = locations.len();
        let truncated = limit.is_some_and(|l| total_count > l);
        let locations = match limit {
            Some(l) if total_count > l => locations.into_iter().take(l).collect(),
            _ => locations,
        };
        Self {
            operation: operation.to_string(),
            locations: Some(locations),
            total_count: Some(total_count),
            truncated: if truncated { Some(true) } else { None },
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
    let mut output = match input.operation {
        SymbolLookupOperation::Definition => {
            let response = resolved.client.goto_definition(resolved.uri, resolved.line, resolved.column).await?;
            let locations = definition_response_to_locations(response);
            let mut output = LspSymbolOutput::with_locations("definition", locations, input.limit);
            enrich_locations_with_context(&mut output, input.context_lines).await;
            output
        }
        SymbolLookupOperation::References => {
            let lsp_locations = resolved
                .client
                .find_references(resolved.uri, resolved.line, resolved.column, input.include_declaration)
                .await?;
            let locations: Vec<LocationResult> = lsp_locations.iter().map(LocationResult::from_location).collect();
            let mut output = LspSymbolOutput::with_locations("references", locations, input.limit);
            enrich_locations_with_context(&mut output, input.context_lines).await;
            output
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

/// Enrich locations in the output with source code context when `context_lines` is set.
async fn enrich_locations_with_context(output: &mut LspSymbolOutput, context_lines: Option<u32>) {
    let Some(n) = context_lines.filter(|&n| n > 0) else {
        return;
    };
    if let Some(locations) = output.locations.as_mut() {
        super::super::common::enrich_locations(locations, n).await;
    }
}

/// Convert `GotoDefinitionResponse` to a list of `LocationResult`
fn definition_response_to_locations(response: GotoDefinitionResponse) -> Vec<LocationResult> {
    match response {
        GotoDefinitionResponse::Scalar(loc) => vec![LocationResult::from_location(&loc)],
        GotoDefinitionResponse::Array(locs) => locs.iter().map(LocationResult::from_location).collect(),
        GotoDefinitionResponse::Link(links) => links
            .iter()
            .map(|link| LocationResult::from_range(uri_to_path(&link.target_uri), &link.target_selection_range))
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
