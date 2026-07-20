use clap::Parser;
use std::process::ExitCode;
use wisp::cli::Cli;
use wisp::run_tui;
use wisp::settings::load_or_create_settings;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run_tui(&cli.agent, load_or_create_settings(), cli.log_dir.as_deref()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fatal error: {e}");
            ExitCode::FAILURE
        }
    }
}
