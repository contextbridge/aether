use super::clone::{WorkspaceCloneError, cow_clone_dir, validate_workspace_name};
use super::registry::{ManagedWorkspace, WorkspaceRegistry, repo_identity};
use super::status::WorkspaceStatus;
use super::transfer::{WorkspaceTransferError, clean_working_changes, move_working_changes};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct WorkspaceManager {
    registry: Option<WorkspaceRegistry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceOption {
    pub name: String,
    pub path: PathBuf,
    pub subtitle: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceDestination {
    Existing { path: PathBuf },
    NewSibling { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFork {
    pub cwd: PathBuf,
    pub status: WorkspaceStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
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
}

impl WorkspaceManager {
    pub fn new() -> Self {
        Self { registry: WorkspaceRegistry::new() }
    }

    #[cfg(test)]
    pub(crate) fn with_registry_path(path: PathBuf) -> Self {
        Self { registry: Some(WorkspaceRegistry::from_path(path)) }
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceManager {
    pub async fn workspace_status(&self, cwd: &Path) -> WorkspaceStatus {
        WorkspaceStatus::resolve(cwd).await
    }

    pub async fn fork_options(&self, cwd: &Path) -> Result<Vec<WorkspaceOption>, WorkspaceError> {
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
            .map(WorkspaceOption::from)
            .collect())
    }

    pub async fn fork(&self, cwd: &Path, destination: WorkspaceDestination) -> Result<WorkspaceFork, WorkspaceError> {
        let fork_cwd = match destination {
            WorkspaceDestination::Existing { path } => {
                move_working_changes(cwd, &path).await?;
                path
            }
            WorkspaceDestination::NewSibling { name } => {
                validate_workspace_name(&name)?;
                let Some(parent) = cwd.parent() else {
                    return Err(WorkspaceCloneError::CloneFailed(
                        "current workspace has no parent directory".to_string(),
                    )
                    .into());
                };
                let dest = parent.join(&name);
                cow_clone_dir(cwd, &dest).await?;
                if let Err(e) = clean_working_changes(cwd).await {
                    tracing::warn!("Failed to clean source workspace after clone: {e}");
                }
                if let Some(registry) = &self.registry {
                    registry.register(ManagedWorkspace::new(name, dest.clone(), repo_identity(&dest).await))?;
                }
                dest
            }
        };
        let status = WorkspaceStatus::resolve(&fork_cwd).await;
        Ok(WorkspaceFork { cwd: fork_cwd, status })
    }
}

impl From<ManagedWorkspace> for WorkspaceOption {
    fn from(workspace: ManagedWorkspace) -> Self {
        Self {
            name: workspace.name,
            subtitle: super::status::home_relative_path(&workspace.path),
            path: workspace.path,
        }
    }
}
