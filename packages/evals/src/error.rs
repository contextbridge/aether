use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalHarnessError {
    #[error("workspace setup failed: {0}")]
    Workspace(#[from] crucible::WorkspaceError),

    #[error("invalid AETHER_EVAL_DOCKER_IMAGE: {0}")]
    DockerImage(#[from] crucible::agents::DockerImageParseError),

    #[error("failed to read eval settings '{}': {source}", path.display())]
    ReadSettings {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse eval settings: {0}")]
    Settings(String),

    #[error("eval run failed: {0}")]
    EvalRun(#[from] crucible::EvalRunError),

    #[error("failed to write eval fixture '{}': {source}", path.display())]
    WriteFixture {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
