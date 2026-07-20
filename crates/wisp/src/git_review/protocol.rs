use super::{DiffDocument, GitDiffError};
use crate::request::RequestId;

#[derive(Debug)]
pub enum GitDiffEvent {
    Loaded { request_id: RequestId, result: Result<DiffDocument, GitDiffError> },
    ActionFinished { request_id: RequestId, result: Result<(), GitDiffError> },
    FullFileLoaded { request_id: RequestId, path: String, result: Result<String, GitDiffError> },
}

impl GitDiffEvent {
    /// The request this result belongs to, so superseded results can be dropped.
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Loaded { request_id, .. }
            | Self::ActionFinished { request_id, .. }
            | Self::FullFileLoaded { request_id, .. } => *request_id,
        }
    }
}
