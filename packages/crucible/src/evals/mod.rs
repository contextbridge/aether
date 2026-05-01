mod eval;
mod judge;
mod report;

pub use eval::{GitRepoSpec, Workspace, WorkspaceSource, run_eval};
pub use judge::{JudgeError, JudgeResult, LlmJudgeContext};
pub use report::{DiffStats, EvalReport, GitDiff, ToolCall};
