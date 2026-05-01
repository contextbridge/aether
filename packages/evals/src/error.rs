use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalHarnessError {
    #[error("workspace setup failed: {0}")]
    Workspace(#[from] crucible::WorkspaceError),

    #[error("AETHER_EVAL_MODEL is required to run evals")]
    MissingEvalModel,

    #[error("eval model provider setup failed: {0}")]
    ModelProvider(#[from] llm::LlmError),

    #[error("eval run failed: {0}")]
    EvalRun(#[from] crucible::EvalRunError),

    #[error("failed to write eval fixture '{}': {source}", path.display())]
    WriteFixture {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
