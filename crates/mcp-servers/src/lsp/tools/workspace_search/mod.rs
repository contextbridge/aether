//! LSP workspace symbol search tool
//!
//! Exposes the LSP `workspace/symbol` request as an MCP tool, enabling
//! workspace-wide symbol search without knowing the file path upfront.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::DocumentSymbolResponse;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

use crate::coding::tools::grep::common::OutputMode;
use crate::coding::tools::grep::{GrepInput, GrepOutput, perform_grep_excluding};
use crate::lsp::common::{LocationResult, enrich_locations, path_to_uri, uri_to_path};
use crate::lsp::registry::LspRegistry;
use aether_lspd::{LanguageId, LspClient, metadata_for, symbol_kind_to_string};

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
    /// LSP language identifier to query, such as "rust", "typescript", or "python".
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
) -> Result<LspWorkspaceSearchOutput, String> {
    if input.query.trim().is_empty() {
        return Err("query cannot be empty".to_string());
    }
    let language = LanguageId::from(input.language);
    let client = registry.get_or_spawn_for_language(language).await.map_err(|error| error.to_string())?;
    let symbols = client.workspace_symbol(input.query.clone()).await.map_err(|error| error.to_string())?;
    let mut all_results: Vec<_> = symbols
        .into_iter()
        .filter(|symbol| is_language_path(&uri_to_path(&symbol.location.uri), language))
        .map(|symbol| WorkspaceSymbolResult {
            name: symbol.name,
            kind: symbol_kind_to_string(symbol.kind).to_string(),
            container_name: symbol.container_name,
            source: WorkspaceSymbolSource::WorkspaceSymbol,
            location: LocationResult::from_location(&symbol.location),
        })
        .collect();

    if all_results.is_empty() {
        all_results = document_symbol_fallback(&input.query, language, registry.root_path(), &client).await?;
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

fn is_language_path(path: &str, language: LanguageId) -> bool {
    let path = Path::new(path);
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    metadata_for(language).is_some_and(|metadata| metadata.extensions.contains(&extension))
}

async fn document_symbol_fallback(
    query: &str,
    language: LanguageId,
    root: &Path,
    client: &Arc<LspClient>,
) -> Result<Vec<WorkspaceSymbolResult>, String> {
    let candidates = candidate_files(root, query, language).await?;
    let mut results = Vec::new();
    for path in candidates {
        let path_text = path.to_string_lossy().to_string();
        let uri = path_to_uri(&path).map_err(|error| error.to_string())?;
        let response = client.document_symbol(uri).await.map_err(|error| error.to_string())?;
        collect_matching_document_symbols(&path_text, query, response, &mut results);
    }
    Ok(results)
}

async fn candidate_files(root: &Path, query: &str, language: LanguageId) -> Result<Vec<PathBuf>, String> {
    let extensions =
        metadata_for(language).into_iter().flat_map(|metadata| metadata.extensions.iter().copied()).collect::<Vec<_>>();
    let glob = match extensions.as_slice() {
        [extension] => format!("*.{extension}"),
        extensions => format!("*.{{{}}}", extensions.join(",")),
    };
    let output = perform_grep_excluding(
        GrepInput {
            pattern: regex::escape(query),
            path: Some(root.to_string_lossy().into_owned()),
            glob: Some(glob),
            output_mode: Some(OutputMode::FilesWithMatches),
            case_insensitive: Some(true),
            ..GrepInput::default()
        },
        &[".git", ".pnpm", "node_modules", "target"],
    )
    .await
    .map_err(|error| error.to_string())?;

    match output {
        GrepOutput::Files(output) => Ok(output.files.into_iter().map(PathBuf::from).collect()),
        _ => unreachable!("files-with-matches grep returns file output"),
    }
}

fn collect_matching_document_symbols(
    file_path: &str,
    query: &str,
    response: DocumentSymbolResponse,
    results: &mut Vec<WorkspaceSymbolResult>,
) {
    match response {
        DocumentSymbolResponse::Flat(symbols) => {
            for symbol in symbols.into_iter().filter(|symbol| symbol_name_matches(&symbol.name, query)) {
                results.push(WorkspaceSymbolResult {
                    name: symbol.name,
                    kind: symbol_kind_to_string(symbol.kind).to_string(),
                    container_name: symbol.container_name,
                    source: WorkspaceSymbolSource::DocumentSymbolFallback,
                    location: LocationResult::from_location(&symbol.location),
                });
            }
        }
        DocumentSymbolResponse::Nested(symbols) => {
            for symbol in symbols {
                collect_nested_symbol(file_path, query, None, symbol, results);
            }
        }
    }
}

fn collect_nested_symbol(
    file_path: &str,
    query: &str,
    container_name: Option<&str>,
    symbol: lsp_types::DocumentSymbol,
    results: &mut Vec<WorkspaceSymbolResult>,
) {
    if symbol_name_matches(&symbol.name, query) {
        results.push(WorkspaceSymbolResult {
            name: symbol.name.clone(),
            kind: symbol_kind_to_string(symbol.kind).to_string(),
            container_name: container_name.map(ToOwned::to_owned),
            source: WorkspaceSymbolSource::DocumentSymbolFallback,
            location: LocationResult::from_range(file_path.to_string(), &symbol.selection_range),
        });
    }
    if let Some(children) = symbol.children {
        for child in children {
            collect_nested_symbol(file_path, query, Some(&symbol.name), child, results);
        }
    }
}

fn symbol_name_matches(name: &str, query: &str) -> bool {
    name.to_lowercase().contains(&query.to_lowercase())
}
