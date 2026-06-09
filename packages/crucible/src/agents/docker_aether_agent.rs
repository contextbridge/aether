use super::agent_eval_message_group::{AgentEvalMessageGroup, log_agent_message};
use super::{
    Agent, AgentConfig, AgentEvalMessage, Docker, DockerError, DockerExecConfig, DockerImage, RunError,
    build_task_prompt,
};
use aether_core::events::AgentMessage;
use aether_project::AetherSettings;
use llm::LlmModel;
use std::path::{Path, PathBuf};
use testcontainers::core::Mount;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::sync::mpsc::Sender;

pub struct DockerAetherAgent {
    image: DockerImage,
    settings: Option<AetherSettings>,
    agent: Option<String>,
}

impl DockerAetherAgent {
    pub fn new(image: DockerImage) -> Self {
        Self { image, settings: None, agent: None }
    }

    pub fn with_settings(mut self, settings: AetherSettings) -> Self {
        self.settings = Some(settings);
        self
    }

    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    fn build_command(&self, task_prompt: &str, container_cwd: &Path) -> Vec<String> {
        let mut command = vec![
            "aether".to_string(),
            "headless".to_string(),
            "--cwd".to_string(),
            container_cwd.display().to_string(),
        ];

        if let Some(agent) = &self.agent {
            command.push("--agent".to_string());
            command.push(agent.clone());
        }

        command.extend(["--output".to_string(), "json".to_string()]);

        if let Some(settings) = &self.settings {
            command.push("--settings-json".to_string());
            command.push(serde_json::to_string(settings).expect("AetherSettings should serialize to JSON"));
        }
        command.push(build_task_prompt(task_prompt, container_cwd));
        command
    }
}

impl Agent for DockerAetherAgent {
    async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentEvalMessage>) -> Result<(), RunError> {
        let aether_home = tempfile::tempdir().map_err(|source| DockerError::AetherHomeTempDir { source })?;
        let docker = {
            let mut docker = Docker::new(self.image.clone())
                .with_env_var("AETHER_HOME", "/root/.aether")
                .with_mount(Mount::bind_mount(aether_home.path().display().to_string(), "/root/.aether"));

            for (key, value) in selected_env_vars(std::env::vars()) {
                docker = docker.with_env_var(key, value);
            }

            docker
        };

        let container_cwd = config
            .relative_cwd
            .map_or_else(|| PathBuf::from("/workspace"), |relative_cwd| Path::new("/workspace").join(relative_cwd));

        let command = self.build_command(config.task_prompt, &container_cwd);
        let (sent_terminal, stderr) = {
            let mut session = docker.exec(DockerExecConfig { workspace_root: config.workspace_root, command }).await?;
            let sent_terminal = forward_stdout_messages(session.stdout(), &tx).await?;

            (sent_terminal, session.stderr_to_string().await?)
        };

        if !stderr.trim().is_empty() {
            tracing::debug!("aether headless stderr: {}", stderr.trim());
        }

        if sent_terminal { Ok(()) } else { Err(DockerError::HeadlessExit { stderr }.into()) }
    }
}

async fn forward_stdout_messages<T: AsyncBufRead + Unpin>(
    reader: T,
    tx: &Sender<AgentEvalMessage>,
) -> Result<bool, RunError> {
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await.map_err(|source| DockerError::StdoutRead { source })? {
        if line.trim().is_empty() {
            continue;
        }
        let message: AgentMessage =
            serde_json::from_str(&line).map_err(|source| DockerError::HeadlessJsonLine { line, source })?;
        log_agent_message(&message);
        if AgentEvalMessageGroup::from(message).forward(tx).await? {
            return Ok(true);
        }
    }

    Ok(false)
}

fn selected_env_vars(vars: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    vars.into_iter().filter(|(key, _)| should_forward_env_var(key)).collect()
}

fn should_forward_env_var(key: &str) -> bool {
    key != "AETHER_HOME"
        && (LlmModel::ALL_REQUIRED_ENV_VARS.contains(&key) || key == "OLLAMA_HOST" || key.starts_with("AETHER_"))
}
