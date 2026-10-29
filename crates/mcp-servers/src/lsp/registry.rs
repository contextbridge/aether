//! LSP Registry - holds one LSP daemon client per workspace
//!
//! # Architecture
//!
//! Agents connect to a shared daemon (`clankerdiff-lspd`) that manages LSP servers.
//! This avoids spawning duplicate LSP servers when running multiple agents
//! concurrently.

// `Uri` only uses interior mutability to cache parsed components; its identity is stable.
#![allow(clippy::mutable_key_type)]

use std::collections::HashMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use clankerdiff_lspd::{LanguageId, LspClient, WorkspaceKey, detect_project_languages, path_to_uri};
use futures::future::join_all;
use lsp_types::{Diagnostic, Uri};

use super::common::{find_document_symbol_line, find_symbol_column, find_symbol_line};
use super::error::LspError;
use crate::workspace_paths::{resolve_file, resolve_path};

/// A resolved symbol location with its LSP client, ready for protocol calls.
pub struct ResolvedSymbol {
    /// The file URI
    pub uri: Uri,
    /// 0-indexed line number (ready for LSP protocol)
    pub line: u32,
    /// 0-indexed column number
    pub column: u32,
    /// The LSP client for this file's language
    pub client: LspClient,
}

#[doc = include_str!("../docs/lsp_registry.md")]
pub struct LspRegistry {
    clients: HashMap<WorkspaceKey, OnceLock<LspClient>>,
    languages: HashMap<LanguageId, WorkspaceKey>,
    /// The project root directory
    root_path: PathBuf,
}

impl Debug for LspRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspRegistry").field("root_path", &self.root_path).finish_non_exhaustive()
    }
}

impl LspRegistry {
    /// Create a new registry for the given project root
    pub fn new(root_path: PathBuf) -> Self {
        let root_path = root_path.canonicalize().unwrap_or(root_path);
        let languages: HashMap<LanguageId, WorkspaceKey> =
            LanguageId::ALL.iter().map(|&language| (language, WorkspaceKey::resolve(&root_path, language))).collect();
        let clients = languages.values().map(|workspace| (workspace.clone(), OnceLock::new())).collect();
        Self { clients, languages, root_path }
    }

    /// Create a new registry and spawn LSP servers for detected project languages.
    ///
    /// This is a convenience constructor that wraps the registry in an `Arc`
    /// and kicks off background LSP spawning immediately.
    pub fn new_and_spawn(root_path: PathBuf) -> Arc<Self> {
        let registry = Arc::new(Self::new(root_path));
        let clone = Arc::clone(&registry);
        tokio::spawn(async move { clone.spawn_project_lsps().await });
        registry
    }

    /// Get the project root path
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn resolve_file(&self, raw: &str) -> Result<String, LspError> {
        resolve_file(&self.root_path, raw)
            .map(|path| path.to_string_lossy().to_string())
            .map_err(|error| LspError::InvalidPath(error.to_string()))
    }

    /// Connect to LSP daemon for all detected project languages.
    ///
    /// This scans the project root for manifest files (Cargo.toml, package.json, etc.)
    /// and connects to the LSP daemon for each detected language. Designed to be called
    /// at boot time so LSPs can start indexing immediately.
    pub async fn spawn_project_lsps(&self) {
        let languages = detect_project_languages(&self.root_path);
        let connections =
            languages.iter().map(|&language| async move { (language, self.client(language).connect().await) });

        for (language, result) in join_all(connections).await {
            match result {
                Ok(()) => tracing::info!("Connected to LSP daemon for {:?} based on project detection", language),
                Err(error) => tracing::warn!(%error, language = language.as_str(), "Failed to start project LSP"),
            }
        }
    }

    /// Resolve a symbol's position, convert its path to a URI, and select its LSP client.
    ///
    /// A matching 1-indexed line hint avoids document-symbol lookup. Missing or stale
    /// hints fall back to exact document symbols, then to a word-boundary text search.
    pub async fn resolve_symbol(
        &self,
        file_path: &str,
        symbol: &str,
        line_hint: Option<u32>,
    ) -> Result<ResolvedSymbol, LspError> {
        let content = tokio::fs::read_to_string(file_path).await?;
        let uri = path_to_uri(Path::new(file_path))?;
        let client = self.client_for(Path::new(file_path))?.clone();

        let hinted =
            line_hint.and_then(|line| find_symbol_column(&content, symbol, line).ok().map(|column| (line, column)));
        let (line, column) = if let Some(position) = hinted {
            position
        } else {
            let document_line = client
                .document_symbol(uri.clone())
                .await
                .ok()
                .and_then(|response| find_document_symbol_line(&response, symbol));
            let line = document_line
                .or_else(|| find_symbol_line(&content, symbol))
                .ok_or_else(|| LspError::SymbolNotFound(format!("Symbol '{symbol}' not found in '{file_path}'")))?;
            (line, find_symbol_column(&content, symbol, line)?)
        };

        Ok(ResolvedSymbol { uri, line: line - 1, column, client })
    }

