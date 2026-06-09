mod report;
mod run_eval;
mod workspace;

pub(crate) use report::format_transcript;
pub use report::{DiffStats, EvalReport, GitDiff, ToolCall};
pub use run_eval::run_eval;
pub use workspace::{GitRepoSpec, Workspace, WorkspaceSource};
