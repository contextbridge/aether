//! LSP workspace symbol search tool
//!
//! Exposes the LSP `workspace/symbol` request as an MCP tool, enabling
//! workspace-wide symbol search without knowing the file path upfront.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

use crate::lsp::common::{LocationResult, enrich_locations};
use crate::lsp::registry::LspRegistry;
use aether_lspd::symbol_kind_to_string;

/// Input for the `lsp_workspace_search` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspWorkspaceSearchInput {
    /// Search query (e.g., "`AppState`", "Repository")
    pub query: String,
    /// Maximum number of results to return
    #[serde(default)]
    pub limit: Option<usize>,
    /// Number of context lines to include around each result
    #[serde(default, alias = "context_lines")]
    pub context_lines: Option<u32>,
}

/// A single workspace symbol result
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSymbolResult {
    /// The symbol name
    pub name: String,
    /// The kind of symbol (function, struct, etc.)
    pub kind: String,
    /// Parent module or class name, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    /// The source location
    pub location: LocationResult,
}

/// Output from the `lsp_workspace_search` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspWorkspaceSearchOutput {
    /// The query that was searched
    pub query: String,
    /// Matching symbols
    pub results: Vec<WorkspaceSymbolResult>,
    /// Total number of results before truncation
    pub total_count: usize,
    /// Whether results were truncated due to `limit`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Execute the `lsp_workspace_search` operation
pub async fn execute_lsp_workspace_search(
    input: LspWorkspaceSearchInput,
    registry: &LspRegistry,
) -> Result<LspWorkspaceSearchOutput, String> {
    let clients = registry.active_clients().await;
    if clients.is_empty() {
        return Err("No active LSP clients. Open a file first so the LSP can start.".to_string());
    }

    let mut all_results: Vec<WorkspaceSymbolResult> = Vec::new();

    for client in &clients {
        let symbols = client.workspace_symbol(input.query.clone()).await.map_err(|e| e.to_string())?;

        for sym in symbols {
            let location = LocationResult::from_location(&sym.location);
            all_results.push(WorkspaceSymbolResult {
                name: sym.name,
                kind: symbol_kind_to_string(sym.kind).to_string(),
                container_name: sym.container_name,
                location,
            });
        }
    }

    // Deduplicate by (name, file_path, start_line)
    let mut seen = HashSet::new();
    all_results.retain(|r| seen.insert((r.name.clone(), r.location.file_path.clone(), r.location.start_line)));

    let total_count = all_results.len();
    let truncated = input.limit.is_some_and(|l| total_count > l);
    if let Some(l) = input.limit {
        all_results.truncate(l);
    }

    // Enrich with context lines if requested
    if let Some(n) = input.context_lines.filter(|&n| n > 0) {
        let mut locations: Vec<LocationResult> = all_results.iter().map(|r| r.location.clone()).collect();
        enrich_locations(&mut locations, n).await;
        for (result, enriched) in all_results.iter_mut().zip(locations) {
            result.location = enriched;
        }
    }

    let display_meta = ToolDisplayMeta::new("LSP search", format!("'{}' ({total_count} results)", input.query));

    Ok(LspWorkspaceSearchOutput {
        query: input.query,
        results: all_results,
        total_count,
        truncated: if truncated { Some(true) } else { None },
        meta: Some(display_meta.into()),
    })
}
