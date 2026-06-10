use aether_evals::{DockerAetherAgent, DockerImage};
use aether_project::{AetherSettings, SettingsError};
use std::env::var;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalHarnessError {
    #[error("workspace setup failed: {0}")]
    Workspace(#[from] aether_evals::WorkspaceError),

    #[error("failed to load eval settings: {0}")]
    Settings(#[from] SettingsError),

    #[error("eval run failed: {0}")]
    EvalRun(#[from] aether_evals::EvalRunError),
}

pub fn create_aether_agent() -> Result<DockerAetherAgent, EvalHarnessError> {
    let image = DockerImage::new("aether-sandbox", "latest");
    let settings = load_repo_eval_settings()?;
    let agent = DockerAetherAgent::new(image).with_settings(settings);

    Ok(match eval_agent_name() {
        Some(agent_name) => agent.with_agent(agent_name),
        None => agent,
    })
}

fn aether_repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("packages/internal-evals should live under the repository root")
        .to_path_buf()
}

fn load_repo_eval_settings() -> Result<AetherSettings, EvalHarnessError> {
    Ok(AetherSettings::load_file_for_export(&aether_repo_root().join(".aether/settings.json"))?)
}

fn eval_agent_name() -> Option<String> {
    var("AETHER_EVAL_AGENT").ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}
