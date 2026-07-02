use crate::{Task, containers::ContainerError};
use aether_core::events::AgentEvent;
use futures::Stream;
use thiserror::Error;

pub type AgentRunResult = Result<AgentEvent, RunError>;

/// Trait for an agent under evaluation.
///
/// Implementors run a [`Task`] in their configured execution environment and stream the observed
/// [`AgentEvent`]s.
///
/// # Example
///
/// ```ignore
/// let mut messages = agent.run(Task::new("write hello.txt"));
/// ```
pub trait Agent: Send + Sync {
    fn run(&self, task: Task) -> impl Stream<Item = AgentRunResult> + Send + '_;
}

/// Errors that can occur when running an agent
#[derive(Debug, Error)]
pub enum RunError {
    #[error("Agent execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Agent configuration error: {0}")]
    ConfigurationError(String),

    #[error("Agent command exited without emitting a terminal AgentEvent: {stderr}")]
    CommandExitWithoutTerminal { stderr: String },

    #[error("Agent command emitted invalid AgentEvent JSON line: {line}")]
    AgentEventJsonLine {
        line: String,
        #[source]
        source: serde_json::Error,
    },

    #[error(transparent)]
    Container(#[from] ContainerError),
}
