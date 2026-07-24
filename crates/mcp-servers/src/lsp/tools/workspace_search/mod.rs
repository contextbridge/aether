//! LSP workspace symbol search tool
//!
//! Exposes the LSP `workspace/symbol` request as an MCP tool, enabling
//! workspace-wide symbol search without knowing the file path upfront.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use lsp_types::DocumentSymbolResponse;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

use crate::lsp::common::{LocationResult, enrich_locations, for_each_document_symbol, path_to_uri, uri_to_path};
use crate::lsp::error::LspError;
use crate::lsp::registry::LspRegistry;
use crate::search::find_files_containing;
use aether_lspd::{LanguageId, LspClient, get_config_for_language, metadata_for, symbol_kind_to_string};

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
    /// How the symbol was discovered.
    pub source: WorkspaceSymbolSource,
    /// The source location
    pub location: LocationResult,
}

/// Source used to discover a workspace symbol.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceSymbolSource {
    /// Returned directly by the language server's workspace symbol index.
    WorkspaceSymbol,
    /// Recovered by querying document symbols in candidate project files.
    DocumentSymbolFallback,
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
    let server_extensions = server_extensions(language);
    let symbols = client.workspace_symbol(input.query.clone()).await?;
    let mut all_results: Vec<_> = symbols
        .into_iter()
        .filter(|symbol| is_server_language_path(&uri_to_path(&symbol.location.uri), &server_extensions))
        .map(|symbol| WorkspaceSymbolResult {
            name: symbol.name,
            kind: symbol_kind_to_string(symbol.kind).to_string(),
            container_name: symbol.container_name,
            source: WorkspaceSymbolSource::WorkspaceSymbol,
            location: LocationResult::from_location(&symbol.location),
        })
        .collect();

    if all_results.is_empty() {
        all_results = document_symbol_fallback(&input.query, registry.root_path(), &client, &server_extensions).await?;
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

fn is_server_language_path(path: &str, extensions: &[&str]) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension))
}

fn server_extensions(language: LanguageId) -> Vec<&'static str> {
    get_config_for_language(language)
        .into_iter()
        .flat_map(|config| config.languages.iter())
        .filter_map(|language| metadata_for(*language))
        .flat_map(|metadata| metadata.extensions.iter().copied())
        .collect()
}

async fn document_symbol_fallback(
    query: &str,
    root: &Path,
    client: &Arc<LspClient>,
    extensions: &[&'static str],
) -> Result<Vec<WorkspaceSymbolResult>, LspError> {
    let candidates = find_files_containing(root.to_path_buf(), query.to_string(), extensions.to_vec(), 100)
        .await
        .map_err(LspError::Search)?;
    let mut results = Vec::new();
    for path in candidates {
        let path_text = path.to_string_lossy().to_string();
        let Ok(uri) = path_to_uri(&path) else {
            continue;
        };
        let Ok(response) = client.document_symbol(uri).await else {
            continue;
        };
        collect_matching_document_symbols(&path_text, query, &response, &mut results);
    }
    Ok(results)
}

fn collect_matching_document_symbols(
    file_path: &str,
    query: &str,
    response: &DocumentSymbolResponse,
    results: &mut Vec<WorkspaceSymbolResult>,
) {
    let query = query.to_lowercase();
    for_each_document_symbol(response, &mut |name, kind, selection_range, container_name| {
        if symbol_name_matches(name, &query) {
            results.push(WorkspaceSymbolResult {
                name: name.to_string(),
                kind: symbol_kind_to_string(kind).to_string(),
                container_name: container_name.map(ToOwned::to_owned),
                source: WorkspaceSymbolSource::DocumentSymbolFallback,
                location: LocationResult::from_range(file_path.to_string(), selection_range),
            });
        }
    });
}

fn symbol_name_matches(name: &str, lowercase_query: &str) -> bool {
    name.to_lowercase().contains(lowercase_query)
}

#[cfg(test)]
mod tests {
    use aether_lspd::LANGUAGE_METADATA;

    use super::*;

    #[test]
    fn language_schema_matches_configured_languages() {
        for metadata in LANGUAGE_METADATA.iter().filter(|metadata| get_config_for_language(metadata.id).is_some()) {
            let value = serde_json::json!(metadata.id.as_str());
            assert!(serde_json::from_value::<LspWorkspaceSearchLanguage>(value).is_ok());
        }
    }
}
