use aether_evals::{
    CONTAINER_AETHER_HOME, Container, ContainerError, DockerAgent, Image, Workspace, default_eval_env_vars,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalHarnessError {
    #[error("workspace setup failed: {0}")]
    Workspace(#[from] aether_evals::WorkspaceError),

    #[error("transcript collection failed: {0}")]
    Transcript(#[from] aether_evals::TranscriptError),

    #[error("container setup failed: {0}")]
    Container(#[from] ContainerError),

    #[error("file IO failed: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn create_aether_agent(workspace: &Workspace) -> Result<(Container, DockerAgent), EvalHarnessError> {
    let container = Container::builder(Image::new("aether-sandbox", "latest"))
        .with_env_vars(default_eval_env_vars())
        .with_ephemeral_mount(CONTAINER_AETHER_HOME)
        .start(workspace)
        .await?;
    let agent = DockerAgent::new(container.clone(), vec!["/usr/local/bin/aether-eval-agent".to_string()]);
    Ok((container, agent))
}
