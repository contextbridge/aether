use aether_cli::{
    headless::HeadlessOptions,
    init::{HarnessIntegration, InitScope, build_batteries_included_settings},
    output::OutputFormat,
};
use aether_evals::{
    Agent, AgentRunResult, CONTAINER_AETHER_HOME, Container, ContainerError, DockerAgent, Image, Task, Workspace,
    default_eval_env_vars,
};
use aether_project::{AetherSettings, SettingsError};
use async_stream::try_stream;
use futures::{Stream, StreamExt};
use llm::ReasoningEffort;
use std::{env, path::Path};
use thiserror::Error;

pub struct EvalAgent {
    settings: Option<AetherSettings>,
    agent: Option<String>,
    output: OutputFormat,
}

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

    #[error("AETHER_EVAL_MODEL must be set for this eval")]
    MissingEvalModel,

    #[error("invalid AETHER_EVAL_REASONING_EFFORT value '{value}': {error}")]
    InvalidEvalReasoningEffort { value: String, error: String },

    #[error("settings setup failed: {0}")]
    Settings(#[from] SettingsError),

    #[error("failed to serialize generated headless options: {0}")]
    HeadlessOptionsSerialize(#[from] serde_json::Error),
}

impl EvalAgent {
    pub fn new() -> Self {
        Self { settings: None, agent: Some("Build".to_string()), output: OutputFormat::Json }
    }

    pub fn settings(mut self, settings: AetherSettings) -> Self {
        self.settings = Some(settings);
        self
    }

    pub fn agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    pub async fn run(
        self,
        workspace: &Workspace,
        task: Task,
    ) -> Result<(Container, impl Stream<Item = AgentRunResult> + Send), EvalHarnessError> {
        let container = Container::builder(Image::new("aether-sandbox", "latest"))
            .with_env_vars(default_eval_env_vars())
            .with_ephemeral_mount(CONTAINER_AETHER_HOME)
            .start(workspace)
            .await?;

        let settings = match self.settings {
            Some(settings) => settings,
            None => batteries_included_settings()?,
        };

        let options = HeadlessOptions {
            prompt: Some(task.prompt().to_string()),
            settings: Some(settings),
            agent: self.agent,
            output: Some(self.output),
            ..HeadlessOptions::default()
        };
        let options_json = serde_json::to_string(&options)?;

        let agent = DockerAgent::new(container.clone(), vec!["/usr/local/bin/aether-eval-agent".to_string()])
            .with_env_var("AETHER_EVAL_OPTIONS_JSON", options_json);

        let stream = try_stream! {
            let messages = agent.run(task);
            futures::pin_mut!(messages);
            while let Some(message) = messages.next().await {
                yield message?;
            }
        };

        Ok((container, stream))
    }
}

impl Default for EvalAgent {
    fn default() -> Self {
        Self::new()
    }
}

pub fn eval_model() -> Result<String, EvalHarnessError> {
    env::var("AETHER_EVAL_MODEL")
        .ok()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .ok_or(EvalHarnessError::MissingEvalModel)
}

pub fn eval_reasoning_effort() -> Result<Option<ReasoningEffort>, EvalHarnessError> {
    let Ok(value) = env::var("AETHER_EVAL_REASONING_EFFORT") else {
        return Ok(None);
    };
    ReasoningEffort::parse(value.trim()).map_err(|error| EvalHarnessError::InvalidEvalReasoningEffort { value, error })
}

pub fn batteries_included_settings() -> Result<AetherSettings, EvalHarnessError> {
    let harnesses = [HarnessIntegration::Agents];
    let model = eval_model()?;
    let effort = eval_reasoning_effort()?;
    let mut settings = build_batteries_included_settings("Eval", model, effort, InitScope::User, &harnesses);
    let template_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aether-cli/src/init/templates");
    settings.inline_resources(&template_root)?;

    if let Some(source) = HarnessIntegration::Agents.prompt_source() {
        settings.prompts.push(source);
    }

    Ok(settings)
}
