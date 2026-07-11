use super::agent::{Agent, AgentRunResult, RunError};
use crate::Task;
use aether_core::events::{AgentEvent, ToolEvent, TurnOutcome};
use async_stream::try_stream;
use futures::Stream;
use llm::ToolCallResult;
use std::path::{Path, PathBuf};
use tokio::fs::{create_dir_all, write};

#[derive(Clone)]
pub struct FakeAgent {
    events: Vec<AgentEvent>,
    file_writes: Vec<(PathBuf, String)>,
    workspace: Option<PathBuf>,
}

impl FakeAgent {
    pub fn new(events: Vec<AgentEvent>) -> Self {
        Self { events, file_writes: Vec::new(), workspace: None }
    }

    pub fn success() -> Self {
        Self::new(vec![
            AgentEvent::text("fake_1", "Task completed successfully", true),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ])
    }

    pub fn with_tool_call(tool_name: impl Into<String>, result: impl Into<String>) -> Self {
        let tool_name = tool_name.into();
        Self::new(vec![
            AgentEvent::Tool(ToolEvent::Result {
                result: ToolCallResult {
                    id: "fake_call_1".to_string(),
                    name: tool_name,
                    arguments: "{}".to_string(),
                    result: result.into(),
                },
                result_meta: None,
            }),
            AgentEvent::text("fake_2", "Task completed using tools", true),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ])
    }

    pub fn writes_file(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        Self::success().with_file_write(path, contents)
    }

    pub fn with_file_write(mut self, path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        self.file_writes.push((path.into(), contents.into()));
        self
    }

    pub fn with_workspace(mut self, workspace: impl AsRef<Path>) -> Self {
        self.workspace = Some(workspace.as_ref().to_path_buf());
        self
    }
}

impl Agent for FakeAgent {
    fn run(&self, _task: Task) -> impl Stream<Item = AgentRunResult> + Send + '_ {
        try_stream! {
            if !self.file_writes.is_empty() && self.workspace.is_none() {
                Err(RunError::ConfigurationError("FakeAgent file writes require a bound workspace".to_string()))?;
            }

            for (path, contents) in &self.file_writes {
                let file_path = self.workspace.as_ref().expect("workspace checked above").join(path);
                if let Some(parent) = file_path.parent() {
                    create_dir_all(parent).await.map_err(|error| {
                        RunError::ExecutionFailed(format!("failed to create fake file parent: {error}"))
                    })?;
                }
                write(&file_path, contents)
                    .await
                    .map_err(|error| RunError::ExecutionFailed(format!("failed to write fake file: {error}")))?;
            }

            for event in &self.events {
                yield event.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::events::{MessageEvent, TurnEvent};

    #[tokio::test]
    async fn fake_agent_success_sends_done() {
        let agent = FakeAgent::success();
        let transcript = crate::Transcript::from_stream(agent.run(Task::new("test task"))).await.unwrap();
        let messages = transcript.events();

        assert!(matches!(messages.first(), Some(AgentEvent::Message(MessageEvent::Text { .. }))));
        assert!(matches!(messages.get(1), Some(AgentEvent::Turn(TurnEvent::Ended { .. }))));
    }

    #[tokio::test]
    async fn fake_agent_with_tool_call_sends_tool_messages() {
        let agent = FakeAgent::with_tool_call("bash", "success");
        let transcript = crate::Transcript::from_stream(agent.run(Task::new("test task"))).await.unwrap();
        let messages = transcript.events();

        assert_eq!(messages.len(), 3);
        assert!(matches!(messages.last(), Some(AgentEvent::Turn(TurnEvent::Ended { .. }))));
    }

    #[tokio::test]
    async fn fake_agent_writes_file_in_workspace() {
        let temp_dir = tempfile::tempdir().unwrap();
        let agent = FakeAgent::writes_file("nested/hello.txt", "hello").with_workspace(temp_dir.path());
        crate::Transcript::from_stream(agent.run(Task::new("test task"))).await.unwrap();
        assert_eq!(std::fs::read_to_string(temp_dir.path().join("nested/hello.txt")).unwrap(), "hello");
    }
}
