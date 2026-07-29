pub mod error;
pub mod run;

use aether_core::agent_spec::{AgentSpec, McpConfigSource};
use aether_project::{AetherSettings, AgentCatalog, TelemetrySettings};
use aether_telemetry::AgentTraceContext;
use error::CliError;
use llm::{ProviderConnectionOverride, ProviderConnectionOverrides};
use mcp_utils::client::McpConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read as _, stdin};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::credentials::oauth_credential_store_from_config;
use crate::mcp_config_args::McpConfigArgs;
use crate::output::OutputFormat;
use crate::provider_connection_args::ProviderConnectionArgs;
use crate::resolve::{AgentSelectionError, InitialSessionSelection, resolve_agent_from_settings};
use crate::settings_args::SettingsSourceArgs;
use aether_auth::OAuthCredentialStorage;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum, Deserialize, Serialize, JsonSchema)]
#[clap(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CliEventKind {
    Text,
    Thought,
    ToolCall,
    ToolResult,
    ToolError,
    AutoContinue,
    ModelSwitched,
    ToolProgress,
    ContextCompactionStarted,
    ContextCompactionEnded,
    ContextCompactionResult,
    ContextUsage,
    ContextCleared,
    TurnStarted,
    TurnEnded,
    LlmRetryScheduled,
    LlmCallStarted,
    LlmCallEnded,
    ToolExecutionStarted,
    ToolDefinitionsUpdated,
}

pub struct RunConfig {
    pub prompt: String,
    pub cwd: PathBuf,
    pub mcp_config_sources: Vec<McpConfigSource>,
    pub spec: AgentSpec,
    pub agent_catalog: AgentCatalog,
    pub system_prompt: Option<String>,
    pub output: OutputFormat,
    pub verbose: bool,
    pub events: Vec<CliEventKind>,
    pub oauth_credential_store: Arc<dyn OAuthCredentialStorage>,
    pub telemetry: Option<TelemetrySettings>,
    pub trace_context: Option<AgentTraceContext>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeadlessOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, ProviderConnectionOverride>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<AetherSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<McpConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbose: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<CliEventKind>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<AgentTraceContext>,
}

pub async fn run_headless(args: HeadlessArgs) -> Result<ExitCode, CliError> {
    run::run(RunConfig::from_args(args)?).await
}

#[derive(clap::Args)]
pub struct HeadlessArgs {
    #[arg(long = "options-json", value_name = "JSON", hide = true)]
    pub options_json: Option<String>,

    /// Prompt to send (reads stdin if omitted and stdin is not a TTY)
    pub prompt: Vec<String>,

    /// Named agent from settings.json (defaults to first user-invocable agent)
    #[arg(short = 'a', long = "agent")]
    pub agent: Option<String>,

    /// Model for ad-hoc runs (e.g. "anthropic:claude-sonnet-4-5"). Mutually exclusive with --agent.
    #[arg(short, long)]
    pub model: Option<String>,

    /// Working directory
    #[arg(short = 'C', long = "cwd", default_value = ".")]
    pub cwd: PathBuf,

    #[command(flatten)]
    pub settings_source: SettingsSourceArgs,

    #[command(flatten)]
    pub provider_connection: ProviderConnectionArgs,

    #[command(flatten)]
    pub mcp_config: McpConfigArgs,

    /// Additional system prompt
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Output format
    #[arg(long, default_value = "text")]
    pub output: OutputFormat,

    /// Verbose diagnostic logging to stderr.
    #[arg(short, long)]
    pub verbose: bool,

    /// Comma-separated list of events to emit (e.g. `tool_call,tool_result,turn_ended`).
    /// Omit to emit every output event. When set, turn outcomes are only shown if `turn_ended` is listed.
    #[arg(long = "events", value_enum, value_delimiter = ',')]
    pub events: Vec<CliEventKind>,
}

