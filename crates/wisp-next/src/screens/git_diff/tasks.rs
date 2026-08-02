use std::path::PathBuf;

use futures::FutureExt;

use crate::components::generation::Generation;
use crate::screens::git_diff::{
    DiffScope, FileStatus, GitDiffDocument, GitDiffError, commit, discard_file, stage_all, stage_files, unstage_all,
    unstage_files,
};

#[derive(Debug)]
pub enum GitDiffTask {
    Load { request_id: Generation, working_dir: PathBuf, repo_root: Option<PathBuf>, scope: DiffScope },
    StageFiles { request_id: Generation, repo_root: PathBuf, paths: Vec<String> },
    UnstageFiles { request_id: Generation, repo_root: PathBuf, paths: Vec<String> },
    StageAll { request_id: Generation, repo_root: PathBuf },
    UnstageAll { request_id: Generation, repo_root: PathBuf },
    Commit { request_id: Generation, repo_root: PathBuf, message: String },
    DiscardFile { request_id: Generation, repo_root: PathBuf, path: String, status: FileStatus },
    LoadFullFile { request_id: Generation, repo_root: PathBuf, path: String },
}

pub enum GitDiffEvent {
    Loaded { request_id: Generation, result: Result<GitDiffDocument, GitDiffError> },
    ActionFinished { request_id: Generation, result: Result<(), GitDiffError> },
    FullFileLoaded { request_id: Generation, path: String, result: Result<String, GitDiffError> },
}

impl GitDiffEvent {
    /// The request this result belongs to, so superseded results can be dropped.
    pub fn request_id(&self) -> Generation {
        match self {
            Self::Loaded { request_id, .. }
            | Self::ActionFinished { request_id, .. }
            | Self::FullFileLoaded { request_id, .. } => *request_id,
        }
    }
}

impl GitDiffTask {
    pub async fn execute(self) -> GitDiffEvent {
        let request_id = self.request_id();
        let result = std::panic::AssertUnwindSafe(self.execute_inner()).catch_unwind().await;
        match result {
            Ok(event) => event,
            Err(_panic) => GitDiffEvent::ActionFinished {
                request_id,
                result: Err(GitDiffError::CommandFailed { stderr: "Internal error".to_string() }),
            },
        }
    }

    /// The request this task will report back under.
    pub fn request_id(&self) -> Generation {
        match self {
            Self::Load { request_id, .. }
            | Self::StageFiles { request_id, .. }
            | Self::UnstageFiles { request_id, .. }
            | Self::StageAll { request_id, .. }
            | Self::UnstageAll { request_id, .. }
            | Self::Commit { request_id, .. }
            | Self::DiscardFile { request_id, .. }
            | Self::LoadFullFile { request_id, .. } => *request_id,
        }
    }

    async fn execute_inner(self) -> GitDiffEvent {
        match self {
            Self::Load { request_id, working_dir, repo_root, scope } => GitDiffEvent::Loaded {
                request_id,
                result: GitDiffDocument::load(&working_dir, repo_root.as_deref(), scope).await,
            },
            Self::StageFiles { request_id, repo_root, paths } => {
                GitDiffEvent::ActionFinished { request_id, result: stage_files(&repo_root, &paths).await }
            }
            Self::UnstageFiles { request_id, repo_root, paths } => {
                GitDiffEvent::ActionFinished { request_id, result: unstage_files(&repo_root, &paths).await }
            }
            Self::StageAll { request_id, repo_root } => {
                GitDiffEvent::ActionFinished { request_id, result: stage_all(&repo_root).await }
            }
            Self::UnstageAll { request_id, repo_root } => {
                GitDiffEvent::ActionFinished { request_id, result: unstage_all(&repo_root).await }
            }
            Self::Commit { request_id, repo_root, message } => {
                GitDiffEvent::ActionFinished { request_id, result: commit(&repo_root, &message).await }
            }
            Self::DiscardFile { request_id, repo_root, path, status } => {
                GitDiffEvent::ActionFinished { request_id, result: discard_file(&repo_root, &path, status).await }
            }
            Self::LoadFullFile { request_id, repo_root, path } => {
                let full_path = repo_root.join(&path);
                let result = tokio::fs::read_to_string(&full_path)
                    .await
                    .map_err(|error| GitDiffError::CommandFailed { stderr: format!("Cannot read {path}: {error}") });
                GitDiffEvent::FullFileLoaded { request_id, path, result }
            }
        }
    }
}
