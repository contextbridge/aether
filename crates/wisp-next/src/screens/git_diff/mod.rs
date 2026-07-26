mod input;
mod rendering;
mod state;
mod tasks;

pub use state::GitDiffScreen;
pub use tasks::{GitDiffEvent, GitDiffTask};

use crate::surface::Action;

/// Wraps a task as the single action its handler returns.
fn task(task: GitDiffTask) -> Vec<Action> {
    vec![Action::Task(task.into())]
}
