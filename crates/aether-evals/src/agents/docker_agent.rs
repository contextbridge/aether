use super::transcript::is_terminal;
use super::{Agent, AgentRunResult, RunError};
use crate::Task;
use crate::containers::{Container, ContainerError};
use aether_core::events::AgentEvent;
use async_stream::try_stream;
use futures::Stream;
use std::collections::{BTreeMap, HashMap};
use tokio::io::AsyncBufReadExt;

/// Environment variable through which `DockerAgent` hands the caller-provided task prompt to the
/// in-container command.
pub const AETHER_EVAL_TASK_PROMPT_ENV: &str = "AETHER_EVAL_TASK_PROMPT";
pub const AETHER_EVAL_WORKSPACE_ROOT_ENV: &str = "AETHER_EVAL_WORKSPACE_ROOT";
pub const AETHER_EVAL_CWD_ENV: &str = "AETHER_EVAL_CWD";

pub struct DockerAgent {
    container: Container,
    command: Vec<String>,
    env_vars: HashMap<String, String>,
}

impl DockerAgent {
    /// `command` is the in-container argv to exec; its stdout must be newline-delimited
    /// `AgentEvent` JSON.
    pub fn new(container: Container, command: Vec<String>) -> Self {
        Self { container, command, env_vars: HashMap::new() }
    }

    pub fn with_env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.insert(key.into(), value.into());
        self
    }

    pub fn with_env_vars(mut self, env_vars: impl IntoIterator<Item = (String, String)>) -> Self {
        self.env_vars.extend(env_vars);
        self
    }
}

impl Agent for DockerAgent {
    fn run(&self, task: Task) -> impl Stream<Item = AgentRunResult> + Send + '_ {
        let env = {
            let mut env: BTreeMap<_, _> = self.env_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            env.insert(AETHER_EVAL_TASK_PROMPT_ENV.to_string(), task.prompt().to_string());
            env.insert(AETHER_EVAL_CWD_ENV.to_string(), self.container.cwd().display().to_string());
            env.insert(
                AETHER_EVAL_WORKSPACE_ROOT_ENV.to_string(),
                self.container.workspace_root().display().to_string(),
            );
            env
        };

        try_stream! {
            let mut exec = self.container.exec_streaming(self.command.clone(), env).await?;
            let finished = {
                let mut lines = exec.stdout().lines();
                let mut finished = false;
                while let Some(line) = lines.next_line().await.map_err(|source| ContainerError::StdoutRead { source })? {
                    if line.trim().is_empty() {
                        continue;
                    }
                    tracing::debug!("docker agent stdout: {line}");
                    let message: AgentEvent = serde_json::from_str(&line)
                        .map_err(|source| RunError::AgentEventJsonLine { line, source })?;
                    let terminal = is_terminal(&message);
                    yield message;
                    if terminal {
                        finished = true;
                        break;
                    }
                }
                finished
            };

            let stderr = exec.stderr_to_string().await?;
            if !stderr.trim().is_empty() {
                tracing::debug!("docker agent stderr: {}", stderr.trim());
            }

            if !finished {
                Err::<AgentEvent, RunError>(RunError::CommandExitWithoutTerminal { stderr })?;
            }
        }
    }
}