    /// Collect diagnostics from LSP clients.
    ///
    /// If `file_path` is provided, queries only the LSP for that file and requests
    /// diagnostics for that specific document URI.
    /// If `file_path` is `None`, iterates every active client and returns all
    /// diagnostics grouped by document URI.
    pub async fn collect_diagnostics(
        &self,
        file_path: Option<&str>,
    ) -> Result<HashMap<Uri, Vec<Diagnostic>>, LspError> {
        if let Some(file_path) = file_path {
            return self.collect_file_diagnostics(file_path).await;
        }

        let clients = self.active_clients();
        if clients.is_empty() {
            return Err(LspError::ServerUnavailable(
                "No active LSP clients. Open a source file first so its language server can start.".to_string(),
            ));
        }

        let mut result: HashMap<Uri, Vec<Diagnostic>> = HashMap::new();
        for client in clients {
            match client.get_diagnostics(None).await {
                Ok(params_list) => merge_diagnostics(&mut result, params_list),
                Err(error) => tracing::warn!(%error, "Failed to collect diagnostics from LSP client"),
            }
        }
        Ok(result)
    }

    pub async fn queue_diagnostic_refresh(&self, file_path: &str) {
        let resolved_path = resolve_path(&self.root_path, PathBuf::from(file_path));

        let client = match self.client_for(&resolved_path) {
            Ok(client) => client,
            Err(error) => {
                tracing::debug!(path = %resolved_path.display(), %error, "No LSP for diagnostic refresh");
                return;
            }
        };

        let Ok(uri) = path_to_uri(&resolved_path) else {
            return;
        };

        if let Err(err) = client.queue_diagnostic_refresh(uri).await {
            tracing::debug!(
                path = %resolved_path.display(),
                %err,
                "Failed to queue diagnostic refresh"
            );
        }
    }

    async fn collect_file_diagnostics(&self, file_path: &str) -> Result<HashMap<Uri, Vec<Diagnostic>>, LspError> {
        let resolved_path = resolve_path(&self.root_path, PathBuf::from(file_path));
        let client = self.client_for(&resolved_path)?;
        let uri = path_to_uri(&resolved_path)?;
        let params_list = client.get_diagnostics(Some(uri)).await?;
        let mut result: HashMap<Uri, Vec<Diagnostic>> = HashMap::new();
        merge_diagnostics(&mut result, params_list);
        Ok(result)
    }

    fn client_for(&self, file_path: &Path) -> Result<&LspClient, LspError> {
        LanguageId::from_path(file_path)
            .map(|language| self.client(language))
            .ok_or_else(|| LspError::UnsupportedLanguage(file_path.display().to_string()))
    }

    /// Clients are created on first use because `LspClient::new` spawns a tokio task.
    fn client(&self, language: LanguageId) -> &LspClient {
        let workspace = &self.languages[&language];
        self.clients[workspace].get_or_init(|| LspClient::new(workspace.clone()))
    }

    fn active_clients(&self) -> Vec<&LspClient> {
        self.clients.values().filter_map(OnceLock::get).filter(|client| client.is_connected()).collect()
    }
}

fn merge_diagnostics(
    result: &mut HashMap<Uri, Vec<Diagnostic>>,
    params_list: Vec<lsp_types::PublishDiagnosticsParams>,
) {
    for params in params_list {
        result.entry(params.uri).or_default().extend(params.diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_arguments_resolve_against_the_project_root() {
        let temp = tempfile::tempdir().expect("temp dir");
        let registry = LspRegistry::new(temp.path().to_path_buf());

        let expected = registry.root_path().join("src/main.rs");
        assert_eq!(registry.resolve_file("src/main.rs").unwrap(), expected.to_string_lossy());
        assert_eq!(registry.resolve_file("/elsewhere/lib.rs").unwrap(), "/elsewhere/lib.rs");
        assert!(registry.resolve_file("  ").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn root_path_resolves_symlinks_to_match_language_server_paths() {
        use std::fs::create_dir;
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp dir");
        let project = temp.path().join("project");
        create_dir(&project).expect("create project dir");
        let link = temp.path().join("link");
        symlink(&project, &link).expect("create symlink");

        let registry = LspRegistry::new(link);

        assert_eq!(registry.root_path(), project.canonicalize().expect("canonical project dir"));
    }
}
