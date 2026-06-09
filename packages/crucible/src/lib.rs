pub mod agents;
mod assertions;
mod error;
pub mod evals;
pub mod git_repo;
pub mod metrics;

pub use aether_core::core::PromptSource;
pub use aether_project::{AetherSettings, AgentConfig as AetherAgentConfig, McpSourceSpec};
pub use agents::{Agent, AgentConfig, AgentEvalMessage, DockerAetherAgent, DockerImage, FakeAgent, RunError};
pub use assertions::{assert_tool_call_count, assert_tool_call_with_args, assert_tool_called};
pub use error::{EvalRunError, WorkspaceError};
pub use evals::{
    DiffStats, EvalReport, GitDiff, GitRepoSpec, JudgeError, JudgeResult, LlmJudgeContext, ToolCall, Workspace,
    WorkspaceSource, run_eval,
};
pub use mcp_utils::client::McpServerConfig;
pub use metrics::{BinaryMetric, EvalMetric, NumericMetric};
