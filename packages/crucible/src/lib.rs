pub mod agents;
mod assertions;
mod error;
pub mod eval_spec;
pub mod evals;
pub mod git_repo;
pub mod metrics;

pub use aether_core::core::Prompt;
pub use agents::{AetherAgent, Agent, AgentConfig, AgentEvalMessage, FakeAgent, RunError};
pub use assertions::{assert_tool_call_count, assert_tool_call_with_args, assert_tool_called};
pub use error::{EvalRunError, EvalSpecError, WorkspaceError};
pub use eval_spec::{
    CheckResult, EnvironmentSpec, EvalSpec, Expectation, GitSpec, MetricSpec, NumericThreshold, PromptSpec, SpecReport,
    WorkspaceSpec, run_spec,
};
pub use evals::{
    DiffStats, EvalReport, GitDiff, GitRepoSpec, JudgeError, JudgeResult, LlmJudgeContext, ToolCall, Workspace,
    WorkspaceSource, run_eval,
};
pub use metrics::{BinaryMetric, EvalMetric, NumericMetric};
