use super::{AgentEvalMessage, RunError};
use aether_core::events::AgentMessage;
use llm::ToolCallRequest;
use tokio::sync::mpsc::Sender;

pub(crate) struct AgentEvalMessageGroup {
    messages: Vec<AgentEvalMessage>,
    terminal: bool,
}

impl AgentEvalMessageGroup {
    pub(crate) async fn forward(self, tx: &Sender<AgentEvalMessage>) -> Result<bool, RunError> {
        let terminal = self.terminal;
        for message in self.messages {
            tx.send(message).await.map_err(|e| RunError::ChannelSendFailed(e.to_string()))?;
        }
        Ok(terminal)
    }
}

impl From<AgentMessage> for AgentEvalMessageGroup {
    fn from(message: AgentMessage) -> Self {
        match message {
            AgentMessage::Text { chunk, is_complete: true, .. } if !chunk.is_empty() => {
                Self::message(AgentEvalMessage::AgentText(chunk))
            }
            AgentMessage::ToolResult { result, .. } => Self::messages([
                AgentEvalMessage::ToolCall { name: result.name.clone(), arguments: result.arguments.clone() },
                AgentEvalMessage::ToolResult { name: result.name, result: result.result },
            ]),
            AgentMessage::ToolError { error, .. } => {
                let rendered_error = format!("{error:?}");
                Self::messages([
                    AgentEvalMessage::ToolCall { name: error.name, arguments: error.arguments.unwrap_or_default() },
                    AgentEvalMessage::ToolError(rendered_error),
                ])
            }
            AgentMessage::Error { message } => Self::terminal(AgentEvalMessage::Error(message)),
            AgentMessage::Cancelled { message } => {
                Self::terminal(AgentEvalMessage::Error(format!("Cancelled: {message}")))
            }
            AgentMessage::Done => Self::terminal(AgentEvalMessage::Done),
            _ => Self::empty(),
        }
    }
}

impl AgentEvalMessageGroup {
    fn empty() -> Self {
        Self { messages: Vec::new(), terminal: false }
    }

    fn message(message: AgentEvalMessage) -> Self {
        Self { messages: vec![message], terminal: false }
    }

    fn messages(messages: impl IntoIterator<Item = AgentEvalMessage>) -> Self {
        Self { messages: messages.into_iter().collect(), terminal: false }
    }

    fn terminal(message: AgentEvalMessage) -> Self {
        Self { messages: vec![message], terminal: true }
    }
}

pub(crate) fn log_agent_message(message: &AgentMessage) {
    match message {
        AgentMessage::Text { chunk, is_complete: true, .. } => log_response(chunk),
        AgentMessage::Text { chunk, is_complete: false, .. } => {
            tracing::debug!("Agent response chunk: {}", chunk);
        }
        AgentMessage::ToolCall { request, .. } => {
            tracing::debug!("Tool call started: {} ({})", request.name, request.id);
        }
        AgentMessage::ToolCallUpdate { tool_call_id, chunk, .. } => {
            tracing::debug!("Tool call update for {}: {}", tool_call_id, chunk);
        }
        AgentMessage::ToolResult { result, .. } => {
            tracing::debug!("Tool result for {}: {}", result.name, result.result);
        }
        AgentMessage::ToolError { error, .. } => {
            tracing::debug!("Tool error: {:?}", error);
        }
        AgentMessage::ToolProgress { request, progress, total, message } => {
            log_tool_progress(request, *progress, *total, message.as_ref());
        }
        AgentMessage::Error { message } => {
            tracing::debug!("Agent error: {}", message);
        }
        AgentMessage::Cancelled { message } => {
            tracing::debug!("Agent cancelled: {}", message);
        }
        AgentMessage::Done => {
            tracing::debug!("Agent done");
        }
        AgentMessage::ContextCompactionStarted { message_count } => {
            tracing::debug!("Context compaction started: {} messages", message_count);
        }
        AgentMessage::ContextCompactionResult { messages_removed, .. } => {
            tracing::debug!("Context compacted: {} messages removed", messages_removed);
        }
        AgentMessage::ContextUsageUpdate { usage_ratio, input_tokens, context_limit, .. } => {
            log_context_usage_update(*usage_ratio, *input_tokens, *context_limit);
        }
        AgentMessage::AutoContinue { attempt, max_attempts } => {
            tracing::debug!(
                "Auto-continuing: attempt {}/{} - LLM stopped with resumable stop reason",
                attempt,
                max_attempts
            );
        }
        AgentMessage::Retrying { attempt, max_attempts, delay_ms, error } => {
            tracing::debug!(
                "Retrying: attempt {}/{} in {}ms after transient error: {}",
                attempt,
                max_attempts,
                delay_ms,
                error
            );
        }
        AgentMessage::ModelSwitched { previous, new } => {
            tracing::debug!("Model switched: {} -> {}", previous, new);
        }
        AgentMessage::ContextCleared => {
            tracing::debug!("Agent context cleared");
        }
        AgentMessage::Thought { chunk, is_complete: false, .. } => {
            tracing::debug!("Agent thought: {}", chunk);
        }
        AgentMessage::Thought { is_complete: true, .. } => {}
    }
}

