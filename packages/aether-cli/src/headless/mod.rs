pub mod error;
pub mod run;

use aether_core::agent_spec::AgentSpec;
use aether_project::{AetherConfig, AgentCatalog};
use error::CliError;
use std::io::{IsTerminal, Read as _, stdin};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::config_args::{ConfigSourceArgs, McpConfigArgs};
use crate::resolve::resolve_agent_spec;

#[derive(Clone)]
pub enum OutputFormat {
    Text,
    Pretty,
    Json,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
#[clap(rename_all = "snake_case")]
pub enum CliEventKind {
    Text,
    Thought,
    ToolCall,
    ToolResult,
    ToolError,
    Error,
    Cancelled,
    AutoContinue,
    Retrying,
    ModelSwitched,
    ToolProgress,
    ContextCompactionStarted,
    ContextCompactionResult,
    ContextUsage,
    ContextCleared,
}

impl CliEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Thought => "thought",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::ToolError => "tool_error",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
            Self::AutoContinue => "auto_continue",
            Self::Retrying => "retrying",
            Self::ModelSwitched => "model_switched",
            Self::ToolProgress => "tool_progress",
            Self::ContextCompactionStarted => "context_compaction_started",
            Self::ContextCompactionResult => "context_compaction_result",
            Self::ContextUsage => "context_usage",
            Self::ContextCleared => "context_cleared",
        }
    }
}

pub struct RunConfig {
    pub prompt: String,
    pub cwd: PathBuf,
    pub mcp_config_sources: Vec<aether_core::agent_spec::McpConfigSource>,
    pub spec: AgentSpec,
    pub system_prompt: Option<String>,
    pub output: OutputFormat,
    pub verbose: bool,
    pub events: Vec<CliEventKind>,
}

pub async fn run_headless(args: HeadlessArgs) -> Result<ExitCode, CliError> {
    let prompt = resolve_prompt(&args)?;
    let cwd = args.cwd.canonicalize().map_err(CliError::IoError)?;
    let spec = resolve_spec(args.agent.as_deref(), args.model.as_deref(), &cwd, &args.config_source)?;

    let output = match args.output {
        CliOutputFormat::Text => OutputFormat::Text,
        CliOutputFormat::Pretty => OutputFormat::Pretty,
        CliOutputFormat::Json => OutputFormat::Json,
    };

    let mcp_config_sources = args.mcp_config.sources(&cwd);

    let config = RunConfig {
        prompt,
        cwd,
        mcp_config_sources,
        spec,
        system_prompt: args.system_prompt,
        output,
        verbose: args.verbose,
        events: args.events,
    };

    run::run(config).await
}

#[derive(Clone, clap::ValueEnum)]
pub enum CliOutputFormat {
    Text,
    Pretty,
    Json,
}

#[derive(clap::Args)]
pub struct HeadlessArgs {
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
    pub config_source: ConfigSourceArgs,

    #[command(flatten)]
    pub mcp_config: McpConfigArgs,

    /// Additional system prompt
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Output format
    #[arg(long, default_value = "text")]
    pub output: CliOutputFormat,

    /// Verbose diagnostic logging to stderr.
    #[arg(short, long)]
    pub verbose: bool,

    /// Comma-separated list of events to emit (e.g. `tool_call,tool_result`).
    /// Omit to emit everything. When set, `error` is only shown if explicitly listed.
    #[arg(long = "events", value_enum, value_delimiter = ',')]
    pub events: Vec<CliEventKind>,
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

fn resolve_spec(
    agent: Option<&str>,
    model: Option<&str>,
    cwd: &std::path::Path,
    config_source: &ConfigSourceArgs,
) -> Result<AgentSpec, CliError> {
    if agent.is_some() && model.is_some() {
        return Err(CliError::ConflictingArgs("Cannot specify both --agent and --model".to_string()));
    }

    let config = if let Some(source) = config_source.source() {
        AetherConfig::load(cwd, [source])
    } else {
        AetherConfig::load_default(cwd)
    }
    .map_err(|e| CliError::AgentError(e.to_string()))?;
    let catalog = if config.agents.is_empty() {
        AgentCatalog::empty(cwd.to_path_buf())
    } else {
        AgentCatalog::from_config(cwd, config).map_err(|e| CliError::AgentError(e.to_string()))?
    };

    match model {
        Some(m) => {
            let parsed = m.parse().map_err(|e: String| CliError::ModelError(e))?;
            Ok(AgentSpec::default_spec(&parsed, None, Vec::new()))
        }
        None => resolve_agent_spec(&catalog, agent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &std::path::Path, path: &str, content: &str) {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
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

    #[test]
    fn resolve_spec_with_named_agent() {
        let dir = setup_dir_with_agents();
        let spec = resolve_spec(Some("beta"), None, dir.path(), &ConfigSourceArgs::default()).unwrap();
        assert_eq!(spec.name, "beta");
    }

    #[test]
    fn resolve_spec_with_model_creates_default() {
        let dir = setup_dir_with_agents();
        let spec =
            resolve_spec(None, Some("anthropic:claude-sonnet-4-5"), dir.path(), &ConfigSourceArgs::default()).unwrap();
        assert_eq!(spec.name, "__default__");
    }

    #[test]
    fn resolve_spec_defaults_to_first_user_invocable() {
        let dir = setup_dir_with_agents();
        let spec = resolve_spec(None, None, dir.path(), &ConfigSourceArgs::default()).unwrap();
        assert_eq!(spec.name, "alpha");
    }

    #[test]
    fn resolve_spec_defaults_to_fallback_without_settings() {
        let dir = tempfile::tempdir().unwrap();
        let spec = resolve_spec(None, None, dir.path(), &ConfigSourceArgs::default()).unwrap();
        assert_eq!(spec.name, "default");
    }

    #[test]
    fn resolve_spec_rejects_both_agent_and_model() {
        let dir = setup_dir_with_agents();
        let err =
            resolve_spec(Some("alpha"), Some("anthropic:claude-sonnet-4-5"), dir.path(), &ConfigSourceArgs::default())
                .unwrap_err();
        assert!(err.to_string().contains("Cannot specify both"), "unexpected error: {err}");
    }

    #[test]
    fn resolve_spec_rejects_invalid_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_spec(None, Some("not-a-valid-model"), dir.path(), &ConfigSourceArgs::default()).unwrap_err();
        assert!(matches!(err, CliError::ModelError(_)));
    }

    #[test]
    fn resolve_spec_rejects_unknown_agent() {
        let dir = setup_dir_with_agents();
        let err = resolve_spec(Some("nonexistent"), None, dir.path(), &ConfigSourceArgs::default()).unwrap_err();
        assert!(matches!(err, CliError::AgentError(_)));
    }
}
