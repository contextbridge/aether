mod agents;
mod assertions;
mod error;
mod evals;
mod git_repo;
mod spec;

pub use agents::{
    AETHER_EVAL_CWD_ENV, AETHER_EVAL_WORKSPACE_ROOT_ENV, AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV, Agent, AgentConfig,
    CONTAINER_AETHER_HOME, DockerAgent, DockerImage, DockerImageParseError, FakeAgent, ImageBuildError, RunError,
    default_eval_env_vars,
};
pub use assertions::{assert_tool_call_count, assert_tool_call_with_args, assert_tool_called};
pub use error::{EvalRunError, WorkspaceError};
pub use evals::{
    DiffStats, GitDiff, GitRepoSpec, RetainedWorkspaceInfo, Task, TaskRun, TaskRunError, ToolCall, Transcript,
    Workspace, WorkspaceSource,
};
pub use git_repo::GitRepoError;
pub use spec::{
    AgentSpec, DockerSpec, EvalFileError, EvalFilesReport, EvalOutcome, EvalSpec, EvalSpecLoadOptions, EvalStreamEvent,
    EvalToolCall, Expect, JudgeCriterionSummary, JudgeRef, JudgeSpec, JudgeSummary, ResolvedEvalSpec, TaskSpec,
    ToolCallExpectation, WorkspaceRetention, WorkspaceSpec, run_eval_spec_streaming, run_eval_specs,
};