impl RunConfig {
    fn from_args(args: HeadlessArgs) -> Result<Self, CliError> {
        if let Some(json) = args.options_json {
            return Self::from_options(serde_json::from_str(&json).map_err(CliError::InvalidOptionsJson)?);
        }

        let prompt = resolve_prompt(&args)?;
        let cwd = args.cwd.canonicalize().map_err(CliError::IoError)?;
        let settings = args.settings_source.load_settings(&cwd)?;
        let provider_connections = args.provider_connection.clone().into_overrides();
        let oauth_credential_store = oauth_credential_store_from_config(settings.credentials_store.clone())?;
        let telemetry = settings.telemetry.clone();
        let selection = match (args.agent, args.model) {
            (Some(agent), None) => InitialSessionSelection::Agent(agent),
            (None, Some(model)) => InitialSessionSelection::Model { model, reasoning_effort: None },
            (None, None) => InitialSessionSelection::Default,
            (Some(_), Some(_)) => {
                return Err(CliError::ConflictingArgs("Cannot specify both --agent and --model".to_string()));
            }
        };
        let resolved = resolve_agent_from_settings(&cwd, settings, provider_connections, &selection)
            .map_err(map_selection_error)?;
        let mcp_config_sources = args.mcp_config.sources(&cwd);

        Ok(Self {
            prompt,
            cwd,
            mcp_config_sources,
            spec: resolved.spec,
            agent_catalog: resolved.catalog,
            system_prompt: args.system_prompt,
            output: args.output,
            verbose: args.verbose,
            events: args.events,
            oauth_credential_store,
            telemetry,
            trace_context: None,
        })
    }

    fn from_options(options: HeadlessOptions) -> Result<Self, CliError> {
        let prompt = options.prompt.ok_or(CliError::NoPrompt)?;
        let cwd = options.cwd.unwrap_or_else(|| PathBuf::from(".")).canonicalize().map_err(CliError::IoError)?;
        let settings_source = SettingsSourceArgs::from_json_options(options.settings, options.settings_file)?;
        let settings = settings_source.load_settings(&cwd)?;
        let provider_connections = ProviderConnectionOverrides::new(options.providers.unwrap_or_default());
        let oauth_credential_store = oauth_credential_store_from_config(settings.credentials_store.clone())?;
        let telemetry = settings.telemetry.clone();
        let selection = match (options.agent, options.model) {
            (Some(agent), None) => InitialSessionSelection::Agent(agent),
            (None, Some(model)) => InitialSessionSelection::Model { model, reasoning_effort: None },
            (None, None) => InitialSessionSelection::Default,
            (Some(_), Some(_)) => {
                return Err(CliError::ConflictingArgs("Cannot specify both --agent and --model".to_string()));
            }
        };
        let resolved = resolve_agent_from_settings(&cwd, settings, provider_connections, &selection)
            .map_err(map_selection_error)?;
        let mcp_config_sources = options
            .mcp_config
            .map(|config| serde_json::to_string(&config).expect("mcp config serialize"))
            .map(McpConfigSource::Json)
            .into_iter()
            .collect();

        Ok(Self {
            prompt,
            cwd,
            mcp_config_sources,
            spec: resolved.spec,
            agent_catalog: resolved.catalog,
            system_prompt: options.system_prompt,
            output: options.output.unwrap_or(OutputFormat::Text),
            verbose: options.verbose.unwrap_or(false),
            events: options.events.unwrap_or_default(),
            oauth_credential_store,
            telemetry,
            trace_context: options.trace_context,
        })
    }
}

fn resolve_prompt(args: &HeadlessArgs) -> Result<String, CliError> {
    match args.prompt.as_slice() {
        args if !args.is_empty() => Ok(args.join(" ")),

        _ if !stdin().is_terminal() => {
            let mut buf = String::new();
            stdin().read_to_string(&mut buf).map_err(CliError::IoError)?;

            match buf.trim() {
                "" => Err(CliError::NoPrompt),
                s => Ok(s.to_string()),
            }
        }
        _ => Err(CliError::NoPrompt),
    }
}

fn map_selection_error(error: AgentSelectionError) -> CliError {
    match error {
        AgentSelectionError::Settings(error) => CliError::Settings(error),
        AgentSelectionError::Agent(error) => CliError::AgentError(error.to_string()),
        AgentSelectionError::Model(error) => CliError::ModelError(error),
    }
}
