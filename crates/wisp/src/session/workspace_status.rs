use std::path::Path;

use acp_utils::notifications::WorkspaceStatusResponse;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceAccess {
    #[default]
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub display_dir: String,
    pub git_ref: Option<String>,
}

impl WorkspaceStatus {
    pub fn new(display_dir: impl Into<String>, git_ref: Option<String>) -> Self {
        Self { display_dir: display_dir.into(), git_ref }
    }

    pub fn initial(cwd: &Path) -> Self {
        Self::new(cwd.display().to_string(), None)
    }
}

impl From<WorkspaceStatusResponse> for WorkspaceStatus {
    fn from(response: WorkspaceStatusResponse) -> Self {
        Self::new(response.display_dir, response.git_ref)
    }
}
