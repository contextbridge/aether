use aether_desktop::cli::Cli;
use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

const DEFAULT_LOG_FILTER: &str = "warn,aether_sessions=error";

fn main() -> ExitCode {
    if let Some(code) = clankerdiff_lspd::run_if_requested() {
        return code;
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_LOG_FILTER.into()))
        .with_writer(std::io::stderr)
        .init();
    aether_desktop::run(Cli::parse());
    ExitCode::SUCCESS
}
