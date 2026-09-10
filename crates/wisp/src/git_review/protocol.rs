use crate::request::RequestId;
use clankerdiff_watch::RepositoryState;
use std::sync::Arc;

pub use clankerdiff_git::GitError as GitDiffError;
pub use clankerdiff_watch::WatchError as GitWatchError;

pub type GitWatchResult = Result<RepositoryState, Arc<GitWatchError>>;

#[derive(Debug, Clone)]
pub struct GitWatchEvent {
    pub review_id: RequestId,
    pub result: GitWatchResult,
}

#[derive(Debug)]
pub struct GitDiffEvent {
    pub review_id: RequestId,
    pub result: Result<(), Arc<GitDiffError>>,
}
