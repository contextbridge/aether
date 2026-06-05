use crate::agents::RunError;
use crate::evals::JudgeError;
use crate::git_repo::GitRepoError;
use std::path::PathBuf;
use thiserror::Error;

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

    #[error("git workspace setup failed: {0}")]
    Git(#[from] GitRepoError),

    #[error("git workspace subdirectory does not exist: {}", path.display())]
    MissingSubdir { path: PathBuf },
}

#[derive(Debug, Error)]
pub enum EvalRunError {
    #[error("agent run failed: {0}")]
    Agent(#[from] RunError),

    #[error("workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
}

#[derive(Debug, Error)]
pub enum EvalSpecError {
    #[error("invalid eval spec JSON: {0}")]
    Json(#[source] serde_json::Error),

    #[error("workspace error: {0}")]
    Workspace(#[from] WorkspaceError),

    #[error("eval run error: {0}")]
    Run(#[from] EvalRunError),

    #[error("judge error: {0}")]
    Judge(#[from] JudgeError),

    #[error("failed to read prompt file '{}': {source}", path.display())]
    PromptFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write fixture file '{}': {source}", path.display())]
    Fixture {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to run command '{cmd}': {source}")]
    Command {
        cmd: String,
        #[source]
        source: std::io::Error,
    },
}
