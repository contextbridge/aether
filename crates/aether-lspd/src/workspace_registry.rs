use crate::error::{DaemonError, DaemonResult};
use crate::language_catalog::LanguageId;
use crate::language_catalog::{ServerKind, metadata_for, resolved_config_for_language, server_kind_for_language};
use crate::process_transport::TransportError;
use crate::protocol::{LSP_REQUEST_TIMED_OUT, LSP_TRANSPORT_CLOSED, LspErrorResponse, extract_document_uri};
use crate::workspace_session::WorkspaceSession;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct WorkspaceKey {
    pub(crate) workspace_root: PathBuf,
    pub(crate) server_kind: ServerKind,
}

/// A client's resolved attachment to one workspace/language-server pair.
/// Carries everything needed to (re)spawn the server, so per-request lookups
/// never re-canonicalize paths or re-derive server configuration.
#[derive(Clone)]
pub(crate) struct WorkspaceBinding {
    key: WorkspaceKey,
    language: LanguageId,
}

#[derive(Clone)]
pub(crate) struct WorkspaceRegistry {
    sessions: Arc<RwLock<HashMap<WorkspaceKey, Arc<WorkspaceSession>>>>,
    request_timeout: Duration,
}

impl WorkspaceRegistry {
    pub(crate) fn new(request_timeout: Duration) -> Self {
        Self { sessions: Arc::new(RwLock::new(HashMap::new())), request_timeout }
    }

    /// Resolve a workspace/language pair and spawn its language server if needed.
    pub(crate) async fn bind(&self, workspace_root: &Path, language: LanguageId) -> DaemonResult<WorkspaceBinding> {
        let key = WorkspaceKey::new(workspace_root, language)?;
        let binding = WorkspaceBinding { key, language };
        self.get_or_spawn(&binding).await?;
        Ok(binding)
    }

    /// Run an LSP call against the binding's current session. If the session's
    /// transport turns out to be closed (the language server exited), replace
    /// the session and retry once against the fresh server. On timeout the
    /// session is declared wedged: its server process is killed so the next
    /// request gets a fresh one.
    pub(crate) async fn lsp_call(
        &self,
        binding: &WorkspaceBinding,
        method: &str,
        params: Value,
    ) -> Result<Value, LspErrorResponse> {
        let session = self.session(binding).await?;
        let result = match self.call_session(&session, method, &params).await {
            Err(SessionCallError::TransportClosed) => {
                session.mark_dead();
                let session = self.session(binding).await?;
                self.call_session(&session, method, &params).await
            }
            result => result,
        };
        result.map_err(|err| err.into_response(method, self.request_timeout))
    }

    pub(crate) async fn get_diagnostics(
        &self,
        binding: &WorkspaceBinding,
        uri: Option<&lsp_types::Uri>,
    ) -> Result<Value, LspErrorResponse> {
        let session = self.session(binding).await?;
        let Ok(diagnostics) = tokio::time::timeout(self.request_timeout, session.get_diagnostics(uri)).await else {
            session.declare_wedged();
            return Err(SessionCallError::TimedOut.into_response("diagnostics", self.request_timeout));
        };
        serde_json::to_value(&diagnostics).map_err(|e| LspErrorResponse { code: -1, message: e.to_string() })
    }

    pub(crate) async fn queue_diagnostic_refresh(
        &self,
        binding: &WorkspaceBinding,
        uri: lsp_types::Uri,
    ) -> Result<(), LspErrorResponse> {
        let session = self.session(binding).await?;
        session.queue_diagnostic_refresh(uri).await;
        Ok(())
    }

    pub(crate) async fn workspace_roots(&self) -> Vec<PathBuf> {
        self.sessions.read().await.keys().map(|key| key.workspace_root.clone()).collect()
    }

    pub(crate) async fn shutdown(&self) {
        let sessions: Vec<_> = self.sessions.read().await.values().cloned().collect();
        futures::future::join_all(sessions.iter().map(|s| s.shutdown())).await;
        self.sessions.write().await.clear();
    }

    async fn session(&self, binding: &WorkspaceBinding) -> Result<Arc<WorkspaceSession>, LspErrorResponse> {
        self.get_or_spawn(binding).await.map_err(|e| LspErrorResponse { code: -1, message: e.to_string() })
    }

