use aether_cli::acp::{AcpArgs, AcpRunOutcome, run_acp};
use aether_cli::eval::{EvalArgs, run_eval_command};
use aether_cli::headless::{HeadlessArgs, run_headless};
use aether_cli::init::{InitOutcome, InitRequest, next_steps_message, run_init};
use aether_cli::settings::SettingsCommand;
use aether_cli::show_prompt::{PromptArgs, run_prompt};
use aether_project::{AgentCatalog, project_settings_path, user_settings_path};
use clap::{Parser, Subcommand};
use std::env::current_dir;
use std::fmt::Display;
use std::process::ExitCode;
use tokio::runtime::Runtime;
use wisp::run_tui;

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
    /// Run declarative eval specs against your configured agents
    Eval(EvalArgs),
    /// Start the ACP server
    Acp(AcpArgs),
    /// Print the fully assembled system prompt (for debugging)
    ShowPrompt(PromptArgs),
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
    let result: Result<ExitCode, String> = match cli.command {
        Some(Command::Headless(args)) => rt.block_on(run_headless(args)).map_err(|e| e.to_string()),

        Some(Command::Eval(args)) => rt.block_on(run_eval_command(args)).map_err(|e| e.to_string()),

        Some(Command::Acp(args)) => rt
            .block_on(run_acp(args))
            .map(|outcome| match outcome {
                AcpRunOutcome::CleanDisconnect => ExitCode::SUCCESS,
            })
            .map_err(|e| e.to_string()),

        Some(Command::ShowPrompt(args)) => {
            rt.block_on(run_prompt(args)).map(|()| ExitCode::SUCCESS).map_err(|e| e.to_string())
        }

        Some(Command::Settings(SettingsCommand::Init(args))) => rt.block_on(run_init_command(args.into())),

        Some(Command::Lspd(args)) => aether_lspd::run_lspd(args).map(|()| ExitCode::SUCCESS),

        None => rt.block_on(run_default_command()).map_err(|e| e.clone()),
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run_init_command(request: InitRequest) -> Result<ExitCode, String> {
    let outcome = run_init(request).await.map_err(|e| e.to_string())?;
    if let Some(msg) = next_steps_message(&outcome) {
        println!("{msg}");
    }

    Ok(match outcome {
        InitOutcome::Applied { .. } | InitOutcome::AlreadyInitialized { .. } | InitOutcome::Cancelled => {
            ExitCode::SUCCESS
        }
    })
}

async fn run_default_command() -> Result<ExitCode, String> {
    let cwd = current_dir().map_err(|e| e.to_string())?;
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

    if AgentCatalog::load_default(&cwd).map_err(|error| invalid_settings_message(&existing_settings, error))?.is_none()
    {
        let outcome = run_init(InitRequest::user_onboarding()).await.map_err(|e| e.to_string())?;
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
    run_tui("aether acp").await.map(|()| ExitCode::SUCCESS).map_err(|e| e.to_string())
}

fn invalid_settings_message(paths: &[std::path::PathBuf], error: impl Display) -> String {
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
