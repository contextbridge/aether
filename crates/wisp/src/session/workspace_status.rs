use std::path::Path;

pub use acp_utils::notifications::WorkspaceStatusResponse as WorkspaceStatus;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceAccess {
    #[default]
    Local,
    Remote,
}

pub fn initial_workspace_status(cwd: &Path) -> WorkspaceStatus {
    WorkspaceStatus { display_dir: cwd.display().to_string(), git_ref: None }
}
