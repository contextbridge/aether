pub(crate) mod config_setting;
pub(crate) mod handlers;
pub(crate) mod mappers;
pub(crate) mod model_config;
pub(crate) mod relay;
pub(crate) mod session;
pub(crate) mod session_manager;
pub(crate) mod session_registry;
pub(crate) mod session_store;
pub mod testing;

pub use mappers::map_mcp_prompt_to_available_command;
pub use session_manager::SessionManager;

use crate::settings_args::SettingsSourceArgs;
use agent_client_protocol::{self as acp, ByteStreams};
use llm::ReasoningEffort;
use std::sync::Arc;
use std::{fs::create_dir_all, path::PathBuf};
use tokio::io::{stdin, stdout};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::info;
use tracing_appender::rolling::daily;
use tracing_subscriber::EnvFilter;

use llm::oauth::OAuthCredentialStore;
use session_manager::{InitialSessionSelection, SessionManagerConfig};
use session_registry::SessionRegistry;
use session_store::SessionStore;

#[derive(clap::Args, Debug)]
pub struct AcpArgs {
    /// Path to log file directory (default: /tmp/aether-acp-logs)
    #[clap(long, default_value = "/tmp/aether-acp-logs")]
    pub log_dir: PathBuf,

    /// Initial agent (mode) to select for new sessions. Mutually exclusive with `--model` and `--reasoning-effort`.
    #[clap(long, conflicts_with_all = ["model", "reasoning_effort"])]
    pub agent: Option<String>,

    /// Initial model id (e.g. `anthropic:claude-sonnet-4-5`) for new sessions.
    /// Mutually exclusive with `--agent`.
    #[clap(long, conflicts_with = "agent")]
    pub model: Option<String>,

    /// Initial reasoning effort for an explicit model session. Requires `--model` and is mutually exclusive with `--agent`.
    #[clap(long, value_name = "low|medium|high|xhigh", requires = "model", conflicts_with = "agent")]
    pub reasoning_effort: Option<ReasoningEffort>,

    #[command(flatten)]
    pub settings_source: SettingsSourceArgs,
}

/// Outcome of running the ACP server successfully.
#[derive(Debug)]
pub enum AcpRunOutcome {
    /// The client disconnected cleanly (e.g. EOF on stdin).
    CleanDisconnect,
}

/// Errors that terminate the ACP server run.
#[derive(Debug)]
pub enum AcpRunError {
    Protocol(acp::Error),
    OAuth(llm::oauth::OAuthError),
}

impl std::fmt::Display for AcpRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcpRunError::Protocol(e) => write!(f, "ACP protocol error: {e}"),
            AcpRunError::OAuth(e) => write!(f, "ACP OAuth error: {e}"),
        }
    }
}

impl std::error::Error for AcpRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AcpRunError::Protocol(e) => Some(e),
            AcpRunError::OAuth(e) => Some(e),
        }
    }
}

pub async fn run_acp(args: AcpArgs) -> Result<AcpRunOutcome, AcpRunError> {
    info!("Starting Aether ACP server");

    setup_logging(&args);

    let initial_selection = if let Some(agent) = args.agent.clone() {
        InitialSessionSelection::agent(agent)
    } else if let Some(model) = args.model.clone() {
        InitialSessionSelection::model(model, args.reasoning_effort)
    } else {
        InitialSessionSelection::default()
    };
    let session_store =
        SessionStore::new().map_or_else(|e| panic!("Failed to initialize session store: {e}"), Arc::new);
    let oauth_credential_store = OAuthCredentialStore::with_platform_store().map_err(AcpRunError::OAuth)?;
    let manager = Arc::new(SessionManager::new(SessionManagerConfig {
        registry: Arc::new(SessionRegistry::new()),
        session_store,
        oauth_credential_store,
        initial_selection,
        settings_source: args.settings_source,
    }));

    let transport = ByteStreams::new(stdout().compat_write(), stdin().compat());
    let connect_result = handlers::acp_agent_builder(manager.clone()).connect_to(transport).await;

    manager.shutdown_all_sessions().await;

    match connect_result {
        Ok(()) => Ok(AcpRunOutcome::CleanDisconnect),
        Err(err) => Err(AcpRunError::Protocol(err)),
    }
}

fn setup_logging(args: &AcpArgs) {
    create_dir_all(&args.log_dir).ok();
    tracing_subscriber::fmt()
        .with_writer(daily(&args.log_dir, "aether-acp.log"))
        .with_ansi(false) // No ANSI colors in log files
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")))
        .pretty()
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        args: AcpArgs,
    }

    #[test]
    fn agent_conflicts_with_model() {
        let err = TestCli::try_parse_from(["test", "--agent", "planner", "--model", "anthropic:claude-sonnet-4-5"])
            .expect_err("agent and model should conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn agent_conflicts_with_reasoning_effort() {
        let err = TestCli::try_parse_from(["test", "--agent", "planner", "--reasoning-effort", "high"])
            .expect_err("agent and reasoning effort should conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn reasoning_effort_requires_model() {
        let err = TestCli::try_parse_from(["test", "--reasoning-effort", "high"])
            .expect_err("reasoning effort should require model");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn reasoning_effort_with_model_is_allowed() {
        let cli =
            TestCli::try_parse_from(["test", "--model", "anthropic:claude-sonnet-4-5", "--reasoning-effort", "high"])
                .expect("reasoning effort can configure an explicit model session");
        assert_eq!(cli.args.reasoning_effort, Some(ReasoningEffort::High));
    }
}
