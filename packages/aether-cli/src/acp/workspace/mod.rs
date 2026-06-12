pub(crate) mod clone;
pub(crate) mod git;
pub(crate) mod registry;
pub(crate) mod transfer;

use acp_utils::notifications::{self, InvalidWorkspaceName, WorkspaceDestination};
use clone::{WorkspaceCloneError, cow_clone_dir};
use registry::{ManagedWorkspace, WorkspaceRegistry, repo_identity};
use std::path::{Path, PathBuf};
use transfer::{WorkspaceTransferError, clean_working_changes, move_working_changes};

#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkspaceError {
    #[error("fork unavailable: no home directory found")]
    RegistryUnavailable,
    #[error("workspace task aborted")]
    TaskAborted,
    #[error(transparent)]
    Clone(#[from] WorkspaceCloneError),
    #[error(transparent)]
    Transfer(#[from] WorkspaceTransferError),
    #[error("failed to update workspace registry: {0}")]
    Registry(#[from] std::io::Error),
    #[error("{0}")]
    InvalidName(#[from] InvalidWorkspaceName),
}

#[allow(dead_code)]
pub(crate) struct WorkspaceManager {
    registry: Option<WorkspaceRegistry>,
}

impl WorkspaceManager {
    pub fn new() -> Self {
        Self { registry: WorkspaceRegistry::new() }
    }

    #[allow(dead_code)]
    pub fn from_registry_path(path: PathBuf) -> Self {
        Self { registry: Some(WorkspaceRegistry::from_path(path)) }
    }

    pub async fn fork_options(&self, cwd: &Path) -> Result<Vec<notifications::WorkspaceOption>, WorkspaceError> {
        let Some(registry) = &self.registry else {
            return Err(WorkspaceError::RegistryUnavailable);
        };

        let current_repo = repo_identity(cwd).await;
        registry.register(ManagedWorkspace::for_dir(cwd, current_repo.clone()))?;
        let Some(current_repo) = current_repo else {
            return Ok(Vec::new());
        };

        Ok(registry
            .list()
            .into_iter()
            .filter(|workspace| workspace.repo.as_deref() == Some(current_repo.as_str()) && workspace.path != cwd)
            .map(|w| notifications::WorkspaceOption {
                name: w.name,
                subtitle: utils::paths::home_relative_path(&w.path),
                path: w.path,
            })
            .collect())
    }

    pub async fn fork(&self, cwd: &Path, destination: &WorkspaceDestination) -> Result<PathBuf, WorkspaceError> {
        match destination {
            WorkspaceDestination::Existing { path } => {
                move_working_changes(cwd, path).await?;
                Ok(path.clone())
            }
            WorkspaceDestination::NewSibling { name } => {
                notifications::validate_workspace_name(name)?;
                let Some(parent) = cwd.parent() else {
                    return Err(WorkspaceCloneError::CloneFailed(
                        "current workspace has no parent directory".to_string(),
                    )
                    .into());
                };
                let dest = parent.join(name);
                cow_clone_dir(cwd, &dest).await?;
                if let Err(e) = clean_working_changes(cwd).await {
                    tracing::warn!("Failed to clean source workspace after clone: {e}");
                }
                if let Some(registry) = &self.registry {
                    registry.register(ManagedWorkspace::new(name.clone(), dest.clone(), repo_identity(&dest).await))?;
                }
                Ok(dest)
            }
        }
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}
