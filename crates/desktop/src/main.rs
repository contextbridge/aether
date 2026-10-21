use aether_desktop::cli::Cli;
use clap::Parser;
use tracing_subscriber::EnvFilter;

/// The log filter when `RUST_LOG` is unset. Every history refresh reads the whole session store,
/// and `aether_sessions` warns once per malformed line it skips.
const DEFAULT_LOG_FILTER: &str = "warn,aether_sessions=error";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_LOG_FILTER.into()))
        .with_writer(std::io::stderr)
        .init();
    aether_desktop::run(Cli::parse());
}
