use crate::EvalHarnessError;
use crucible::{AetherSettings, DockerAetherAgent, DockerImage};
use std::env::var;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

const DEFAULT_EVAL_DOCKER_IMAGE: &str = "aether-sandbox:latest";

pub type Agent = DockerAetherAgent;

pub async fn create_aether_agent(_workspace_path: &Path) -> Result<Agent, EvalHarnessError> {
    let image_ref = var("AETHER_EVAL_DOCKER_IMAGE").unwrap_or_else(|_| DEFAULT_EVAL_DOCKER_IMAGE.to_string());
    let image = DockerImage::parse(&image_ref)?;
    let agent = DockerAetherAgent::new(image).with_settings(aether_settings()?);

    Ok(match eval_agent_name() {
        Some(agent_name) => agent.with_agent(agent_name),
        None => agent,
    })
}

fn aether_settings() -> Result<AetherSettings, EvalHarnessError> {
    let root = aether_repo_root();
    let path = root.join(".aether/settings.json");
    let content = read_to_string(&path).map_err(|source| EvalHarnessError::ReadSettings { path, source })?;
    let mut settings =
        AetherSettings::try_from(content.as_str()).map_err(|e| EvalHarnessError::Settings(e.to_string()))?;
    settings.inline_resources(&root).map_err(|e| EvalHarnessError::Settings(e.to_string()))?;
    Ok(settings)
}

fn eval_agent_name() -> Option<String> {
    var("AETHER_EVAL_AGENT").ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

fn aether_repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("packages/evals should live under the repository root")
        .to_path_buf()
}
