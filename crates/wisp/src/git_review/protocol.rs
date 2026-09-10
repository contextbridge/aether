use crate::request::RequestId;
use clankerdiff_git::RepositorySnapshot;

pub use clankerdiff_git::GitError as GitDiffError;

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
