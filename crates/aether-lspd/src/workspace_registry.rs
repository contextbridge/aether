use crate::error::{DaemonError, DaemonResult};
use crate::language_catalog::LanguageId;
use crate::language_catalog::{ServerKind, metadata_for, resolved_config_for_language, server_kind_for_language};
use crate::workspace_session::WorkspaceSession;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct WorkspaceKey {
    pub(crate) workspace_root: PathBuf,
    pub(crate) server_kind: ServerKind,
}

#[derive(Clone, Default)]
pub(crate) struct WorkspaceRegistry {
    sessions: Arc<RwLock<HashMap<WorkspaceKey, Arc<WorkspaceSession>>>>,
}

impl WorkspaceRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn get_or_spawn(
        &self,
        workspace_root: &Path,
        language: LanguageId,
    ) -> DaemonResult<Arc<WorkspaceSession>> {
        let key = WorkspaceKey::new(workspace_root, language)?;

        if let Some(session) = self.sessions.read().await.get(&key) {
            return Ok(Arc::clone(session));
        }

        let config = resolved_config_for_language(language)
            .ok_or_else(|| DaemonError::LspSpawnFailed(format!("No LSP configured for language: {language:?}")))?;

        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get(&key) {
            return Ok(Arc::clone(session));
        }

        let session = Arc::new(WorkspaceSession::spawn(
            &key.workspace_root,
            &config.command,
            &config.args,
            supported_extensions(&config),
        )?);
        sessions.insert(key, Arc::clone(&session));
        Ok(session)
    }

    pub(crate) async fn workspace_roots(&self) -> Vec<PathBuf> {
        self.sessions.read().await.keys().map(|key| key.workspace_root.clone()).collect()
    }

    pub(crate) async fn shutdown(&self) {
        let sessions: Vec<_> = self.sessions.read().await.values().cloned().collect();
        futures::future::join_all(sessions.iter().map(|s| s.shutdown())).await;
        self.sessions.write().await.clear();
    }
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

impl WorkspaceKey {
    pub(crate) fn new(workspace_root: &Path, language: LanguageId) -> DaemonResult<Self> {
        let workspace_root = workspace_root.canonicalize().unwrap_or_else(|_| workspace_root.to_path_buf());
        let server_kind = server_kind_for_language(language)
            .ok_or_else(|| DaemonError::LspSpawnFailed(format!("No LSP configured for language: {language:?}")))?;
        Ok(Self { workspace_root, server_kind })
    }
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
