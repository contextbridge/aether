use clap::Parser;
use std::process::ExitCode;
use wisp::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    wisp::setup_logging(cli.log_dir.as_deref());

    let state = match wisp::runtime_state::RuntimeState::from_cli(&cli).await {
        Ok(state) => state,
        Err(e) => {
            eprintln!("Failed to initialize: {e}");
            return ExitCode::FAILURE;
        }
    };

    match wisp::run_with_state(state).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fatal error: {e}");
            ExitCode::FAILURE
        }
    }
}
