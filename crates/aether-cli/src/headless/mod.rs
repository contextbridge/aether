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
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::credentials::oauth_credential_store_from_config;
use crate::mcp_config_args::McpConfigArgs;
use crate::output::OutputFormat;
use crate::provider_connection_args::ProviderConnectionArgs;
use crate::resolve::resolve_agent_spec;
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
        let spec = resolve_spec_from_settings(
            args.agent.as_deref(),
            args.model.as_deref(),
            &cwd,
            settings,
            provider_connections,
        )?;
        let mcp_config_sources = args.mcp_config.sources(&cwd);

        Ok(Self {
            prompt,
            cwd,
            mcp_config_sources,
            spec,
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
        let spec = resolve_spec_from_settings(
            options.agent.as_deref(),
            options.model.as_deref(),
            &cwd,
            settings,
            provider_connections,
        )?;
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
            spec,
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

fn resolve_spec_from_settings(
    agent: Option<&str>,
    model: Option<&str>,
    cwd: &Path,
    settings: AetherSettings,
    provider_connections: ProviderConnectionOverrides,
) -> Result<AgentSpec, CliError> {
    if agent.is_some() && model.is_some() {
        return Err(CliError::ConflictingArgs("Cannot specify both --agent and --model".to_string()));
    }

    let catalog = AgentCatalog::from_settings_or_empty(cwd, settings)?;

    let mut spec = match model {
        Some(m) => {
            let parsed = m.parse().map_err(CliError::ModelError)?;
            AgentSpec::default_spec(&parsed, None, Vec::new())
        }
        None => resolve_agent_spec(&catalog, agent)?,
    };
    spec.provider_connections.merge(provider_connections);
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use std::fs::{create_dir_all, write};

    use super::*;

    fn resolve_spec(
        agent: Option<&str>,
        model: Option<&str>,
        cwd: &Path,
        settings_source: &SettingsSourceArgs,
        provider_connections: ProviderConnectionOverrides,
    ) -> Result<AgentSpec, CliError> {
        let settings = settings_source.load_settings(cwd)?;
        resolve_spec_from_settings(agent, model, cwd, settings, provider_connections)
    }

    #[test]
    fn resolve_spec_with_named_agent() {
        let dir = setup_dir_with_agents();
        let spec = resolve_spec(
            Some("beta"),
            None,
            dir.path(),
            &project_settings_args(),
            ProviderConnectionOverrides::default(),
        )
        .unwrap();
        assert_eq!(spec.name, "beta");
    }

    #[test]
    fn resolve_spec_with_model_creates_default() {
        let dir = setup_dir_with_agents();
        let spec = resolve_spec(
            None,
            Some("anthropic:claude-sonnet-4-5"),
            dir.path(),
            &project_settings_args(),
            ProviderConnectionOverrides::default(),
        )
        .unwrap();
        assert_eq!(spec.name, "__default__");
    }

    #[test]
    fn resolve_spec_defaults_to_first_user_invocable() {
        let dir = setup_dir_with_agents();
        let spec =
            resolve_spec(None, None, dir.path(), &project_settings_args(), ProviderConnectionOverrides::default())
                .unwrap();
        assert_eq!(spec.name, "alpha");
    }

    #[test]
    fn resolve_spec_defaults_to_fallback_without_settings() {
        let dir = tempfile::tempdir().unwrap();
        let spec = resolve_spec(None, None, dir.path(), &empty_settings_args(), ProviderConnectionOverrides::default())
            .unwrap();
        assert_eq!(spec.name, "__default__");
    }

    #[test]
    fn resolve_spec_rejects_both_agent_and_model() {
        let dir = setup_dir_with_agents();
        let err = resolve_spec(
            Some("alpha"),
            Some("anthropic:claude-sonnet-4-5"),
            dir.path(),
            &SettingsSourceArgs::default(),
            ProviderConnectionOverrides::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Cannot specify both"), "unexpected error: {err}");
    }

    #[test]
    fn resolve_spec_rejects_invalid_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_spec(
            None,
            Some("not-a-valid-model"),
            dir.path(),
            &empty_settings_args(),
            ProviderConnectionOverrides::default(),
        )
        .unwrap_err();
        assert!(matches!(err, CliError::ModelError(_)));
    }

    #[test]
    fn resolve_spec_rejects_unknown_agent() {
        let dir = setup_dir_with_agents();
        let err = resolve_spec(
            Some("nonexistent"),
            None,
            dir.path(),
            &project_settings_args(),
            ProviderConnectionOverrides::default(),
        )
        .unwrap_err();
        assert!(matches!(err, CliError::AgentError(_)));
    }

    fn write_file(dir: &std::path::Path, path: &str, content: &str) {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            create_dir_all(parent).unwrap();
        }
        write(full, content).unwrap();
    }

    fn project_settings_args() -> SettingsSourceArgs {
        SettingsSourceArgs { settings_json: None, settings_file: Some(PathBuf::from(".aether/settings.json")) }
    }

    fn empty_settings_args() -> SettingsSourceArgs {
        SettingsSourceArgs { settings_json: Some(r#"{"agents":[]}"#.to_string()), settings_file: None }
    }

    fn setup_dir_with_agents() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "PROMPT.md", "Be helpful");
        write_file(
            dir.path(),
            ".aether/settings.json",
            r#"{"agents": [
                {"name": "alpha", "description": "Alpha agent", "model": "anthropic:claude-sonnet-4-5", "userInvocable": true, "prompts": [{"type":"file","path":"PROMPT.md"}]},
                {"name": "beta", "description": "Beta agent", "model": "anthropic:claude-sonnet-4-5", "userInvocable": true, "prompts": [{"type":"file","path":"PROMPT.md"}]}
            ]}"#,
        );
        dir
    }
}
