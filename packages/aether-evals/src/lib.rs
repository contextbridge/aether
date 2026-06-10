mod agents;
mod assertions;
mod error;
mod evals;
mod git_repo;
mod spec;

pub use agents::{
    Agent, AgentConfig, DockerAetherAgent, DockerImage, DockerImageParseError, FakeAgent, ImageBuildError, RunError,
};
pub use assertions::{assert_tool_call_count, assert_tool_call_with_args, assert_tool_called};
pub use error::{EvalRunError, WorkspaceError};
pub use evals::{DiffStats, EvalReport, GitDiff, GitRepoSpec, ToolCall, Workspace, WorkspaceSource, run_eval};
pub use git_repo::GitRepoError;
pub use spec::{
    EvalFileError, EvalFilesReport, EvalOutcome, EvalRunOptions, JudgeCriterionSummary, JudgeSummary, run_eval_files,
};
