use super::{
    AcpClientCommand, Agent, AgentConfig, CONTAINER_AETHER_HOME, DockerAgent, DockerError, DockerImage, RunError,
    aether_eval_env_vars,
};
use aether_core::events::AgentMessage;
use aether_project::AetherSettings;
use testcontainers::core::Mount;
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
}

impl Agent for DockerAetherAgent {
    async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentMessage>) -> Result<(), RunError> {
        let aether_home = tempfile::tempdir().map_err(|source| DockerError::AetherHomeTempDir { source })?;
        let agent = DockerAgent::with_command_builder(
            self.image.clone(),
            AcpClientCommand::aether(self.settings.clone(), self.agent.clone()),
        )
        .with_env_vars(aether_eval_env_vars())
        .with_mount(Mount::bind_mount(aether_home.path().display().to_string(), CONTAINER_AETHER_HOME));

        agent.run(config, tx).await
    }
}
