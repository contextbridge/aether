mod agents;
mod assertions;
mod containers;
mod error;
mod evals;
mod git_repo;
mod judge;

pub use aether_core::events::ContextUsage;
pub use agents::{
    AETHER_EVAL_CWD_ENV, AETHER_EVAL_TASK_PROMPT_ENV, AETHER_EVAL_WORKSPACE_ROOT_ENV, Agent, AgentRunResult,
    CONTAINER_AETHER_HOME, DockerAgent, FakeAgent, RunError, default_eval_env_vars,
};
pub use assertions::{assert_tool_call_count, assert_tool_call_with_args, assert_tool_called};
pub use containers::{Container, ContainerBuilder, ContainerError, ExecOutput, Image, ImageParseError};
pub use error::{EvalRunError, WorkspaceError};
pub use evals::{
    DiffStats, GitBundleSpec, GitDiff, GitRepoSpec, RetainedWorkspaceInfo, Task, ToolCall, Transcript, TranscriptError,
    Workspace, WorkspaceSource, create_git_bundle,
};
pub use git_repo::GitRepoError;
pub use judge::{
    Judge, JudgeBuilder, JudgeContext, JudgeCriterionResponse, JudgeCriterionSpec, JudgeCriterionSummary, JudgeError,
    JudgeRubricResponse, JudgeSummary, judge,
};
