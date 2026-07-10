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

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_lspd::{LanguageId, LspClient, get_config_for_language, socket_path};
use futures::future::join_all;
use lsp_types::Uri;
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

use lsp_types::Diagnostic;

use super::common::{find_symbol_column, path_to_uri};
use super::error::LspError;

#[doc = include_str!("../docs/lsp_registry.md")]
pub struct LspRegistry {
    /// Active daemon clients keyed by the daemon socket path they share.
    clients: RwLock<HashMap<PathBuf, Arc<LspClient>>>,
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
        Self { clients: RwLock::new(HashMap::new()), root_path }
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

    /// Get or connect to the LSP daemon client for a file path
    ///
    /// Returns None if no LSP is configured for this file type or if connection fails.
    pub async fn get_or_spawn(&self, file_path: &Path) -> Option<Arc<LspClient>> {
        let language_id = LanguageId::from_path(file_path);
        self.get_or_spawn_for_language(language_id).await
    }

    /// Get or connect to the LSP daemon client for a specific language
    ///
    /// Returns None if no LSP is configured for this language or if connection fails.
    pub async fn get_or_spawn_for_language(&self, language_id: LanguageId) -> Option<Arc<LspClient>> {
        let client_key = self.client_key(language_id)?;

        // Check if already connected
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&client_key) {
                return Some(Arc::clone(client));
            }
        }

        // Connect to daemon
        let mut clients = self.clients.write().await;

        // Double-check after acquiring write lock
        if let Some(client) = clients.get(&client_key) {
            return Some(Arc::clone(client));
        }

        match LspClient::connect(&self.root_path, language_id).await {
            Ok(client) => {
                let client = Arc::new(client);
                clients.insert(client_key, Arc::clone(&client));
                Some(client)
            }
            Err(e) => {
                tracing::error!("Failed to connect to LSP daemon for {:?}: {}", language_id, e);
                None
            }
        }
    }

    /// Get the LSP daemon client for a specific language, if already connected
    pub async fn get_client_for_language(&self, language_id: LanguageId) -> Option<Arc<LspClient>> {
        let client_key = self.client_key(language_id)?;
        let clients = self.clients.read().await;
        clients.get(&client_key).cloned()
    }

    /// Get all active LSP daemon clients
    pub async fn active_clients(&self) -> Vec<Arc<LspClient>> {
        let clients = self.clients.read().await;
        clients.values().cloned().collect()
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
        let languages = self.detect_project_languages();
        let spawn_futures: Vec<_> =
            languages.iter().map(|&lang| async move { (lang, self.get_or_spawn_for_language(lang).await) }).collect();

        for (lang, result) in join_all(spawn_futures).await {
            if result.is_some() {
                tracing::info!("Connected to LSP daemon for {:?} based on project detection", lang);
            }
        }
    }

    /// Detect project languages by checking for manifest files in the root directory.
    ///
    /// This is a fast, synchronous check that looks for common project files:
    /// - Cargo.toml → Rust
    /// - package.json → TypeScript/JavaScript
    /// - pyproject.toml / setup.py / requirements.txt → Python
    /// - go.mod → Go
    /// - CMakeLists.txt → C/C++
    fn detect_project_languages(&self) -> Vec<LanguageId> {
        let mappings = [
            (LanguageId::Rust, HashSet::from(["Cargo.toml"])),
            (LanguageId::TypeScript, HashSet::from(["package.json"])),
            (LanguageId::Go, HashSet::from(["go.mod"])),
            (LanguageId::Cpp, HashSet::from(["CMakeLists.txt"])),
            (LanguageId::Python, HashSet::from(["pyproject.toml", "setup.py", "requirements.txt"])),
        ];

        mappings
            .iter()
            .filter(|(_, files)| files.iter().any(|f| self.root_path.join(f).exists()))
            .map(|(lang, _)| *lang)
            .collect()
    }

    fn client_key(&self, language_id: LanguageId) -> Option<PathBuf> {
        get_config_for_language(language_id)?;
        Some(socket_path(&self.root_path, language_id))
    }

    /// Resolve a symbol's position, convert path to URI, and get the LSP client.
    ///
    /// Accepts a 1-indexed `line` (as provided by the user / document symbols) and
    /// returns a [`ResolvedSymbol`] with 0-indexed `line` and `column`, ready for
    /// LSP protocol calls.
    pub async fn resolve_symbol(&self, file_path: &str, symbol: &str, line: u32) -> Result<ResolvedSymbol, LspError> {
        let content = tokio::fs::read_to_string(file_path).await?;
        let column = find_symbol_column(&content, symbol, line)?;
        let uri = path_to_uri(Path::new(file_path)).map_err(|e| LspError::Transport(e.to_string()))?;
        let client = self.require_client(file_path).await?;
        Ok(ResolvedSymbol { uri, line: line - 1, column, client })
    }

    /// Collect diagnostics from LSP clients.
    ///
    /// If `file_path` is provided, queries only the LSP for that file and requests
    /// diagnostics for that specific document URI.
    /// If `file_path` is `None`, iterates every active client and returns all
    /// diagnostics grouped by file path.
    pub async fn collect_diagnostics(&self, file_path: Option<&str>) -> HashMap<String, Vec<Diagnostic>> {
        if let Some(file_path) = file_path {
            return self.collect_file_diagnostics(file_path).await;
        }

        let mut result: HashMap<String, Vec<Diagnostic>> = HashMap::new();
        for client in self.active_clients().await {
            if let Ok(params_list) = client.get_diagnostics(None).await {
                merge_diagnostics(&mut result, params_list);
            }
        }
        result
    }

    pub async fn queue_diagnostic_refresh(&self, file_path: &str) {
        let resolved_path = self.resolve_path(file_path);

        let Some(client) = self.get_or_spawn(&resolved_path).await else {
            return;
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

    async fn collect_file_diagnostics(&self, file_path: &str) -> HashMap<String, Vec<Diagnostic>> {
        let resolved_path = self.resolve_path(file_path);

        let Some(client) = self.get_or_spawn(&resolved_path).await else {
            return HashMap::new();
        };

        let Ok(uri) = path_to_uri(&resolved_path) else {
            return HashMap::new();
        };

        let mut result: HashMap<String, Vec<Diagnostic>> = HashMap::new();
        if let Ok(params_list) = client.get_diagnostics(Some(uri)).await {
            merge_diagnostics(&mut result, params_list);
        }
        result
    }

    fn resolve_path(&self, file_path: &str) -> PathBuf {
        if Path::new(file_path).is_absolute() { PathBuf::from(file_path) } else { self.root_path.join(file_path) }
    }

    /// Get the LSP client for a file, returning an error if none is configured.
    pub async fn require_client(&self, file_path: &str) -> Result<Arc<LspClient>, LspError> {
        self.get_or_spawn(Path::new(file_path))
            .await
            .ok_or_else(|| LspError::Transport("No LSP configured for this file type".to_string()))
    }
}

fn merge_diagnostics(
    result: &mut HashMap<String, Vec<Diagnostic>>,
    params_list: Vec<lsp_types::PublishDiagnosticsParams>,
) {
    for params in params_list {
        result.entry(params.uri.as_str().to_string()).or_default().extend(params.diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_key_uses_shared_socket_identity_for_typescript_family() {
        let registry = LspRegistry::new(PathBuf::from("/tmp"));

        assert_eq!(registry.client_key(LanguageId::TypeScript), registry.client_key(LanguageId::TypeScriptReact));
    }

    #[test]
    fn client_key_uses_shared_socket_identity_for_c_family() {
        let registry = LspRegistry::new(PathBuf::from("/tmp"));

        assert_eq!(registry.client_key(LanguageId::C), registry.client_key(LanguageId::Cpp));
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
