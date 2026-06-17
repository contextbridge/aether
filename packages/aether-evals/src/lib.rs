mod agents;
mod assertions;
mod error;
mod evals;
mod git_repo;
mod spec;

pub use agents::{
    AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV, AcpClientCommand, Agent, AgentCommandBuilder, AgentConfig,
    CONTAINER_AETHER_HOME, DockerAetherAgent, DockerAgent, DockerCommandConfig, DockerImage, DockerImageParseError,
    FakeAgent, ImageBuildError, RunError, aether_eval_env_vars,
};
pub use assertions::{assert_tool_call_count, assert_tool_call_with_args, assert_tool_called};
pub use error::{EvalRunError, WorkspaceError};
pub use evals::{DiffStats, EvalReport, GitDiff, GitRepoSpec, ToolCall, Workspace, WorkspaceSource, run_eval};
pub use git_repo::GitRepoError;
pub use spec::{
    EvalFileError, EvalFilesReport, EvalOutcome, EvalRunOptions, JudgeCriterionSummary, JudgeSummary, run_eval_files,
};
