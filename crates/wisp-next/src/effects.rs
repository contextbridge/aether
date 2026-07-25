//! Work the UI hands off rather than doing on the event loop, and the results
//! that come back.
//!
//! The event loop drains effects each frame, runs them on the tokio runtime,
//! and feeds the result back to `App` on the next turn of the loop.

use crate::picker::{FileEntry, index_files};
use crate::screens::git_diff::{GitDiffEffect, GitDiffEvent};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub enum Effect {
    GitDiff(GitDiffEffect),
    /// Walk `root` for the `@` file-completion index. Large repositories take
    /// long enough that doing this inline visibly stalls the keystroke.
    IndexFiles {
        request_id: u64,
        root: PathBuf,
    },
}

pub enum EffectResult {
    GitDiff(GitDiffEvent),
    FilesIndexed { request_id: u64, files: Vec<FileEntry> },
}

impl Effect {
    pub async fn execute(self) -> EffectResult {
        match self {
            Self::GitDiff(effect) => EffectResult::GitDiff(effect.execute().await),
            Self::IndexFiles { request_id, root } => {
                // A filesystem walk is blocking work, so it belongs on the pool
                // that exists for it rather than on an async worker.
                let files = tokio::task::spawn_blocking(move || index_files(&root)).await.unwrap_or_default();
                EffectResult::FilesIndexed { request_id, files }
            }
        }
    }
}

impl From<GitDiffEffect> for Effect {
    fn from(effect: GitDiffEffect) -> Self {
        Self::GitDiff(effect)
    }
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// A fresh id, so a result that arrives after the request was superseded can be
/// recognised and dropped.
pub fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}
