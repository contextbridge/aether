//! LSP workspace symbol search tool
//!
//! Exposes the LSP `workspace/symbol` request as an MCP tool, enabling
//! workspace-wide symbol search without knowing the file path upfront.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

use crate::lsp::common::{LocationResult, enrich_locations};
use crate::lsp::error::LspError;
use crate::lsp::registry::LspRegistry;
use aether_lspd::{LanguageId, symbol_kind_to_string};

/// Language server selected for a workspace symbol search.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LspWorkspaceSearchLanguage {
    Rust,
    Python,
    JavaScript,
    JavaScriptReact,
    TypeScript,
    TypeScriptReact,
    Go,
    C,
    Cpp,
}

impl From<LspWorkspaceSearchLanguage> for LanguageId {
    fn from(language: LspWorkspaceSearchLanguage) -> Self {
        match language {
            LspWorkspaceSearchLanguage::Rust => Self::Rust,
            LspWorkspaceSearchLanguage::Python => Self::Python,
            LspWorkspaceSearchLanguage::JavaScript => Self::JavaScript,
            LspWorkspaceSearchLanguage::JavaScriptReact => Self::JavaScriptReact,
            LspWorkspaceSearchLanguage::TypeScript => Self::TypeScript,
            LspWorkspaceSearchLanguage::TypeScriptReact => Self::TypeScriptReact,
            LspWorkspaceSearchLanguage::Go => Self::Go,
            LspWorkspaceSearchLanguage::C => Self::C,
            LspWorkspaceSearchLanguage::Cpp => Self::Cpp,
        }
    }
}
/// Input for the `lsp_workspace_search` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LspWorkspaceSearchInput {
    /// Language identifier to query. Languages sharing a server search their combined file extensions.
    pub language: LspWorkspaceSearchLanguage,
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
) -> Result<LspWorkspaceSearchOutput, LspError> {
    if input.query.trim().is_empty() {
        return Err(LspError::InvalidQuery("query cannot be empty".to_string()));
    }
    let language = LanguageId::from(input.language);
    let client = registry.get_or_spawn_for_language(language).await?;
    let symbols = client.workspace_symbol(input.query.clone()).await?;
    let mut all_results: Vec<_> = symbols
        .into_iter()
        .map(|symbol| WorkspaceSymbolResult {
            name: symbol.name,
            kind: symbol_kind_to_string(symbol.kind).to_string(),
            container_name: symbol.container_name,
            location: LocationResult::from_location(&symbol.location),
        })
        .collect();

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

#[cfg(test)]
mod tests {
    use aether_lspd::{LANGUAGE_METADATA, get_config_for_language};

    use super::*;

    #[test]
    fn language_schema_matches_configured_languages() {
        for metadata in LANGUAGE_METADATA.iter().filter(|metadata| get_config_for_language(metadata.id).is_some()) {
            let value = serde_json::json!(metadata.id.as_str());
            assert!(serde_json::from_value::<LspWorkspaceSearchLanguage>(value).is_ok());
        }
    }
}
