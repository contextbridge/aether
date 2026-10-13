use crate::agents::RunError;
use std::path::PathBuf;
use thiserror::Error;
use utils::git::GitError;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("failed to create temporary directory: {0}")]
    CreateTempDir(#[source] std::io::Error),

    #[error("failed to copy fixture directory from '{}' to '{}': {source}", from.display(), to.display())]
    CopyFixture {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write workspace file '{}': {source}", path.display())]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("git workspace setup failed: {0}")]
    Git(#[from] GitError),

    #[error("git workspace subdirectory does not exist: {}", path.display())]
    MissingSubdir { path: PathBuf },

    #[error("git bundle file does not exist: {}", path.display())]
    MissingBundle { path: PathBuf },
}

#[derive(Debug, Error)]
pub enum EvalRunError {
    #[error("agent run failed: {0}")]
    Agent(#[from] RunError),

    #[error("workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
}
