//! LSP Registry - manages LSP daemon clients with lazy connection
//!
//! The registry lazily connects to the LSP daemon on first access for each language.
//! LSP server configurations are managed by the daemon (`aether-lspd`).
//!
//! # Architecture
//!
//! Agents connect to a shared daemon (`aether-lspd`) that manages LSP servers.
//! This avoids spawning duplicate LSP servers when running multiple agents
//! concurrently.

// `Uri` only uses interior mutability to cache parsed components; its identity is stable.
#![allow(clippy::mutable_key_type)]

use std::collections::HashMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_lspd::{
    ClientError, LANGUAGE_METADATA, LanguageId, LspClient, detect_project_languages, get_config_for_language,
    server_metadata_for_language, socket_path,
};
use futures::future::join_all;
use lsp_types::{Diagnostic, Uri};
use tokio::sync::RwLock;

/// A resolved symbol location with its LSP client, ready for protocol calls.
pub struct ResolvedSymbol {
    /// The file URI
    pub uri: Uri,
    /// 0-indexed line number (ready for LSP protocol)
    pub line: u32,
    /// 0-indexed column number
    pub column: u32,
    /// The LSP client for this file's language
    pub client: Arc<LspClient>,
}

use super::common::{find_document_symbol_line, find_symbol_column, find_symbol_line, path_to_uri};
use super::error::LspError;

