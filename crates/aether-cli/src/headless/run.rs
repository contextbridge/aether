use aether_core::core::{AgentDeps, Prompt, recv_agent_event};
use aether_core::events::{
    AgentEvent, Command, ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome,
};
use aether_telemetry::TelemetryRuntime;
use llm::StreamingModelProvider;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::telemetry::build_telemetry_runtime;

use super::{CliEventKind, RunConfig};
use crate::error::CliError;
use crate::output::{OutputFormat, write_message};
use crate::runtime::RuntimeBuilder;
use crate::slash_commands::expand_slash_command_text;

pub struct HeadlessRunner {
    config: RunConfig,
    provider: Option<Arc<dyn StreamingModelProvider>>,
}

impl HeadlessRunner {
    pub fn new(config: RunConfig) -> Self {
        Self { config, provider: None }
    }

    pub fn with_provider(mut self, provider: Arc<dyn StreamingModelProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub async fn run(self, stdout: &mut dyn Write, stderr: &mut dyn Write) -> Result<ExitCode, CliError> {
        let telemetry = build_telemetry_runtime(self.config.telemetry.as_ref(), self.config.trace_context.clone())?;
        let result = run_agent(self.config, self.provider, telemetry.clone(), stdout, stderr).await;
        if let Some(telemetry) = telemetry {
            telemetry.shutdown_or_log();
        }
        result
    }
}

pub async fn run(config: RunConfig) -> Result<ExitCode, CliError> {
    setup_tracing(config.verbose);
    HeadlessRunner::new(config).run(&mut io::stdout(), &mut io::stderr()).await
}

async fn run_agent(
    config: RunConfig,
    provider: Option<Arc<dyn StreamingModelProvider>>,
    telemetry: Option<Arc<TelemetryRuntime>>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<ExitCode, CliError> {
    let mut spec = config.spec;
    if let Some(system_prompt) = config.system_prompt {
        spec.prompts.push(Prompt::text(&system_prompt));
    }

    let registry = config.agent_catalog.registry().clone();
    let deps =
        AgentDeps::new(config.oauth_credential_store, telemetry.as_ref().map(|runtime| runtime.observer_factory()))
            .with_agent_registry(registry);
    let mut builder = RuntimeBuilder::from_spec(config.cwd.clone(), spec)
        .mcp_config(config.mcp_config)
        .agent_deps(deps)
        .wait_for_mcp();
    if let Some(provider) = provider {
        builder = builder.provider(provider);
    }
    let mut agent = builder.build(Vec::new()).await?;

    let prompt = expand_slash_command_text(&agent.mcp, config.prompt).await;

    let outcome = match agent.agent_tx.send(Command::text(&prompt)).await {
        Ok(()) => Ok(stream_output(&mut agent.agent_rx, config.output, &config.events, stdout, stderr).await),
        Err(_) => Err(CliError::AgentStopped),
    };

    agent.shutdown().await;

    outcome
}

async fn stream_output(
    rx: &mut mpsc::Receiver<AgentEvent>,
    format: OutputFormat,
    events: &[CliEventKind],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> ExitCode {
    loop {
        let msg = recv_agent_event(rx).await;
        if should_emit(&msg, events)
            && let Err(error) = write_message(format, &msg, stdout, stderr)
        {
            let _ = writeln!(stderr, "Failed to write headless event: {error}");
            return ExitCode::FAILURE;
        }

        if let Some(outcome) = msg.turn_outcome() {
            return match outcome {
                TurnOutcome::Failed { .. } => ExitCode::FAILURE,
                TurnOutcome::Completed | TurnOutcome::Cancelled => ExitCode::SUCCESS,
            };
        }
    }
}

fn should_emit(msg: &AgentEvent, include: &[CliEventKind]) -> bool {
    let Some(kind) = event_kind(msg) else { return false };
    include.is_empty() || include.contains(&kind)
}

fn event_kind(msg: &AgentEvent) -> Option<CliEventKind> {
    match msg {
        AgentEvent::Message(MessageEvent::Text { is_complete: true, .. }) => Some(CliEventKind::Text),
        AgentEvent::Message(MessageEvent::Thought { is_complete: true, .. }) => Some(CliEventKind::Thought),
        AgentEvent::Tool(ToolEvent::Call { .. }) => Some(CliEventKind::ToolCall),
        AgentEvent::Tool(
            ToolEvent::Result { .. } | ToolEvent::TaskCreated { .. } | ToolEvent::TaskCompleted { .. },
        ) => Some(CliEventKind::ToolResult),
        AgentEvent::Tool(ToolEvent::Error { .. } | ToolEvent::TaskFailed { .. } | ToolEvent::TaskCancelled { .. }) => {
            Some(CliEventKind::ToolError)
        }
        AgentEvent::Turn(TurnEvent::AutoContinue { .. }) => Some(CliEventKind::AutoContinue),
        AgentEvent::Model(ModelEvent::Switched { .. }) => Some(CliEventKind::ModelSwitched),
        AgentEvent::Tool(
            ToolEvent::Progress { .. }
            | ToolEvent::DisplayUpdate { .. }
            | ToolEvent::SubAgentProgress { .. }
            | ToolEvent::TaskStatus { .. },
        ) => Some(CliEventKind::ToolProgress),
        AgentEvent::Context(ContextEvent::CompactionStarted { .. }) => Some(CliEventKind::ContextCompactionStarted),
        AgentEvent::Context(ContextEvent::CompactionEnded { .. }) => Some(CliEventKind::ContextCompactionEnded),
        AgentEvent::Context(ContextEvent::CompactionResult { .. }) => Some(CliEventKind::ContextCompactionResult),
        AgentEvent::Context(ContextEvent::UsageUpdated { .. }) => Some(CliEventKind::ContextUsage),
        AgentEvent::SessionUsage(_) => Some(CliEventKind::SessionUsage),
        AgentEvent::Context(ContextEvent::Cleared) => Some(CliEventKind::ContextCleared),
        AgentEvent::Turn(TurnEvent::Started { .. }) => Some(CliEventKind::TurnStarted),
        AgentEvent::Turn(TurnEvent::Ended { .. }) => Some(CliEventKind::TurnEnded),
        AgentEvent::Turn(TurnEvent::RetryScheduled { .. }) => Some(CliEventKind::LlmRetryScheduled),
        AgentEvent::Turn(TurnEvent::LlmCallStarted { .. }) => Some(CliEventKind::LlmCallStarted),
        AgentEvent::Turn(TurnEvent::LlmCallEnded { .. }) => Some(CliEventKind::LlmCallEnded),
        AgentEvent::Tool(ToolEvent::DefinitionsUpdated { .. }) => Some(CliEventKind::ToolDefinitionsUpdated),
        AgentEvent::Message(
            MessageEvent::Text { is_complete: false, .. } | MessageEvent::Thought { is_complete: false, .. },
        )
        | AgentEvent::Turn(TurnEvent::UserMessageInserted { .. } | TurnEvent::UserMessageDiscarded { .. })
        | AgentEvent::Tool(ToolEvent::InputStarted { .. } | ToolEvent::InputDelta { .. }) => None,
    }
}

fn setup_tracing(verbose: bool) {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::EnvFilter;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = if verbose { EnvFilter::new("debug,agent=off") } else { EnvFilter::new("error,agent=off") };
    let layer = fmt::layer().with_writer(io::stderr).with_filter(filter);

    tracing_subscriber::registry().with(layer).init();
}
