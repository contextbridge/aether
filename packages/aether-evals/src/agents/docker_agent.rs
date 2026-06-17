use super::docker::{Docker, DockerExecConfig};
use super::transcript::is_terminal;
use super::{Agent, AgentConfig, DockerError, DockerImage, RunError, build_task_prompt};
use aether_core::events::AgentMessage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use testcontainers::core::Mount;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::sync::mpsc::Sender;

/// Environment variable through which `DockerAgent` hands the wrapped task prompt to the
/// in-container command. Shared with `aether-evals-acp-client`, which reads it back.
pub const AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV: &str = "AETHER_EVAL_WRAPPED_TASK_PROMPT";

pub struct DockerAgent {
    image: DockerImage,
    command: Arc<dyn AgentCommandBuilder>,
    env_vars: HashMap<String, String>,
    mounts: Vec<Mount>,
}

pub struct DockerCommandConfig<'a> {
    pub container_cwd: &'a Path,
}

pub trait AgentCommandBuilder: Send + Sync {
    fn build(&self, config: DockerCommandConfig<'_>) -> Vec<String>;
}

impl DockerAgent {
    pub fn new(image: DockerImage, command: Vec<String>) -> Self {
        Self::with_command_builder(image, command)
    }

    pub fn with_command_builder(image: DockerImage, builder: impl AgentCommandBuilder + 'static) -> Self {
        Self { image, command: Arc::new(builder), env_vars: HashMap::new(), mounts: Vec::new() }
    }

    pub fn with_env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.insert(key.into(), value.into());
        self
    }

    pub fn with_env_vars(mut self, env_vars: HashMap<String, String>) -> Self {
        self.env_vars.extend(env_vars);
        self
    }

    pub fn with_mount(mut self, mount: Mount) -> Self {
        self.mounts.push(mount);
        self
    }
}

impl AgentCommandBuilder for Vec<String> {
    fn build(&self, _config: DockerCommandConfig<'_>) -> Vec<String> {
        self.clone()
    }
}

impl Agent for DockerAgent {
    async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentMessage>) -> Result<(), RunError> {
        let container_workspace_root = Path::new("/workspace");
        let container_cwd = container_cwd(container_workspace_root, config.relative_cwd);
        let wrapped_task_prompt = build_task_prompt(config.task_prompt, &container_cwd);
        let command = self.command.build(DockerCommandConfig { container_cwd: &container_cwd });

        let mut docker = Docker::new(self.image.clone())
            .with_env_var("AETHER_EVAL_WORKSPACE_ROOT", container_workspace_root.display().to_string())
            .with_env_var("AETHER_EVAL_CWD", container_cwd.display().to_string())
            .with_env_var("AETHER_EVAL_TASK_PROMPT", config.task_prompt)
            .with_env_var(AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV, wrapped_task_prompt);

        for mount in &self.mounts {
            docker = docker.with_mount(mount.clone());
        }

        for (key, value) in &self.env_vars {
            docker = docker.with_env_var(key.clone(), value.clone());
        }

        let (sent_terminal, stderr) = {
            let mut session = docker.exec(DockerExecConfig { workspace_root: config.workspace_root, command }).await?;
            let sent_terminal = forward_stdout_messages(session.stdout(), &tx).await?;
            (sent_terminal, session.stderr_to_string().await?)
        };

        if !stderr.trim().is_empty() {
            tracing::debug!("docker agent stderr: {}", stderr.trim());
        }

        if sent_terminal { Ok(()) } else { Err(DockerError::CommandExitWithoutTerminal { stderr }.into()) }
    }
}

async fn forward_stdout_messages<T: AsyncBufRead + Unpin>(
    reader: T,
    tx: &Sender<AgentMessage>,
) -> Result<bool, RunError> {
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await.map_err(|source| DockerError::StdoutRead { source })? {
        if line.trim().is_empty() {
            continue;
        }
        tracing::debug!("docker agent stdout: {line}");
        let message: AgentMessage =
            serde_json::from_str(&line).map_err(|source| DockerError::AgentMessageJsonLine { line, source })?;
        let terminal = is_terminal(&message);
        tx.send(message).await.map_err(|error| RunError::ChannelSendFailed(error.to_string()))?;
        if terminal {
            return Ok(true);
        }
    }

    Ok(false)
}

fn container_cwd(container_workspace_root: &Path, relative_cwd: Option<&Path>) -> PathBuf {
    relative_cwd.map_or_else(
        || container_workspace_root.to_path_buf(),
        |relative_cwd| container_workspace_root.join(relative_cwd),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_env_vars_merges_explicit_container_env() {
        let agent = DockerAgent::new(DockerImage::new("sandbox", "latest"), vec!["agent".to_string()])
            .with_env_var("ONE", "1")
            .with_env_vars(HashMap::from([("TWO".to_string(), "2".to_string())]));

        assert_eq!(agent.env_vars.get("ONE"), Some(&"1".to_string()));
        assert_eq!(agent.env_vars.get("TWO"), Some(&"2".to_string()));
    }

    #[test]
    fn static_vec_command_is_returned_as_argv() {
        let command = vec!["print-agent-messages".to_string()]
            .build(DockerCommandConfig { container_cwd: Path::new("/workspace") });

        assert_eq!(command, vec!["print-agent-messages".to_string()]);
    }

    #[test]
    fn container_cwd_uses_workspace_root_when_no_relative_cwd() {
        assert_eq!(container_cwd(Path::new("/workspace"), None), Path::new("/workspace"));
    }

    #[test]
    fn container_cwd_joins_relative_cwd() {
        assert_eq!(container_cwd(Path::new("/workspace"), Some(Path::new("subdir"))), Path::new("/workspace/subdir"));
    }
}