fn log_response(text: &str) {
    for line in text.lines() {
        tracing::debug!("Agent response: {}", line);
    }
}

fn log_tool_progress(request: &ToolCallRequest, progress: f64, total: Option<f64>, message: Option<&String>) {
    let msg = message.map(|m| format!("{m} ")).unwrap_or_default();
    let total_str = total.map(|t| format!("/{t}")).unwrap_or_default();
    tracing::debug!("Tool progress for {}: {}{}{}", request.name, msg, progress, total_str);
}

fn log_context_usage_update(usage_ratio: Option<f64>, input_tokens: u32, context_limit: Option<u32>) {
    match (usage_ratio, context_limit) {
        (Some(usage_ratio), Some(context_limit)) => {
            tracing::debug!("Context usage: {:.1}% ({}/{} tokens)", usage_ratio * 100.0, input_tokens, context_limit);
        }
        _ => {
            tracing::debug!("Context usage: unknown limit ({} tokens used)", input_tokens);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::{ToolCallError, ToolCallResult};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn maps_complete_text_without_accumulating_partial_chunks() {
        let partial = AgentEvalMessageGroup::from(AgentMessage::text("msg_1", "hel", false, "test"));
        let complete = AgentEvalMessageGroup::from(AgentMessage::text("msg_1", "hello", true, "test"));

        assert!(collect_messages(partial).await.is_empty());
        let messages = collect_messages(complete).await;
        assert!(matches!(&messages[0], AgentEvalMessage::AgentText(text) if text == "hello"));
    }

    #[tokio::test]
    async fn maps_done_as_terminal_message() {
        let (terminal, mut rx) = {
            let (tx, rx) = mpsc::channel(10);
            let terminal = AgentEvalMessageGroup::from(AgentMessage::Done).forward(&tx).await.unwrap();
            (terminal, rx)
        };

        assert!(terminal);
        let mut messages = Vec::new();
        while let Some(message) = rx.recv().await {
            messages.push(message);
        }
        assert!(matches!(messages.last(), Some(AgentEvalMessage::Done)));
    }

    #[tokio::test]
    async fn maps_tool_call_from_result_arguments() {
        let mapping = AgentEvalMessageGroup::from(AgentMessage::ToolResult {
            result: ToolCallResult {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: r#"["Cargo.toml"]"#.to_string(),
                result: "file contents".to_string(),
            },
            result_meta: None,
            model_name: "test".to_string(),
        });
        let messages = collect_messages(mapping).await;

        assert!(matches!(
            &messages[0],
            AgentEvalMessage::ToolCall { name, arguments }
                if name == "coding__read_file" && arguments == r#"["Cargo.toml"]"#
        ));

        assert!(matches!(
            &messages[1],
            AgentEvalMessage::ToolResult { name, result }
                if name == "coding__read_file" && result == "file contents"
        ));
    }

    #[tokio::test]
    async fn maps_tool_call_from_error_arguments() {
        let mapping = AgentEvalMessageGroup::from(AgentMessage::ToolError {
            error: ToolCallError {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: Some(r#"["Cargo.toml"]"#.to_string()),
                error: "boom".to_string(),
            },
            model_name: "test".to_string(),
        });
        let messages = collect_messages(mapping).await;

        assert!(matches!(
            &messages[0],
            AgentEvalMessage::ToolCall { name, arguments }
                if name == "coding__read_file" && arguments == r#"["Cargo.toml"]"#
        ));
        assert!(matches!(&messages[1], AgentEvalMessage::ToolError(error) if error.contains("boom")));
    }

    async fn collect_messages(group: AgentEvalMessageGroup) -> Vec<AgentEvalMessage> {
        let (tx, mut rx) = mpsc::channel(16);
        group.forward(&tx).await.unwrap();
        drop(tx);

        let mut messages = Vec::new();
        while let Some(message) = rx.recv().await {
            messages.push(message);
        }
        messages
    }
}
