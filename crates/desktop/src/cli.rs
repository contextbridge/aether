use clap::Parser;

/// The agent command line used when `--agent` is not given.
pub const DEFAULT_AGENT: &str = "aether acp";

#[derive(Parser, Debug, Clone)]
#[command(name = "aether-desktop")]
#[command(about = "A native desktop client for running and managing many Aether agent threads")]
pub struct Cli {
    #[arg(
        long = "agent",
        help = "Command line that starts an agent process (speaks ACP over stdin/stdout)",
        default_value = DEFAULT_AGENT
    )]
    pub agent: String,
}