#[doc = include_str!("../docs/lsp_registry.md")]
pub struct LspRegistry {
    /// LSP daemon client slots keyed by the daemon socket path they share.
    clients: HashMap<PathBuf, RwLock<Option<Arc<LspClient>>>>,
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
    ///
    /// LSP server configurations are managed by the daemon.
    pub fn new(root_path: PathBuf) -> Self {
        let clients = LANGUAGE_METADATA
            .iter()
            .filter(|metadata| get_config_for_language(metadata.id).is_some())
            .map(|metadata| (socket_path(&root_path, metadata.id), RwLock::new(None)))
            .collect();
        Self { clients, root_path }
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

    /// Get or connect to the LSP daemon client for a file path.
    pub async fn get_or_spawn(&self, file_path: &Path) -> Result<Arc<LspClient>, LspError> {
        let language_id = LanguageId::from_path(file_path);
        self.get_or_spawn_for_language(language_id).await
    }

    /// Get or connect to the LSP daemon client for a specific language.
    pub async fn get_or_spawn_for_language(&self, language_id: LanguageId) -> Result<Arc<LspClient>, LspError> {
        let slot = self.slot(language_id)?;
        if let Some(client) = connected(slot).await {
            return Ok(client);
        }

        let mut client = slot.write().await;
        if let Some(connected) = client.as_ref().filter(|client| client.is_connected()) {
            return Ok(Arc::clone(connected));
        }

        let connected = LspClient::connect(&self.root_path, language_id)
            .await
            .map(Arc::new)
            .map_err(|error| connection_error(language_id, error))
            .inspect_err(
                |error| tracing::error!(%error, language = language_id.as_str(), "Failed to connect to LSP daemon"),
            )?;
        *client = Some(Arc::clone(&connected));
        Ok(connected)
    }

    /// Get the LSP daemon client for a specific language, if already connected
    pub async fn get_client_for_language(&self, language_id: LanguageId) -> Option<Arc<LspClient>> {
        connected(self.slot(language_id).ok()?).await
    }

    /// Get all active LSP daemon clients
    pub async fn active_clients(&self) -> Vec<Arc<LspClient>> {
        let mut active = Vec::new();
        for slot in self.clients.values() {
            if let Some(client) = connected(slot).await {
                active.push(client);
            }
        }
        active
    }

    /// Check if an LSP is configured for a given file path
    ///
    /// This checks the daemon's configuration registry.
    pub fn has_config_for(&self, file_path: &Path) -> bool {
        let language_id = LanguageId::from_path(file_path);
        get_config_for_language(language_id).is_some()
    }

    /// Connect to LSP daemon for all detected project languages.
    ///
    /// This scans the project root for manifest files (Cargo.toml, package.json, etc.)
    /// and connects to the LSP daemon for each detected language. Designed to be called
    /// at boot time so LSPs can start indexing immediately.
    pub async fn spawn_project_lsps(&self) {
        let languages = detect_project_languages(&self.root_path);
        let spawn_futures: Vec<_> =
            languages.iter().map(|&lang| async move { (lang, self.get_or_spawn_for_language(lang).await) }).collect();

        for (lang, result) in join_all(spawn_futures).await {
            match result {
                Ok(_) => tracing::info!("Connected to LSP daemon for {:?} based on project detection", lang),
                Err(error) => tracing::warn!(%error, language = lang.as_str(), "Failed to start project LSP"),
            }
        }
    }

    fn slot(&self, language_id: LanguageId) -> Result<&RwLock<Option<Arc<LspClient>>>, LspError> {
        self.clients
            .get(&socket_path(&self.root_path, language_id))
            .ok_or_else(|| LspError::UnsupportedLanguage(language_id.as_str().to_string()))
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
        let uri = path_to_uri(Path::new(file_path)).map_err(|error| LspError::InvalidPath(error.to_string()))?;
        let client = self.get_or_spawn(Path::new(file_path)).await?;

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

        let clients = self.active_clients().await;
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
        let resolved_path = self.resolve_path(file_path);

        let client = match self.get_or_spawn(&resolved_path).await {
            Ok(client) => client,
            Err(error) => {
                tracing::debug!(path = %resolved_path.display(), %error, "Failed to start LSP for diagnostic refresh");
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
        let resolved_path = self.resolve_path(file_path);
        let client = self.get_or_spawn(&resolved_path).await?;
        let uri = path_to_uri(&resolved_path).map_err(|error| LspError::InvalidPath(error.to_string()))?;
        let params_list = client.get_diagnostics(Some(uri)).await?;
        let mut result: HashMap<Uri, Vec<Diagnostic>> = HashMap::new();
        merge_diagnostics(&mut result, params_list);
        Ok(result)
    }

    fn resolve_path(&self, file_path: &str) -> PathBuf {
        if Path::new(file_path).is_absolute() { PathBuf::from(file_path) } else { self.root_path.join(file_path) }
    }
}

async fn connected(slot: &RwLock<Option<Arc<LspClient>>>) -> Option<Arc<LspClient>> {
    slot.read().await.as_ref().filter(|client| client.is_connected()).cloned()
}

fn connection_error(language_id: LanguageId, error: ClientError) -> LspError {
    if matches!(&error, ClientError::InitializationFailed(_))
        && let Some(metadata) = server_metadata_for_language(language_id)
        && let Some(instructions) = metadata.installation_instructions
    {
        return LspError::ServerUnavailable(format!(
            "{} could not start: {error}. {instructions}",
            metadata.display_name
        ));
    }

    LspError::Client(error)
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
    fn slot_uses_shared_socket_identity_for_typescript_family() {
        let registry = LspRegistry::new(PathBuf::from("/tmp"));

        assert!(std::ptr::eq(
            registry.slot(LanguageId::TypeScript).unwrap(),
            registry.slot(LanguageId::TypeScriptReact).unwrap()
        ));
    }

    #[test]
    fn slot_uses_shared_socket_identity_for_c_family() {
        let registry = LspRegistry::new(PathBuf::from("/tmp"));

        assert!(std::ptr::eq(registry.slot(LanguageId::C).unwrap(), registry.slot(LanguageId::Cpp).unwrap()));
    }

    #[test]
    fn test_has_config_for() {
        let registry = LspRegistry::new(PathBuf::from("/tmp"));

        assert!(registry.has_config_for(Path::new("foo.rs")));
        assert!(registry.has_config_for(Path::new("bar.ts")));
        assert!(registry.has_config_for(Path::new("baz.py")));
        assert!(!registry.has_config_for(Path::new("unknown.xyz")));
    }

    #[tokio::test]
    async fn test_no_clients_initially() {
        let registry = LspRegistry::new(PathBuf::from("/tmp"));

        assert!(registry.active_clients().await.is_empty());
    }
}
