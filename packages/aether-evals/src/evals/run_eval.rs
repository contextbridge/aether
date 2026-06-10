use super::report::EvalReport;
use crate::agents::{Agent, AgentConfig, is_terminal};
use crate::{EvalRunError, Workspace};
use aether_core::events::AgentMessage;
use tokio::sync::mpsc;

#[tracing::instrument(skip(agent, prompt, workspace))]
pub async fn run_eval<T: Agent>(
    agent: &T,
    prompt: impl Into<String>,
    workspace: Workspace,
) -> Result<EvalReport, EvalRunError> {
    let prompt = prompt.into();
    let messages = run_agent(agent, &workspace, &prompt).await?;
    let (agent_diff, reference_diff) = workspace.capture_git_diffs();
    Ok(EvalReport::new(prompt, workspace, messages, agent_diff, reference_diff))
}

async fn run_agent<T: Agent>(
    agent: &T,
    workspace: &Workspace,
    task_prompt: &str,
) -> Result<Vec<AgentMessage>, EvalRunError> {
    let (tx, mut rx) = mpsc::channel(100);
    let agent_task = agent.run(
        AgentConfig { workspace_root: workspace.root_path(), relative_cwd: workspace.relative_cwd(), task_prompt },
        tx,
    );

    let message_task = async {
        let mut messages = Vec::new();
        while let Some(message) = rx.recv().await {
            let is_done = is_terminal(&message);
            messages.push(message);
            if is_done {
                break;
            }
        }
        messages
    };

    let (run_result, messages) = tokio::join!(agent_task, message_task);
    run_result?;
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FakeAgent, RunError, Workspace};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc::Sender;

    #[tokio::test]
    async fn run_eval_returns_report_with_prompt_workspace_and_messages() {
        let report =
            run_eval(&FakeAgent::with_tool_call("bash", "success"), "do the thing", Workspace::empty().unwrap())
                .await
                .unwrap();

        assert_eq!(report.prompt(), "do the thing");
        assert!(report.workspace().path().exists());
        assert!(report.tool_called("bash"));
        assert!(matches!(report.messages().last(), Some(AgentMessage::Done)));
    }

    #[tokio::test]
    async fn run_eval_passes_raw_prompt_without_scaffolding() {
        let agent = CapturingAgent::default();
        run_eval(&agent, "do the thing", Workspace::empty().unwrap()).await.unwrap();

        assert_eq!(agent.captured_task_prompt(), Some("do the thing".to_string()));
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
