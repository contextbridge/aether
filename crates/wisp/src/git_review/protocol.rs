use crate::request::RequestId;
use clankerdiff_git::RepositorySnapshot;

#[derive(Debug, thiserror::Error)]
pub enum GitDiffError {
    #[error("Not a Git repository")]
    NotARepository,
    #[error("{stderr}")]
    CommandFailed { stderr: String },
    #[error(transparent)]
    Repository(#[from] clankerdiff_git::GitError),
    #[error(transparent)]
    Diff(#[from] clankerdiff_core::DiffError),
}

#[derive(Debug)]
pub enum GitDiffEvent {
    Loaded { request_id: RequestId, result: Result<RepositorySnapshot, GitDiffError> },
    ActionFinished { request_id: RequestId, result: Result<(), GitDiffError> },
}

impl GitDiffEvent {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Loaded { request_id, .. } | Self::ActionFinished { request_id, .. } => *request_id,
        }
    }
}
