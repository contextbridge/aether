use super::agent::{Agent, AgentConfig, RunError};
use aether_core::events::AgentMessage;
use llm::ToolCallResult;
use std::path::PathBuf;
use tokio::sync::mpsc::Sender;

#[derive(Clone)]
pub struct FakeAgent {
    messages: Vec<AgentMessage>,
    file_writes: Vec<(PathBuf, String)>,
}

impl FakeAgent {
    pub fn new(messages: Vec<AgentMessage>) -> Self {
        Self { messages, file_writes: Vec::new() }
    }

    pub fn success() -> Self {
        Self::new(vec![AgentMessage::text("fake_1", "Task completed successfully", true, "fake"), AgentMessage::Done])
    }

    pub fn with_tool_call(tool_name: impl Into<String>, result: impl Into<String>) -> Self {
        let tool_name = tool_name.into();
        Self::new(vec![
            AgentMessage::ToolResult {
                result: ToolCallResult {
                    id: "fake_call_1".to_string(),
                    name: tool_name,
                    arguments: "{}".to_string(),
                    result: result.into(),
                },
                result_meta: None,
                model_name: "fake".to_string(),
            },
            AgentMessage::text("fake_2", "Task completed using tools", true, "fake"),
            AgentMessage::Done,
        ])
    }

    pub fn writes_file(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        Self::success().with_file_write(path, contents)
    }

    pub fn with_file_write(mut self, path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        self.file_writes.push((path.into(), contents.into()));
        self
    }
}

impl Agent for FakeAgent {
    async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentMessage>) -> Result<(), RunError> {
        for (path, contents) in &self.file_writes {
            let file_path = config.workspace().join(path);
            if let Some(parent) = file_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|error| {
                    RunError::ExecutionFailed(format!("failed to create fake file parent: {error}"))
                })?;
            }
            tokio::fs::write(&file_path, contents)
                .await
                .map_err(|error| RunError::ExecutionFailed(format!("failed to write fake file: {error}")))?;
        }

        for message in &self.messages {
            tx.send(message.clone()).await.map_err(|error| RunError::ChannelSendFailed(error.to_string()))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn fake_agent_success_sends_done() {
        let agent = FakeAgent::success();
        let (tx, mut rx) = mpsc::channel(10);
        let temp_dir = tempfile::tempdir().unwrap();
        let config = AgentConfig { workspace_root: temp_dir.path(), relative_cwd: None, task_prompt: "test task" };
        let result = agent.run(config, tx).await;
        assert!(result.is_ok());

        let message = rx.recv().await.unwrap();
        assert!(matches!(message, AgentMessage::Text { .. }));

        let message = rx.recv().await.unwrap();
        assert!(matches!(message, AgentMessage::Done));
    }

    #[tokio::test]
    async fn fake_agent_with_tool_call_sends_tool_messages() {
        let agent = FakeAgent::with_tool_call("bash", "success");
        let (tx, mut rx) = mpsc::channel(10);
        let temp_dir = tempfile::tempdir().unwrap();
        let config = AgentConfig { workspace_root: temp_dir.path(), relative_cwd: None, task_prompt: "test task" };
        let result = agent.run(config, tx).await;
        assert!(result.is_ok());

        let mut count = 0;
        while let Some(message) = rx.recv().await {
            count += 1;
            if matches!(message, AgentMessage::Done) {
                break;
            }
        }
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn fake_agent_writes_file_in_workspace() {
        let agent = FakeAgent::writes_file("nested/hello.txt", "hello");
        let (tx, _rx) = mpsc::channel(10);
        let temp_dir = tempfile::tempdir().unwrap();
        let config = AgentConfig { workspace_root: temp_dir.path(), relative_cwd: None, task_prompt: "test task" };
        agent.run(config, tx).await.unwrap();
        assert_eq!(std::fs::read_to_string(temp_dir.path().join("nested/hello.txt")).unwrap(), "hello");
    }
}
