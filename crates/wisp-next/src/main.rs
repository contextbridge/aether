use clap::Parser;
use std::process::ExitCode;
use wisp_next::cli::Cli;
use wisp_next::session::Session;
use wisp_next::settings::load_or_create_settings;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    wisp_next::setup_logging(cli.log_dir.as_deref());

    let settings = load_or_create_settings();
    let session = match Session::connect(&cli.agent, settings).await {
        Ok(session) => session,
        Err(e) => {
            eprintln!("Failed to initialize: {e}");
            return ExitCode::FAILURE;
        }
    };

    match wisp_next::run_with_session(session).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fatal error: {e}");
            ExitCode::FAILURE
        }
    }
}
