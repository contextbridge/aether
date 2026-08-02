mod document;
mod input;
mod rendering;
mod state;
mod tasks;

pub use document::{
    CommentContext, DiffScope, FileDiff, FileStatus, GitDiffDocument, GitDiffError, Hunk, PatchAnchor, PatchLine,
    PatchLineKind, QueuedComment, ReviewQueue, StageState, commit, discard_file, stage_all, stage_files, unstage_all,
    unstage_files,
};
pub use state::GitDiffScreen;
pub use tasks::{GitDiffEvent, GitDiffTask};

use crate::surfaces::surface::Action;

/// Wraps a task as the single action its handler returns.
fn task(task: GitDiffTask) -> Vec<Action> {
    vec![Action::Task(task.into())]
}
