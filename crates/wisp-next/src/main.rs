use clap::Parser;
use std::process::ExitCode;
use wisp::runtime_state::RuntimeState;
use wisp::settings::{StatusLineSettings, load_or_create_settings};
use wisp_next::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    wisp::setup_logging(Some(cli.log_dir.as_deref().unwrap_or(wisp_next::DEFAULT_LOG_DIR)));

    let settings = load_or_create_settings(StatusLineSettings::defaults());
    let state = match RuntimeState::new(&cli.agent, settings).await {
        Ok(state) => state,
        Err(e) => {
            eprintln!("Failed to initialize: {e}");
            return ExitCode::FAILURE;
        }
    };

    match wisp_next::run_with_state(state).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fatal error: {e}");
            ExitCode::FAILURE
        }
    }
}
