pub(crate) mod agent;
pub(crate) mod agent_key;
pub(crate) mod agent_runtime;
pub(crate) mod config_setting;
pub(crate) mod error;
pub(crate) mod fake_prompt_mcp;
pub(crate) mod model_config;
pub(crate) mod prompt_history_index;
pub(crate) mod protocol;
pub(crate) mod session_actor;
pub(crate) mod session_config_state;
pub(crate) mod session_factory;
pub(crate) mod session_store;
pub(crate) mod slash_commands;
pub(crate) mod state;
pub(crate) mod stdio;
pub mod testing;

pub use protocol::map_mcp_prompt_to_available_command;

use crate::acp::agent::acp_agent_builder;
use crate::acp::state::{AcpState, AcpStateConfig};
use crate::acp::stdio::Stdio;
use crate::provider_connection_args::ProviderConnectionArgs;
use crate::settings_args::SettingsSourceArgs;
use agent_client_protocol as acp;
use llm::ReasoningEffort;
use std::sync::Arc;
use std::{fs::create_dir_all, path::PathBuf};
use thiserror::Error;
use tracing::info;
use tracing_appender::rolling::daily;
use tracing_subscriber::EnvFilter;

use aether_auth::OAuthCredentialStorage;
use session_factory::InitialSessionSelection;
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
    pub provider_connection: ProviderConnectionArgs,

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
#[derive(Debug, Error)]
pub enum AcpRunError {
    #[error("ACP protocol error: {0}")]
    Protocol(#[from] acp::Error),
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
    let oauth_credential_store: Arc<dyn OAuthCredentialStorage> =
        Arc::new(aether_auth::OsKeyringStore::with_platform_store());
    let provider_connections = args.provider_connection.into_overrides();
    let state = Arc::new(AcpState::new(AcpStateConfig {
        session_store,
        oauth_credential_store,
        initial_selection,
        settings_source: args.settings_source,
        provider_connections,
    }));

    let connect_result = acp_agent_builder(state.clone()).connect_to(Stdio::new()).await;
    state.shutdown_all().await;

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
