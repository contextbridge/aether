use super::agent_eval_message::AgentEvalMessage;
use std::future::Future;
use std::path::Path;
use thiserror::Error;
use tokio::sync::mpsc::Sender;

/// Configuration for running an agent on a specific task
pub struct AgentConfig<'a> {
    pub workspace: &'a Path,
    pub task_prompt: &'a str,
}

/// Trait for an agent under evaluation.
///
/// Implementors are responsible for:
/// - Creating their own MCP connections (if needed)
/// - Running the agent with the provided configuration
/// - Sending `AgentEvalMessage`s to the provided channel
/// - Sending `AgentEvalMessage::Done` when the agent finishes
///
/// # Example
///
/// ```ignore
/// struct MyAgent;
///
/// impl Agent for MyAgent {
///     async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentEvalMessage>) -> Result<(), RunError> {
///         tx.send(AgentEvalMessage::AgentText("Hello".to_string())).await
///             .map_err(|e| RunError::ChannelSendFailed(e.to_string()))?;
///         tx.send(AgentEvalMessage::Done).await
///             .map_err(|e| RunError::ChannelSendFailed(e.to_string()))?;
///         Ok(())
///     }
/// }
/// ```
pub trait Agent: Send + Sync {
    fn run(
        &self,
        config: AgentConfig<'_>,
        tx: Sender<AgentEvalMessage>,
    ) -> impl Future<Output = Result<(), RunError>> + Send;
}

/// Errors that can occur when running an agent
#[derive(Debug, Error)]
pub enum RunError {
    #[error("Agent execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Failed to send event: {0}")]
    ChannelSendFailed(String),

    #[error("Agent configuration error: {0}")]
    ConfigurationError(String),
}
