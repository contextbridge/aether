mod input;
mod rendering;
mod state;

pub use state::GitDiffScreen;

use crate::command::GitCommand;
use crate::surfaces::input::GitReviewOutput;

/// Wraps a task as the single action its handler returns.
fn task(task: GitCommand) -> Vec<GitReviewOutput> {
    vec![GitReviewOutput::Task(task)]
}
