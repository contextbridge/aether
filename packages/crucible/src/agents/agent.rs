use super::agent_eval_message::AgentEvalMessage;
use super::docker::DockerError;
use std::future::Future;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::sync::mpsc::Sender;

/// Configuration for running an agent on a specific task
pub struct AgentConfig<'a> {
    pub workspace_root: &'a Path,
    pub relative_cwd: Option<&'a Path>,
    pub task_prompt: &'a str,
}

impl AgentConfig<'_> {
    /// The working directory the agent should operate in
    pub fn workspace(&self) -> PathBuf {
        match self.relative_cwd {
            Some(relative) => self.workspace_root.join(relative),
            None => self.workspace_root.to_path_buf(),
        }
    }
}

/// Wrap a task prompt with the standard eval prompt
pub(crate) fn build_task_prompt(task_prompt: &str, workspace: &Path) -> String {
    [
        "Complete the following task:".to_string(),
        format!("<task>{task_prompt}</task>"),
        format!(
            "CRITICAL INSTRUCTIONS: when working on this task, you MUST only operate within this directory: {}",
            workspace.display()
        ),
    ]
    .join("\n")
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

    #[error(transparent)]
    Docker(#[from] DockerError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_task_prompt_wraps_prompt_and_pins_workspace() {
        let prompt = build_task_prompt("write hello.txt", Path::new("/tmp/work"));

        assert!(prompt.contains("<task>write hello.txt</task>"));
        assert!(prompt.contains("/tmp/work"));
    }
}
