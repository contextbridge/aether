use super::diff::GitDiff;
use super::transcript::{Transcript, format_transcript};
use super::workspace::Workspace;
use crate::EvalRunError;
use crate::agents::{Agent, AgentConfig, RunError, is_terminal};
use aether_core::events::AgentMessage;
use std::fmt::{Debug, Write as _};
use thiserror::Error;
use tokio::sync::mpsc;

pub struct Task {
    prompt: String,
    workspace: Workspace,
}

pub struct TaskRun {
    prompt: String,
    workspace: Workspace,
    transcript: Transcript,
}

#[derive(Error)]
#[error("{error}")]
pub struct TaskRunError {
    run: TaskRun,
    #[source]
    error: EvalRunError,
}

impl Task {
    pub fn new(prompt: impl Into<String>, workspace: Workspace) -> Self {
        Self { prompt: prompt.into(), workspace }
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Run the task against `agent` and collect its transcript. Use [`Task::run_observed`] to also
    /// observe each `AgentMessage` as it streams.
    #[tracing::instrument(skip(agent, self))]
    pub async fn run<T: Agent>(self, agent: &T) -> Result<TaskRun, TaskRunError> {
        self.run_observed(agent, |_| {}).await
    }

    /// Run the task against `agent`, forwarding each `AgentMessage` to `on_message` before it is
    /// added to the transcript. Terminal messages (`Done`, `Error`, `Cancelled`) are forwarded too.
    #[tracing::instrument(skip(agent, self, on_message))]
    pub async fn run_observed<T: Agent, U: FnMut(&AgentMessage) + Send>(
        self,
        agent: &T,
        mut on_message: U,
    ) -> Result<TaskRun, TaskRunError> {
        let Task { prompt, workspace } = self;
        let (tx, mut rx) = mpsc::channel(100);
        let agent_task = agent.run(
            AgentConfig {
                workspace_root: workspace.root_path(),
                relative_cwd: workspace.relative_cwd(),
                task_prompt: &prompt,
            },
            tx,
        );

        let message_task = async {
            let mut messages = Vec::new();
            while let Some(message) = rx.recv().await {
                let is_done = is_terminal(&message);
                on_message(&message);
                messages.push(message);
                if is_done {
                    break;
                }
            }
            messages
        };

        let (run_result, messages) = tokio::join!(agent_task, message_task);
        let run = TaskRun::new(prompt, workspace, Transcript::new(messages));
        match run_result {
            Ok(()) => Ok(run),
            Err(error) => Err(TaskRunError::new(run, error)),
        }
    }
}

impl TaskRun {
    pub(crate) fn new(prompt: String, workspace: Workspace, transcript: Transcript) -> Self {
        Self { prompt, workspace, transcript }
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn into_workspace(self) -> Workspace {
        self.workspace
    }

    pub fn failure_context(&self) -> String {
        let mut summary = String::new();
        let _ = writeln!(summary, "Task failure context");
        let _ = writeln!(summary, "Workspace: {}", self.workspace.path().display());
        summary.push_str("Prompt:\n");
        push_indented(&mut summary, &self.prompt, 2);
        summary.push('\n');

        let (agent_diff, reference_diff) = self.workspace.capture_git_diffs();
        if let Some(diff) = &agent_diff {
            summary.push_str("Agent diff summary:\n");
            push_diff_stats(&mut summary, diff);
            summary.push('\n');
        }

        if let Some(diff) = &reference_diff {
            summary.push_str("Reference diff summary:\n");
            push_diff_stats(&mut summary, diff);
            summary.push('\n');
        }

        summary.push_str("Agent messages:\n");
        if self.transcript.messages().is_empty() {
            summary.push_str("  none\n");
        } else {
            push_indented(&mut summary, &format_transcript(self.transcript.messages()), 2);
        }

        summary
    }
}

impl TaskRunError {
    fn new(run: TaskRun, error: RunError) -> Self {
        Self { run, error: EvalRunError::from(error) }
    }

    pub fn run(&self) -> &TaskRun {
        &self.run
    }

    pub fn error(&self) -> &EvalRunError {
        &self.error
    }

    pub fn into_parts(self) -> (TaskRun, EvalRunError) {
        (self.run, self.error)
    }
}

impl Debug for TaskRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TaskRunError").field("error", &self.error).finish_non_exhaustive()
    }
}

impl From<TaskRunError> for EvalRunError {
    fn from(error: TaskRunError) -> Self {
        error.error
    }
}

fn push_indented(output: &mut String, value: &str, spaces: usize) {
    let indentation = " ".repeat(spaces);
    for line in value.lines() {
        output.push_str(&indentation);
        output.push_str(line);
        output.push('\n');
    }
}

fn push_diff_stats(output: &mut String, diff: &GitDiff) {
    let _ = writeln!(output, "  Files changed: {}", diff.stats.files_changed);
    let _ = writeln!(output, "  Lines added: {}", diff.stats.lines_added);
    let _ = writeln!(output, "  Lines removed: {}", diff.stats.lines_removed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FakeAgent, RunError};
    use aether_core::events::AgentMessage;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc::Sender;

    #[tokio::test]
    async fn task_run_contains_prompt_workspace_and_transcript() {
        let run = Task::new("do the thing", Workspace::empty().unwrap())
            .run(&FakeAgent::with_tool_call("bash", "success"))
            .await
            .unwrap();

        assert_eq!(run.prompt(), "do the thing");
        assert!(run.workspace().path().exists());
        assert!(run.transcript().tool_called("bash"));
        assert!(matches!(run.transcript().messages().last(), Some(AgentMessage::Done)));
    }

    #[tokio::test]
    async fn task_run_passes_raw_prompt_without_scaffolding() {
        let agent = CapturingAgent::default();
        Task::new("do the thing", Workspace::empty().unwrap()).run(&agent).await.unwrap();

        assert_eq!(agent.captured_task_prompt(), Some("do the thing".to_string()));
    }

    #[tokio::test]
    async fn task_run_with_message_observer_forwards_messages_in_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let run = Task::new("do the thing", Workspace::empty().unwrap())
            .run_observed(&FakeAgent::with_tool_call("bash", "success"), move |message| {
                captured.lock().unwrap().push(message.clone());
            })
            .await
            .unwrap();

        let agent_messages = run.transcript().messages().to_vec();
        let observed = seen.lock().unwrap().clone();

        assert_eq!(observed.len(), agent_messages.len());
        for (observed, transcript) in observed.iter().zip(agent_messages.iter()) {
            assert_eq!(observed, transcript);
        }
        assert!(matches!(observed.last(), Some(AgentMessage::Done)));
    }

    #[derive(Default)]
    struct CapturingAgent {
        task_prompt: Arc<Mutex<Option<String>>>,
    }

    impl CapturingAgent {
        fn captured_task_prompt(&self) -> Option<String> {
            self.task_prompt.lock().unwrap().clone()
        }
    }

    impl Agent for CapturingAgent {
        async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentMessage>) -> Result<(), RunError> {
            *self.task_prompt.lock().unwrap() = Some(config.task_prompt.to_string());
            tx.send(AgentMessage::Done).await.map_err(|e| RunError::ChannelSendFailed(e.to_string()))
        }
    }
}
