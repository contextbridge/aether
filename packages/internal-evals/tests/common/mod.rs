use aether_evals::{CONTAINER_AETHER_HOME, DockerAgent, DockerImage, default_eval_env_vars};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalHarnessError {
    #[error("workspace setup failed: {0}")]
    Workspace(#[from] aether_evals::WorkspaceError),

    #[error("eval run failed: {0}")]
    EvalRun(#[from] aether_evals::EvalRunError),

    #[error("eval run failed: {0}")]
    TaskRun(Box<aether_evals::TaskRunError>),
}

impl From<aether_evals::TaskRunError> for EvalHarnessError {
    fn from(error: aether_evals::TaskRunError) -> Self {
        Self::TaskRun(Box::new(error))
    }
}

pub fn create_aether_agent() -> DockerAgent {
    DockerAgent::new(DockerImage::new("aether-sandbox", "latest"), vec!["/usr/local/bin/aether-eval-agent".to_string()])
        .with_env_vars(default_eval_env_vars())
        .with_ephemeral_mount(CONTAINER_AETHER_HOME)
}
