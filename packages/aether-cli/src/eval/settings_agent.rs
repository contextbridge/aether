use crate::runtime::RuntimeBuilder;
use aether_auth::OAuthCredentialStorage;
use aether_core::agent_spec::{AgentSpec, McpConfigSource};
use aether_core::events::{AgentMessage, Command};
use crucible::{Agent, AgentConfig, AgentEvalMessage, RunError};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// Drives the real settings-configured agent runtime under a Crucible eval.
///
/// Implements Crucible's [`Agent`] trait by building the same [`RuntimeBuilder`]
/// that `aether headless` uses — so the eval exercises the user's actual model,
/// prompts, MCP servers, and tool filters — then translates the runtime's
/// [`AgentMessage`] stream into [`AgentEvalMessage`]s.
pub struct SettingsAgent {
    spec: AgentSpec,
    mcp_config_sources: Vec<McpConfigSource>,
    oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
}

impl SettingsAgent {
    pub fn new(
        spec: AgentSpec,
        mcp_config_sources: Vec<McpConfigSource>,
        oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    ) -> Self {
        Self { spec, mcp_config_sources, oauth_credential_store }
    }
}

impl Agent for SettingsAgent {
    async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentEvalMessage>) -> Result<(), RunError> {
        // `build_ready` blocks until MCP servers connect and pre-seeds the agent
        // with their tools, so the eval's single turn can use tools immediately
        // (plain `build` attaches tools asynchronously and races the first turn).
        let (runtime, _details) = RuntimeBuilder::from_spec(config.workspace.to_path_buf(), self.spec.clone())
            .mcp_sources(self.mcp_config_sources.clone())
            .oauth_credential_store(Arc::clone(&self.oauth_credential_store))
            .build_ready(Vec::new())
            .await
            .map_err(|error| RunError::ExecutionFailed(error.to_string()))?;

        runtime
            .agent_tx
            .send(Command::text(config.task_prompt))
            .await
            .map_err(|error| RunError::ExecutionFailed(format!("failed to send prompt: {error}")))?;

        let mut agent_rx = runtime.agent_rx;
        while let Some(message) = agent_rx.recv().await {
            if matches!(message, AgentMessage::Done) {
                send(&tx, AgentEvalMessage::Done).await?;
                break;
            }
            if let Some(translated) = translate(message) {
                send(&tx, translated).await?;
            }
        }

        runtime.agent_handle.abort();
        runtime.mcp_handle.abort();
        Ok(())
    }
}

fn translate(message: AgentMessage) -> Option<AgentEvalMessage> {
    match message {
        AgentMessage::Text { chunk, is_complete: true, .. } => Some(AgentEvalMessage::AgentText(chunk)),
        AgentMessage::ToolCall { request, .. } => {
            Some(AgentEvalMessage::ToolCall { name: request.name, arguments: request.arguments })
        }
        AgentMessage::ToolResult { result, .. } => {
            Some(AgentEvalMessage::ToolResult { name: result.name, result: result.result })
        }
        AgentMessage::ToolError { error, .. } => {
            Some(AgentEvalMessage::ToolError(format!("[{}] {}", error.name, error.error)))
        }
        AgentMessage::Error { message } => Some(AgentEvalMessage::Error(message)),
        _ => None,
    }
}

async fn send(tx: &Sender<AgentEvalMessage>, message: AgentEvalMessage) -> Result<(), RunError> {
    tx.send(message).await.map_err(|error| RunError::ChannelSendFailed(error.to_string()))
}
