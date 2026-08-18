use aether_cli::acp::{AcpArgs, AcpRunError, AcpRunOutcome, run_acp};
use aether_cli::error::CliError;
use aether_cli::generate_command::{GenerateArgs, GenerateCommandError, run as run_generate_command};
use aether_cli::headless::{HeadlessArgs, run_headless};
use aether_cli::init::{InitError, InitOutcome, InitRequest, next_steps_message, run_init};
use aether_cli::mcp_command::{McpArgs, McpCommandError, run as run_mcp_command};
use aether_cli::settings::SettingsCommand;
use aether_cli::show_prompt::{PromptArgs, run_prompt};
use aether_project::{AgentCatalog, project_settings_path, user_settings_path};
use clap::{Parser, Subcommand};
use std::env::current_dir;
use std::process::ExitCode;
use tokio::runtime::Runtime;
use wisp::run_tui;
use wisp::settings::{StatusLineSegmentConfig, StatusLineSettings, load_or_create_settings};

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error("{0}")]
    Cli(#[from] CliError),
    #[error("{0}")]
    Generate(#[from] GenerateCommandError),
    #[error("{0}")]
    Acp(#[from] AcpRunError),
    #[error("{0}")]
    Init(#[from] InitError),
    #[error("{0}")]
    Mcp(#[from] McpCommandError),
    #[error("{0}")]
    Lspd(#[from] aether_lspd::LspdRunError),
    #[error("{0}")]
    Tui(#[from] wisp::error::AppError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Settings(String),
}

#[derive(Parser)]
#[command(name = "aether")]
#[command(about = "Aether AI coding agent")]
#[command(version)]
struct Cli {
    /// Run inside a Docker sandbox using the given image
    #[arg(long, global = true)]
    sandbox_image: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a single prompt headlessly
    Headless(HeadlessArgs),
    /// Call a model with a single prompt and print its response
    Generate(GenerateArgs),
    /// Start the ACP server
    Acp(AcpArgs),
    /// Print the fully assembled system prompt (for debugging)
    ShowPrompt(PromptArgs),
    /// Discover and call deferred MCP tools
    Mcp(McpArgs),
    /// Manage Aether settings
    #[command(subcommand)]
    Settings(SettingsCommand),
    /// Start the LSP daemon (used internally)
    #[command(hide = true)]
    Lspd(aether_lspd::LspdArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(image) = cli.sandbox_image {
        return aether_cli::sandbox::exec_in_container(&image);
    }

    let rt = Runtime::new().expect("Failed to create tokio runtime");
    let result: Result<ExitCode, MainError> = match cli.command {
        Some(Command::Headless(args)) => rt.block_on(run_headless(args)).map_err(Into::into),

        Some(Command::Generate(args)) => rt.block_on(run_generate_command(args)).map_err(Into::into),

        Some(Command::Acp(args)) => rt
            .block_on(run_acp(args))
            .map(|outcome| match outcome {
                AcpRunOutcome::CleanDisconnect => ExitCode::SUCCESS,
            })
            .map_err(Into::into),

        Some(Command::ShowPrompt(args)) => {
            rt.block_on(run_prompt(args)).map(|()| ExitCode::SUCCESS).map_err(Into::into)
        }

        Some(Command::Mcp(args)) => rt.block_on(run_mcp_command(args)).map(|()| ExitCode::SUCCESS).map_err(Into::into),

        Some(Command::Settings(SettingsCommand::Init(args))) => rt.block_on(run_init_command(args.into())),

        Some(Command::Lspd(args)) => aether_lspd::run_lspd(args).map(|()| ExitCode::SUCCESS).map_err(Into::into),

        None => rt.block_on(run_default_command()),
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e}");
            match e {
                MainError::Mcp(error) => ExitCode::from(error.exit_code()),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

async fn run_init_command(request: InitRequest) -> Result<ExitCode, MainError> {
    let outcome = run_init(request).await?;
    if let Some(msg) = next_steps_message(&outcome) {
        println!("{msg}");
    }

    Ok(match outcome {
        InitOutcome::Applied { .. } | InitOutcome::AlreadyInitialized { .. } | InitOutcome::Cancelled => {
            ExitCode::SUCCESS
        }
    })
}

async fn run_default_command() -> Result<ExitCode, MainError> {
    let cwd = current_dir()?;
    let existing_settings = {
        let mut paths = Vec::new();
        if let Some(path) = user_settings_path().filter(|path| path.is_file()) {
            paths.push(path);
        }

        let project_path = project_settings_path(&cwd);
        if project_path.is_file() {
            paths.push(project_path);
        }

        paths
    };

    if AgentCatalog::load_default(&cwd)
        .map_err(|error| MainError::Settings(invalid_settings_message(&existing_settings, error)))?
        .is_none()
    {
        let outcome = run_init(InitRequest::user_onboarding()).await?;
        if let Some(msg) = next_steps_message(&outcome) {
            println!("{msg}");
        }

        match outcome {
            InitOutcome::Cancelled | InitOutcome::Applied { missing_env_var: Some(_), .. } => {
                return Ok(ExitCode::SUCCESS);
            }
            InitOutcome::Applied { missing_env_var: None, .. } | InitOutcome::AlreadyInitialized { .. } => {}
        }
    }

    run_tui("aether acp", load_or_create_settings(default_status_line()))
        .await
        .map(|()| ExitCode::SUCCESS)
        .map_err(Into::into)
}

fn default_status_line() -> StatusLineSettings {
    StatusLineSettings {
        separator: Some(" · ".to_string()),
        left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }, StatusLineSegmentConfig::GitRef]),
        right: Some(vec![
            StatusLineSegmentConfig::Mode,
            StatusLineSegmentConfig::Model { max_width: None },
            StatusLineSegmentConfig::Reasoning,
            StatusLineSegmentConfig::Context,
            StatusLineSegmentConfig::ServerHealth,
        ]),
    }
}
fn invalid_settings_message(paths: &[std::path::PathBuf], error: impl std::fmt::Display) -> String {
    format!(
        "Found settings at {}, but they are invalid: {error}\nRun `aether settings init --user --force` to replace user settings, `aether settings init --project --force` to replace project settings, or edit the settings JSON manually.",
        format_settings_paths(paths)
    )
}

fn format_settings_paths(paths: &[std::path::PathBuf]) -> String {
    if paths.is_empty() {
        "the default locations".to_string()
    } else {
        paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
    }
}