    async fn get_or_spawn(&self, binding: &WorkspaceBinding) -> DaemonResult<Arc<WorkspaceSession>> {
        if let Some(session) = self.sessions.read().await.get(&binding.key)
            && session.is_alive()
        {
            return Ok(Arc::clone(session));
        }

        let config = resolved_config_for_language(binding.language).ok_or_else(|| {
            DaemonError::LspSpawnFailed(format!("No LSP configured for language: {:?}", binding.language))
        })?;

        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get(&binding.key) {
            if session.is_alive() {
                return Ok(Arc::clone(session));
            }
            sessions.remove(&binding.key);
        }

        let session = Arc::new(WorkspaceSession::spawn(
            &binding.key.workspace_root,
            &config.command,
            &config.args,
            supported_extensions(&config),
        )?);
        sessions.insert(binding.key.clone(), Arc::clone(&session));
        Ok(session)
    }

    /// Run a single LSP call against one session, bounded by the request timeout.
    async fn call_session(
        &self,
        session: &WorkspaceSession,
        method: &str,
        params: &Value,
    ) -> Result<Value, SessionCallError> {
        let call = async {
            let opened_uri = if let Some(uri) = extract_document_uri(method, params) {
                let _ = session.ensure_document_open(&uri).await;
                Some(uri)
            } else {
                None
            };

            let result = request_with_retry(session, method, params, TRANSIENT_RETRY_LIMIT).await;

            if let Some(uri) = opened_uri {
                session.close_document(&uri).await;
            }
            result
        };

        let Ok(result) = tokio::time::timeout(self.request_timeout, call).await else {
            session.declare_wedged();
            return Err(SessionCallError::TimedOut);
        };
        result.map_err(SessionCallError::from)
    }
}

impl WorkspaceKey {
    pub(crate) fn new(workspace_root: &Path, language: LanguageId) -> DaemonResult<Self> {
        let workspace_root = workspace_root.canonicalize().unwrap_or_else(|_| workspace_root.to_path_buf());
        let server_kind = server_kind_for_language(language)
            .ok_or_else(|| DaemonError::LspSpawnFailed(format!("No LSP configured for language: {language:?}")))?;
        Ok(Self { workspace_root, server_kind })
    }
}

const LSP_CONTENT_MODIFIED: i32 = -32801;
const TRANSIENT_RETRY_LIMIT: u32 = 3;
const TRANSIENT_RETRY_DELAY: Duration = Duration::from_millis(500);

enum SessionCallError {
    Lsp(LspErrorResponse),
    TransportClosed,
    TimedOut,
}

impl SessionCallError {
    fn into_response(self, what: &str, request_timeout: Duration) -> LspErrorResponse {
        match self {
            Self::Lsp(err) => err,
            Self::TransportClosed => {
                LspErrorResponse { code: LSP_TRANSPORT_CLOSED, message: "LSP transport closed".into() }
            }
            Self::TimedOut => LspErrorResponse {
                code: LSP_REQUEST_TIMED_OUT,
                message: format!(
                    "LSP request '{what}' timed out after {}s; the language server was killed and will be replaced on the next request",
                    request_timeout.as_secs()
                ),
            },
        }
    }
}

impl From<TransportError> for SessionCallError {
    fn from(err: TransportError) -> Self {
        match err {
            TransportError::Lsp(err) => Self::Lsp(err),
            TransportError::Closed => Self::TransportClosed,
        }
    }
}

async fn request_with_retry(
    session: &WorkspaceSession,
    method: &str,
    params: &Value,
    max_retries: u32,
) -> Result<Value, TransportError> {
    let mut last_err = None;
    for attempt in 0..=max_retries {
        match session.request_raw(method, params.clone()).await {
            Ok(value) => return Ok(value),
            Err(TransportError::Lsp(err)) if err.code == LSP_CONTENT_MODIFIED && attempt < max_retries => {
                last_err = Some(TransportError::Lsp(err));
                tokio::time::sleep(TRANSIENT_RETRY_DELAY).await;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_err.unwrap())
}

fn supported_extensions(config: &crate::language_catalog::LspConfig) -> HashSet<String> {
    config
        .languages
        .iter()
        .filter_map(|language| metadata_for(*language))
        .flat_map(|metadata| metadata.extensions.iter().copied())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_server_languages_share_workspace_key() {
        let workspace = Path::new(".");
        let ts = WorkspaceKey::new(workspace, LanguageId::TypeScript).unwrap();
        let tsx = WorkspaceKey::new(workspace, LanguageId::TypeScriptReact).unwrap();
        let c = WorkspaceKey::new(workspace, LanguageId::C).unwrap();
        let cpp = WorkspaceKey::new(workspace, LanguageId::Cpp).unwrap();

        assert_eq!(ts, tsx);
        assert_eq!(c, cpp);
    }
}
